#!/usr/bin/env python3
"""Disposable checks for an administrator-trusted Ubuntu OpenSSH installation."""

import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time

import lifecycle_probe as life
import openssh_probe as native
import session_probe as session


BINARY_LIMIT = 16 * 1024 * 1024
METADATA_LIMIT = 1024 * 1024
NAMES = ("ssh", "ssh-keygen", "ssh-agent", "ssh-add")
SYSTEM_PATHS = {name: Path("/usr/bin") / name for name in (*NAMES, "dpkg-query")}
CASES = ("system-package-identity", "retained-executable-objects", "path-poison-refused",
         "unsafe-executable-refused", "native-keygen-decrypt", "native-empty-agent",
         "native-load-and-sign", "wrong-passphrase-refused", "damaged-ciphertext-refused",
         "headless-load-refused", "post-close-connections-refused")
CURRENT_REPORT = None
CURRENT_PHASE = "preflight"
UNCERTAIN_BUNDLES = []


class ProbeError(Exception):
    pass


class ExecutorFailed(ProbeError):
    def __init__(self, diagnostic):
        self.diagnostic = diagnostic
        super().__init__("native executor reported a bounded failure")


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def safe_error(error, phase):
    if isinstance(error, ExecutorFailed):
        return dict(error.diagnostic)
    return {"phase": phase, "error_type": type(error).__name__,
            "errno": error.errno if isinstance(error, OSError) else None,
            "code": "os-error" if isinstance(error, OSError) else "probe-error"}


def signature(value):
    return tuple(getattr(value, field) for field in
                 ("st_dev", "st_ino", "st_mode", "st_uid", "st_gid", "st_nlink",
                  "st_size", "st_mtime_ns", "st_ctime_ns"))


def validate_metadata(value, *, executable=False, allow_setgid=False):
    require(stat.S_ISREG(value.st_mode) and value.st_uid == 0 and not value.st_mode & 0o022,
            "system file ownership or permissions were refused")
    require(not value.st_mode & stat.S_ISUID and (allow_setgid or not value.st_mode & stat.S_ISGID),
            "system file privilege bits were refused")
    require(0 < value.st_size <= (BINARY_LIMIT if executable else METADATA_LIMIT),
            "system file size was refused")
    if executable:
        require(value.st_mode & 0o111, "system executable has no execute permission")


def check_directory_chain(path):
    require(path.is_absolute(), "system path must be absolute")
    for parent in reversed(path.parents):
        value = parent.lstat()
        require(value.st_uid == 0, "system directory owner was refused")
        if stat.S_ISLNK(value.st_mode):
            continue
        require(stat.S_ISDIR(value.st_mode) and not value.st_mode & 0o022,
                "system directory permissions were refused")


class RetainedFile:
    def __init__(self, path, *, executable=False, allow_setgid=False):
        self.original = Path(path)
        self.fd = None
        self.executable, self.allow_setgid = executable, allow_setgid
        check_directory_chain(self.original)
        original = self.original.lstat()
        require(original.st_uid == 0, "system path owner was refused")
        self.path = self.original.resolve(strict=True)
        check_directory_chain(self.path)
        before = self.path.lstat()
        validate_metadata(before, executable=executable, allow_setgid=allow_setgid)
        descriptor = os.open(self.path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        try:
            self.identity = signature(os.fstat(descriptor))
            require(self.identity == signature(before) == signature(self.path.lstat()),
                    "system file changed while opening")
            self.fd = descriptor
            if executable:
                require(os.pread(descriptor, 4, 0) == b"\x7fELF", "system executable is not ELF")
            self.sha256 = self.digest("sha256")
            self.revalidate()
        except BaseException:
            self.fd = None
            os.close(descriptor)
            raise

    def digest(self, algorithm):
        digest = hashlib.new(algorithm)
        position = 0
        while position < self.identity[6]:
            chunk = os.pread(self.fd, min(65536, self.identity[6] - position), position)
            require(chunk, "system file was truncated while reading")
            digest.update(chunk)
            position += len(chunk)
        require(not os.pread(self.fd, 1, position), "system file grew while reading")
        return digest.hexdigest()

    def revalidate(self):
        require(self.fd is not None, "retained system file is closed")
        check_directory_chain(self.original)
        require(self.original.lstat().st_uid == 0 and self.original.resolve(strict=True) == self.path,
                "system path resolution changed")
        check_directory_chain(self.path)
        require(signature(self.path.lstat()) == self.identity == signature(os.fstat(self.fd)),
                "retained system file identity changed")
        validate_metadata(os.fstat(self.fd), executable=self.executable, allow_setgid=self.allow_setgid)
        require(self.digest("sha256") == self.sha256 and signature(os.fstat(self.fd)) == self.identity,
                "retained system file content changed")

    def read(self):
        require(not self.executable, "binary contents are not metadata")
        self.revalidate()
        value = os.pread(self.fd, self.identity[6] + 1, 0)
        require(len(value) == self.identity[6], "system metadata length changed")
        self.revalidate()
        return value

    def execution_path(self):
        require(self.executable, "metadata is not executable")
        self.revalidate()
        path = Path("/proc", str(os.getpid()), "fd", str(self.fd))
        require(signature(path.stat()) == self.identity, "execution reference differs from retained object")
        return str(path)

    def close(self):
        if self.fd is not None:
            descriptor, self.fd = self.fd, None
            os.close(descriptor)


def parse_os_release(data):
    require(len(data) <= METADATA_LIMIT, "OS metadata exceeded its bound")
    values = {}
    try:
        for line in data.decode("ascii").splitlines():
            if line.split("=", 1)[0] not in ("ID", "VERSION_ID"):
                continue
            match = re.fullmatch(r'(ID|VERSION_ID)=(?:"([A-Za-z0-9._-]+)"|([A-Za-z0-9._-]+))', line)
            require(match is not None and match[1] not in values, "OS identity metadata was refused")
            values[match[1]] = match[2] or match[3]
    except UnicodeError as error:
        raise ProbeError("OS metadata encoding was refused") from error
    require(values == {"ID": "ubuntu", "VERSION_ID": "24.04"}, "OS family is outside the fixture profile")
    return values


def parse_md5(data):
    require(len(data) <= METADATA_LIMIT, "package metadata exceeded its bound")
    values = {}
    try:
        for line in data.decode("ascii").splitlines():
            match = re.fullmatch(r"([0-9a-f]{32})  ([A-Za-z0-9_+./:@-]+)", line)
            require(match is not None, "package checksum metadata was refused")
            path = match[2]
            require(not path.startswith("/") and all(part not in ("", ".", "..") for part in path.split("/"))
                    and path not in values, "package checksum path was refused")
            values[path] = match[1]
    except UnicodeError as error:
        raise ProbeError("package checksum encoding was refused") from error
    require(values, "package checksum metadata is empty")
    return values


def parse_package(data):
    require(len(data) <= 512, "package receipt exceeded its bound")
    try:
        match = re.fullmatch(rb"openssh-client\t([0-9][0-9A-Za-z.+:~\-]{0,127})\t([a-z0-9][a-z0-9-]{0,31})\tinstalled\n", data)
        require(match is not None, "installed package receipt was refused")
        return match[1].decode("ascii"), match[2].decode("ascii")
    except UnicodeError as error:
        raise ProbeError("package receipt encoding was refused") from error


def package_owners(data):
    require(len(data) <= 2048, "package ownership receipt exceeded its bound")
    try:
        lines = data.decode("ascii").splitlines()
    except UnicodeError as error:
        raise ProbeError("package ownership encoding was refused") from error
    expected = {"openssh-client: " + SYSTEM_PATHS[name].as_posix() for name in NAMES}
    require(len(lines) == len(expected) and set(lines) == expected, "system executable package ownership differs")


class Provider:
    def __init__(self):
        self.files = {}

    def open(self):
        try:
            for name, path in SYSTEM_PATHS.items():
                self.files[name] = RetainedFile(path, executable=True, allow_setgid=name == "ssh-agent")
            for name, path in (("os-release", "/etc/os-release"),
                               ("package-status", "/var/lib/dpkg/status"),
                               ("package-files", "/var/lib/dpkg/info/openssh-client.list"),
                               ("md5sums", "/var/lib/dpkg/info/openssh-client.md5sums")):
                self.files[name] = RetainedFile(path)
            parse_os_release(self.files["os-release"].read())
        except BaseException:
            self.close()
            raise

    def revalidate(self):
        for value in self.files.values():
            value.revalidate()

    def execute(self, name, arguments, env, *, responses=None, **options):
        self.revalidate()
        command = [self.files[name].execution_path(), *arguments]
        try:
            return native.bounded_run(command, env) if responses is None else native.pty_run(command, env, responses, **options)
        finally:
            self.revalidate()

    def package(self, env):
        code, output = self.execute("dpkg-query", ["-W", "-f=${Package}\t${Version}\t${Architecture}\t${db:Status-Status}\n", "openssh-client"], env)
        require(code == 0, "package metadata query failed")
        version, architecture = parse_package(output)
        code, output = self.execute("dpkg-query", ["-S", *(SYSTEM_PATHS[name].as_posix() for name in NAMES)], env)
        require(code == 0, "package ownership query failed")
        package_owners(output)
        checksums = parse_md5(self.files["md5sums"].read())
        for name in NAMES:
            require(checksums.get(SYSTEM_PATHS[name].as_posix().lstrip("/")) == self.files[name].digest("md5"),
                    "system file differs from local package checksum")
        self.revalidate()
        return version, architecture

    def components(self):
        self.revalidate()
        return [{"name": name, "path": str(self.files[name].path), "sha256": self.files[name].sha256,
                 "root_owned": True, "nonwritable": True, "fd_retained": True, "revalidated": True}
                for name in NAMES]

    def close(self):
        for value in self.files.values():
            value.close()


def receive_executor(control):
    value = life.receive_json(control, time.monotonic() + native.TIMEOUT)
    if isinstance(value, dict) and value.get("kind") == "executor-error":
        diagnostic = value.get("diagnostic")
        require(isinstance(diagnostic, dict) and set(diagnostic) == {"phase", "error_type", "errno", "code"}
                and diagnostic["phase"] in ("executor-parent", "executor-arm", "executor-barrier", "executor-exec")
                and isinstance(diagnostic["error_type"], str)
                and re.fullmatch(r"[A-Za-z][A-Za-z0-9]{0,63}", diagnostic["error_type"]) is not None
                and diagnostic["code"] in ("os-error", "probe-error")
                and (diagnostic["errno"] is None or type(diagnostic["errno"]) is int
                     and 0 <= diagnostic["errno"] <= 65535), "executor diagnostic was refused")
        raise ExecutorFailed(diagnostic)
    return value


def agent_exec(args):
    control = socket.socket(fileno=args.control_fd)
    phase = "executor-parent"
    try:
        native.child_setup()
        require(os.getppid() == args.expected_parent, "agent executor parent changed")
        require(re.fullmatch(r"/proc/" + str(args.expected_parent) + r"/fd/[0-9]+", args.executable) is not None,
                "agent executable is outside its parent's retained objects")
        phase = "executor-arm"
        require(life.configure_parent_death() == "armed" and os.getppid() == args.expected_parent,
                "agent parent-death link was refused")
        life.send_json(control, {"kind": "armed", "pid": os.getpid(), "parent": args.expected_parent})
        phase = "executor-barrier"
        require(life.receive_command(control) == b"G" and os.getppid() == args.expected_parent,
                "agent execution barrier was refused")
        phase = "executor-exec"
        os.set_inheritable(args.control_fd, False)
        os.execve(args.executable, [args.executable, "-D", "-a", str(args.agent_socket)],
                  life.private_env(args.root, args.agent_socket))
    except BaseException as error:
        try:
            life.send_json(control, {"kind": "executor-error", "diagnostic": safe_error(error, phase)})
        except (OSError, life.ProbeError):
            pass
        raise
    finally:
        control.close()


class OwnedAgent:
    def __init__(self, provider, root, name):
        self.provider, self.root = provider, root
        self.path = root / (name + ".sock")
        self.process = self.pidfd = None
        self.start_time = None
        self.exit_observed = False

    def start(self):
        parent, child = socket.socketpair()
        try:
            self.provider.revalidate()
            executable = self.provider.files["ssh-agent"].execution_path()
            self.process = subprocess.Popen(
                [sys.executable, "-B", str(Path(__file__).resolve()), "--internal-agent-exec",
                 "--expected-parent", str(os.getpid()), "--executable", executable,
                 "--control-fd", str(child.fileno()), "--agent-socket", str(self.path), "--root", str(self.root)],
                env=life.private_env(self.root), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, pass_fds=(child.fileno(),), close_fds=True, start_new_session=True)
            child.close()
            self.pidfd = os.pidfd_open(self.process.pid)
            self.start_time = life.process_start(self.process.pid)
            require(receive_executor(parent)
                    == {"kind": "armed", "pid": self.process.pid, "parent": os.getpid()},
                    "owned agent executor identity changed")
            parent.sendall(b"G")
            self.wait_ready(parent)
            life.inspect_agent(self.process.pid, self.start_time, executable, os.getpid())
            require(os.getsid(self.process.pid) == self.process.pid and os.getpgid(self.process.pid) == self.process.pid,
                    "owned agent has no private session")
            require(native.identities(self.path) == [], "new owned agent is not empty")
            self.provider.revalidate()
        finally:
            parent.close()
            child.close()

    def wait_ready(self, control):
        deadline = time.monotonic() + native.TIMEOUT
        while time.monotonic() < deadline:
            if control is not None and select.select([control], [], [], 0.025)[0]:
                if control.recv(1, socket.MSG_PEEK):
                    receive_executor(control)
                    raise ProbeError("native executor sent an unexpected control message")
                control = None
            if self.process.poll() is not None:
                if control is not None:
                    continue
                raise ProbeError("native agent exited before readiness")
            if self.path.exists():
                try:
                    native.identities(self.path)
                    return
                except (OSError, native.ProbeError):
                    pass
            time.sleep(0.025)
        raise ProbeError("native agent readiness timed out")

    def stop(self):
        if self.process is None:
            return
        if self.pidfd is not None:
            life.stop_agent(self.pidfd)
        elif self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=life.DEATH_TIMEOUT)
        require(self.pidfd is None or life.pidfd_ready(self.pidfd, 0), "owned agent exit remains uncertain")
        self.exit_observed = True

    def close(self):
        require(self.process is None or self.exit_observed, "agent resources precede native exit")
        if self.pidfd is not None:
            os.close(self.pidfd)
            self.pidfd = None
        if self.path.exists():
            life.unlink_owned_socket(self.path)


def finish(bundle, primary):
    if primary is not None and CURRENT_REPORT is not None:
        CURRENT_REPORT["primary_failure"] = safe_error(primary, CURRENT_PHASE)
    errors = []
    for agent in bundle["agents"]:
        try:
            agent.stop()
        except BaseException as error:
            errors.append(error)
    if errors:
        UNCERTAIN_BUNDLES.append(bundle)
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(errors[0], "cleanup-native-exit")
        if primary is None:
            raise ProbeError("native cleanup remains uncertain") from errors[0]
        return
    try:
        for agent in bundle["agents"]:
            agent.close()
        if bundle.get("connection") is not None:
            bundle["connection"].close()
        bundle["provider"].close()
        session.package_backend().cleanup_owned_tree(bundle["root"], bundle["identity"])
    except BaseException as error:
        UNCERTAIN_BUNDLES.append(bundle)
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(error, "cleanup-resources")
        if primary is None:
            raise


def record(report, name, field):
    report["cases"].append({"case": name, "observed": True})
    report[field] = True


def keygen_matches(output, expected):
    for line in output.splitlines():
        fields = line.strip().split()
        if len(fields) >= 2 and fields[0] == b"ssh-ed25519":
            try:
                return base64.b64decode(fields[1], validate=True) == expected
            except ValueError:
                return False
    return False


def run_probe(writer, verifier):
    global CURRENT_REPORT, CURRENT_PHASE
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "provider_admitted": False, "provider_capabilities_tested": False,
              "provenance_kind": "administrator-trusted-dpkg-database", "archive_signature_verified": False,
              "os_id": "ubuntu", "os_version": "24.04", "package_name": "openssh-client",
              "cases": [], "sentinel_survived": False}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="provider_probe.py")
    provider = Provider()
    root = Path(tempfile.mkdtemp(prefix="vivac-provider-", dir="/tmp"))
    root.chmod(0o700)
    identity = session.package_backend().validate_safe_tree_root(root)
    bundle = {"provider": provider, "root": root, "identity": identity, "agents": [], "connection": None}
    primary = None
    try:
        CURRENT_PHASE = "provider-admission"
        provider.open()
        (root / "home").mkdir(mode=0o700)
        env = life.private_env(root)
        version, architecture = provider.package(env)
        report.update(package_version=version, package_arch=architecture,
                      package_database_matched=True, system_package_md5_matched=True)
        record(report, "system-package-identity", "package_database_matched")
        report["components"] = provider.components()
        record(report, "retained-executable-objects", "retained_objects_checked")
        report["trusted_paths_checked"] = True
        poison = root / "poison"
        poison.mkdir(mode=0o700)
        marker = root / "poison-executed"
        native.private_write(poison / "ssh-add", b"#!/bin/sh\n: > " + os.fsencode(marker) + b"\n")
        (poison / "ssh-add").chmod(0o700)
        env["PATH"] = str(poison)
        try:
            unsafe = RetainedFile(poison / "ssh-add", executable=True)
        except ProbeError:
            pass
        else:
            unsafe.close()
            raise ProbeError("private executable candidate was admitted")
        record(report, "unsafe-executable-refused", "unsafe_path_refused")
        CURRENT_PHASE = "fixture-create"
        _, _, comment, parsed, challenge, message, _ = life.create_fixture(root, writer, verifier)
        key = root / "fixture-key"
        CURRENT_PHASE = "keygen-decrypt"
        code, output = provider.execute("ssh-keygen", ["-y", "-f", str(key)], env, responses=[native.PASSWORD])
        require(code == 0 and keygen_matches(output, parsed["public_blob"]), "native keygen public key differs")
        record(report, "native-keygen-decrypt", "keygen_public_matches")
        agent, sentinel = OwnedAgent(provider, root, "agent"), OwnedAgent(provider, root, "sentinel")
        bundle["agents"] = [agent, sentinel]
        CURRENT_PHASE = "agent-start"
        agent.start()
        sentinel.start()
        record(report, "native-empty-agent", "native_empty_agent")
        agent_env, sentinel_env = dict(env, SSH_AUTH_SOCK=str(agent.path)), dict(env, SSH_AUTH_SOCK=str(sentinel.path))
        CURRENT_PHASE = "native-load-sign"
        for target_env in (agent_env, sentinel_env):
            code, _ = provider.execute("ssh-add", ["-k", str(key)], target_env, responses=[native.PASSWORD])
            require(code == 0, "native encrypted key load failed")
        require(native.identities(agent.path) == [(parsed["public_blob"], comment.encode("ascii"))],
                "loaded native identity differs")
        report["native_identity_verified"] = True
        connection, signed = life.open_signed_connection(agent.path, parsed["public_blob"], comment, message)
        bundle["connection"] = connection
        value = native.proof(challenge, signed)
        native.verify(verifier, root, env, value, "provider-valid", True)
        altered = copy.deepcopy(value)
        altered["challenge"]["base_revision"] += 1
        native.verify(verifier, root, env, altered, "provider-altered", False)
        record(report, "native-load-and-sign", "native_proof_verified")
        connection.close()
        bundle["connection"] = None
        CURRENT_PHASE = "wrong-passphrase"
        native.clear_agent(agent.path)
        code, output = provider.execute("ssh-add", ["-k", str(key)], agent_env, responses=[native.WRONG_PASSWORD], allow_retry_eof=True)
        require(code != 0 and b"Bad passphrase" in output and native.identities(agent.path) == [],
                "wrong passphrase was not explicitly refused")
        record(report, "wrong-passphrase-refused", "wrong_passphrase_refused")
        CURRENT_PHASE = "damaged-ciphertext"
        damaged = root / "damaged-key"
        native.corrupt_tag(key, damaged)
        code, _ = provider.execute("ssh-add", ["-k", str(damaged)], agent_env, responses=[native.PASSWORD],
                                   require_responses=False, allow_retry_eof=True)
        require(code != 0 and native.identities(agent.path) == [], "damaged ciphertext was accepted")
        record(report, "damaged-ciphertext-refused", "damaged_ciphertext_refused")
        CURRENT_PHASE = "headless-load"
        code, _ = provider.execute("ssh-add", ["-k", str(key)], agent_env)
        require(code != 0 and native.identities(agent.path) == [], "headless load was accepted")
        record(report, "headless-load-refused", "headless_load_refused")
        CURRENT_PHASE = "native-close"
        code, _ = provider.execute("ssh-add", ["-k", str(key)], agent_env, responses=[native.PASSWORD])
        require(code == 0, "native reload failed")
        connection, signed = life.open_signed_connection(agent.path, parsed["public_blob"], comment, message)
        bundle["connection"] = connection
        native.verify(verifier, root, env, native.proof(challenge, signed), "provider-before-close", True)
        agent.stop()
        require(agent.exit_observed and agent.process.returncode == -signal.SIGKILL,
                "owned native agent kill was not observed")
        require(life.request_after_death(connection, parsed["public_blob"], message) in ("transport_refused", "eof", "refused"),
                "retained connection signed after closure")
        require(life.new_connection_after_death(agent.path, parsed["public_blob"], message)
                in ("connect_refused", "transport_refused", "eof", "refused"), "new connection signed after closure")
        report.update(agent_exit_observed=True, retained_connection_refused=True, new_connection_refused=True)
        record(report, "post-close-connections-refused", "agent_exit_observed")
        require(sentinel.process.poll() is None and not life.pidfd_ready(sentinel.pidfd, 0), "sentinel was affected")
        signature_value = native.sign(sentinel.path, parsed["public_blob"], message)
        native.verify(verifier, root, env, native.proof(challenge, signature_value), "provider-sentinel", True)
        report["sentinel_survived"] = True
        require(not marker.exists(), "PATH homonym was executed")
        record(report, "path-poison-refused", "path_poison_ignored")
        provider.revalidate()
        report["components"] = provider.components()
        report["snapshots_revalidated"] = True
        require(len(report["cases"]) == len(CASES) and {case["case"] for case in report["cases"]} == set(CASES),
                "provider matrix is incomplete")
    except BaseException as error:
        primary = error
        raise
    finally:
        finish(bundle, primary)
    report["provider_capabilities_tested"] = True
    report["status"] = "observed"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("writer", "verifier", "report", "root", "agent-socket"):
        parser.add_argument("--" + name, type=Path)
    parser.add_argument("--internal-agent-exec", action="store_true")
    parser.add_argument("--expected-parent", type=int)
    parser.add_argument("--control-fd", type=int)
    parser.add_argument("--executable")
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "process is missing no_new_privs")
        if args.internal_agent_exec:
            return agent_exec(args)
        require(args.writer is not None and args.verifier is not None and args.report is not None,
                "required experiment arguments are absent")
        session.require_report_directory(args.report)
        result = run_probe(args.writer.resolve(strict=True), args.verifier.resolve(strict=True))
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Provider observations recorded; custody_accredited=false")
        return 0
    except Exception as error:
        if args.report is not None:
            try:
                session.require_report_directory(args.report)
                result = dict(CURRENT_REPORT or {})
                result.update(status="incomplete", custody_accredited=False, provider_admitted=False,
                              phase=CURRENT_PHASE, failure=safe_error(error, CURRENT_PHASE))
                life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
            except (OSError, session.ProbeError):
                pass
        print("Provider probe failed; no result is accredited.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
