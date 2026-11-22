#!/usr/bin/env python3
"""Disposable sequential session admission and idle-lock experiment."""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time

import lifecycle_probe as life
import openssh_probe as native


CURRENT_STAGE = "preflight"
CURRENT_REPORT = None
CASES = ("ready-sign-idle-lock", "changed-binding-before-ready",
         "changed-lease-before-ready", "changed-lease-before-sign",
         "changed-lease-before-publish", "lock-during-startup", "agent-death-before-sign")


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def package_backend():
    # This existing Linux-only fixture is loaded only when needed.
    import package_probe
    return package_probe


class Session:
    def __init__(self, writer, verifier, package, root):
        self.writer, self.verifier, self.package, self.root = writer, verifier, package, root
        self.socket_path = root / "session.sock"
        self.env = life.private_env(root, self.socket_path)
        self.state = "NEW"
        self.ready_published = False
        self.close_acknowledged = False
        self.native_sign_calls = 0
        self.published_proofs = 0
        self.proof_sequence = 0
        self.snapshot = None
        self.memfd = None
        self.agent = None
        self.pidfd = None
        self.agent_start = None
        self.agent_loaded = False
        self.expected_public_blob = None
        self.expected_comment = None

    def _open_package(self):
        package = package_backend()
        code, _ = package.check(self.writer, self.package, self.env)
        require(code == 0, "structural package check was refused")
        self.snapshot = package.open_retained_package(self.package)
        package.revalidate_retained_package(self.snapshot)
        parsed = native.parse_key_bytes(self.snapshot["key_bytes"])
        manifest = self.snapshot["manifest"]
        require(parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["kdf"] == "bcrypt"
                and parsed["rounds"] == 64 and manifest["key_size"] == len(self.snapshot["key_bytes"])
                and manifest["key_sha256"] == self.snapshot["key_sha256"]
                and manifest["public_key"] == parsed["public_key"].hex(),
                "retained encrypted package profile was refused")
        self.expected_public_blob = parsed["public_blob"]
        self.expected_comment = "vivac.device-key.v1:" + manifest["device_id"] + ":" + manifest["credential_id"]
        self.memfd, _ = package.make_sealed_key_memfd(self.snapshot["key_bytes"])

    def _launch_agent(self, *, isolate_terminal=False):
        observer, executor = socket.socketpair()
        try:
            observer.settimeout(life.TIMEOUT)
            arguments = [sys.executable, str(Path(life.__file__).resolve()), "--internal-agent-exec",
                         "--expected-parent", str(os.getpid()), "--agent-socket", str(self.socket_path),
                         "--home", str(self.root / "home"), "--control-fd", str(executor.fileno())]
            self.agent = subprocess.Popen(
                arguments, env=self.env, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                pass_fds=(executor.fileno(),), close_fds=True, preexec_fn=native.child_setup,
                start_new_session=isolate_terminal,
            )
            executor.close()
            self.pidfd = os.pidfd_open(self.agent.pid)
            self.agent_start = life.process_start(self.agent.pid)
            expected = {"kind": "prearm", "pid": self.agent.pid, "parent": os.getpid()}
            require(life.receive_json(observer, time.monotonic() + life.TIMEOUT) == expected,
                    "agent executor pre-arm identity changed")
            observer.sendall(b"A")
            expected["kind"] = "armed"
            require(life.receive_json(observer, time.monotonic() + life.TIMEOUT) == expected,
                    "agent executor did not acknowledge its parent-death link")
            observer.sendall(b"G")
            life.wait_socket(self.socket_path, self.agent, self.agent.pid)
            expected_exe = shutil.which("ssh-agent", path=os.defpath)
            life.inspect_agent(self.agent.pid, self.agent_start, expected_exe, os.getpid())
            require(native.identities(self.socket_path) == [], "new owned agent was not empty")
        finally:
            observer.close()
            executor.close()

    def _validate(self):
        require(self.state in ("STARTING", "READY"), "session admission is closed")
        require(self.pidfd is not None and not life.pidfd_ready(self.pidfd, 0), "owned agent is not alive")
        require(life.process_start(self.agent.pid) == self.agent_start, "owned agent identity changed")
        package_backend().revalidate_retained_package(self.snapshot)

    def _load_agent(self):
        self._validate()
        executable = shutil.which("ssh-add", path=os.defpath)
        require(executable is not None, "native loader is unavailable")
        try:
            code, _ = native.pty_run(
                [executable, "-k", "/proc/self/fd/" + str(self.memfd)], self.env, [native.PASSWORD],
                inherited_fds=(self.memfd,),
            )
        finally:
            self._validate()
        require(code == 0, "native loader refused the sealed ciphertext")
        self.agent_loaded = True

    def _proof(self):
        self.proof_sequence += 1
        name = "session-proof-" + str(self.proof_sequence)
        path = self.root / (name + ".json")
        def signer(public_blob, message):
            self._validate()
            self.native_sign_calls += 1
            signature = native.sign(self.socket_path, public_blob, message)
            self._validate()
            return signature
        try:
            accepted = package_backend().produce_loaded_proof(
                self.verifier, self.root, self.env, self.socket_path, self.snapshot,
                self.expected_public_blob, self.expected_comment, name, signer,
            )
            require(accepted, "loaded identity proof was refused")
            raw = native.read_private_file(path)
            require(len(raw) <= 4096, "session proof exceeded its bound")
            return json.loads(raw.decode("utf-8"))
        finally:
            if path.exists():
                path.unlink()

    def start(self, *, before_ready=None):
        require(self.state == "NEW", "session cannot be reopened")
        self.state = "STARTING"
        try:
            self._open_package()
            self._launch_agent()
            self._load_agent()
            if before_ready is not None:
                before_ready(self)
            require(self.state == "STARTING", "startup admission was closed")
            self._validate()
            receipt = self._proof()
            self._validate()
            require(self.state == "STARTING", "startup admission was closed")
            self.state = "READY"
            self.ready_published = True
            return receipt
        except BaseException:
            if self.state != "CLOSED":
                self.lock()
            raise

    def sign(self, *, before_publish=None):
        require(self.state == "READY", "session is not ready")
        try:
            self._validate()
            proof = self._proof()
            if before_publish is not None:
                before_publish(self)
            self._validate()
            require(self.state == "READY", "proof publication admission was closed")
            self.published_proofs += 1
            return proof
        except BaseException:
            if self.state != "CLOSED":
                self.lock()
            raise

    def _stop_agent(self):
        if self.agent is None:
            return
        if self.pidfd is not None:
            life.stop_agent(self.pidfd)
        elif self.agent.poll() is None:
            self.agent.kill()
        self.agent.wait(timeout=life.DEATH_TIMEOUT)
        if self.pidfd is not None:
            require(life.pidfd_ready(self.pidfd, 0), "agent exit was not observed before close")

    def _release(self):
        # Every resource gets its cleanup even if native exit cannot be confirmed.
        stopped = False
        with ExitStack() as cleanup:
            if self.snapshot is not None:
                cleanup.callback(self._close_snapshot)
            if self.memfd is not None:
                cleanup.callback(self._close_descriptor, "memfd")
            if self.pidfd is not None:
                cleanup.callback(self._close_descriptor, "pidfd")
            self._stop_agent()
            stopped = True
        if stopped:
            life.unlink_owned_socket(self.socket_path)

    def _close_descriptor(self, name):
        descriptor = getattr(self, name)
        setattr(self, name, None)
        if descriptor is not None:
            os.close(descriptor)

    def _close_snapshot(self):
        snapshot, self.snapshot = self.snapshot, None
        if snapshot is not None:
            package_backend().close_retained_package(snapshot)

    def lock(self):
        if self.state == "CLOSED":
            return True
        self.state = "CLOSING"
        self._release()
        self.state = "CLOSED"
        self.close_acknowledged = True
        return True


def require_report_directory(path):
    require(path.is_absolute() and not path.exists() and not path.is_symlink(), "report path must be new and absolute")
    require(path.parent.is_dir() and os.access(path.parent, os.W_OK | os.X_OK), "report directory is not writable")


def require_store(store):
    require(store == Path("/store/run"), "store root is outside the dedicated fixture mount")
    mount = Path("/store")
    require(not mount.is_symlink() and not store.is_symlink(), "store mount or root is a symlink")
    package = package_backend()
    require(package.is_mountpoint(mount), "store is not a dedicated mountpoint")
    metadata = store.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid()
            and stat.S_IMODE(metadata.st_mode) == 0o700 and not package.acl_xattrs(store),
            "store root metadata was refused")
    require(not any(store.iterdir()), "store root must be empty")
    require(package.filesystem_magic(store) == package.EXT_MAGIC,
            "store filesystem is outside the ext-family fixture")


def mutate_key(package):
    path = package / "key.openssh"
    original = path.read_bytes()
    changed = bytearray(original)
    changed[len(changed) // 2] ^= 1
    package_backend().write_in_place(path, bytes(changed))
    return lambda: package_backend().write_in_place(path, original)


def refuse_reopen(session):
    for operation in (session.start, session.sign):
        try:
            operation()
        except ProbeError:
            pass
        else:
            raise ProbeError("closed session accepted another operation")


def expect_refusal(operation, expected_type, *, message=None):
    try:
        operation()
    except expected_type as error:
        require(message is None or str(error) == message, "negative case reached the wrong rejection gate")
        return
    raise ProbeError("negative case did not refuse its operation")


def run_case(writer, verifier, store, root, name):
    global CURRENT_STAGE
    CURRENT_STAGE = name
    package = package_backend()
    case_root = Path(tempfile.mkdtemp(prefix="session-case-", dir=root))
    case_root.chmod(0o700)
    root_identity = package.validate_safe_tree_root(case_root)
    (case_root / "home").mkdir(mode=0o700)
    package_path = store / name
    package.create_positive(writer, package_path, life.private_env(case_root))
    package_identity = package.validate_safe_tree_root(package_path)
    session = Session(writer, verifier, package_path, case_root)
    connection = None
    retained_pidfd = None
    restore = None
    initial_calls = 0
    seam_reached = False
    try:
        if name == "changed-binding-before-ready":
            manifest, _ = package.read_manifest(package_path)
            manifest["device_id"] = native.identifier()
            package.write_in_place(package_path / "manifest.json", package.manifest_bytes(manifest))
            code, _ = package.check(writer, package_path, session.env)
            require(code == 0, "changed-ID fixture was not structurally valid")
        before_ready = None
        if name == "changed-lease-before-ready":
            def before_ready(value):
                nonlocal restore, seam_reached
                restore = mutate_key(value.package)
                seam_reached = True
        elif name == "lock-during-startup":
            def before_ready(value):
                nonlocal seam_reached
                seam_reached = True
                value.lock()
        startup_refusal = name in ("changed-binding-before-ready", "changed-lease-before-ready", "lock-during-startup")
        if startup_refusal:
            expected_type = {"changed-binding-before-ready": package.BindingMismatch,
                             "changed-lease-before-ready": package.LeaseInvalidated,
                             "lock-during-startup": ProbeError}[name]
            expect_refusal(lambda: session.start(before_ready=before_ready), expected_type,
                           message="startup admission was closed" if name == "lock-during-startup" else None)
            require(session.agent_loaded, "negative startup did not reach completed native loading")
            if name == "changed-binding-before-ready":
                seam_reached = True
            require(seam_reached, "negative startup did not reach its selected seam")
            require(not session.ready_published and session.native_sign_calls == 0,
                    "negative startup reached native signing or readiness")
        else:
            receipt = session.start()
            native.verify(verifier, case_root, session.env, receipt, "readiness-independent", True)
            initial_calls = session.native_sign_calls
            require(initial_calls == 1, "readiness did not require one native signature")
            if name == "ready-sign-idle-lock":
                proof = session.sign()
                native.verify(verifier, case_root, session.env, proof, "sign-independent", True)
                message = native.canonical(proof["challenge"])
                connection, signature = life.open_signed_connection(
                    session.socket_path, session.expected_public_blob, session.expected_comment, message)
                native.verify(verifier, case_root, session.env,
                              native.proof(proof["challenge"], signature), "retained-before-lock", True)
                retained_pidfd = os.dup(session.pidfd)
                session.lock()
                require(life.pidfd_ready(retained_pidfd, 0), "lock acknowledged before observed native exit")
                require(life.request_after_death(connection, session.expected_public_blob, message)
                        in ("transport_refused", "eof", "refused"), "retained connection signed after lock")
                require(life.new_connection_after_death(session.socket_path, session.expected_public_blob, message)
                        in ("connect_refused", "transport_refused", "eof", "refused"),
                        "new connection signed after lock")
                require(session.published_proofs == 1, "successful request did not publish exactly one proof")
                session.lock()
            else:
                if name == "changed-lease-before-sign":
                    restore = mutate_key(package_path)
                    seam_reached = True
                elif name == "agent-death-before-sign":
                    life.pidfd_signal(session.pidfd, signal.SIGKILL)
                    require(life.pidfd_ready(session.pidfd), "agent death was not observed")
                    seam_reached = True
                def before_publish(value):
                    nonlocal restore, seam_reached
                    restore = mutate_key(value.package)
                    seam_reached = True
                expect_refusal(
                    lambda: session.sign(before_publish=before_publish if name == "changed-lease-before-publish" else None),
                    ProbeError if name == "agent-death-before-sign" else package.LeaseInvalidated,
                    message="owned agent is not alive" if name == "agent-death-before-sign" else None,
                )
                require(seam_reached, "negative request did not reach its selected seam")
                expected_calls = initial_calls + (1 if name == "changed-lease-before-publish" else 0)
                require(session.native_sign_calls == expected_calls and session.published_proofs == 0,
                        "negative request crossed the wrong signing/publication boundary")
        require(session.state == "CLOSED" and session.close_acknowledged,
                "case did not reach observed terminal closure")
        if restore is not None:
            restore()
            restore = None
        refuse_reopen(session)
        return {"case": name, "state": session.state, "readiness_published": session.ready_published,
                "gate_native_sign_calls": session.native_sign_calls, "published_proofs": session.published_proofs,
                "readiness_sign_calls": initial_calls,
                "request_native_sign_calls": session.native_sign_calls - initial_calls,
                "fixture_connection_sign_calls": 1 if name == "ready-sign-idle-lock" else 0,
                "negative_seam_reached": seam_reached,
                "close_acknowledged": session.close_acknowledged, "closed_session_refused": True}
    finally:
        try:
            if session.state != "CLOSED":
                session.lock()
        finally:
            if connection is not None:
                connection.close()
            if retained_pidfd is not None:
                os.close(retained_pidfd)
            if restore is not None:
                restore()
            package.cleanup_owned_tree(package_path, package_identity)
            package.cleanup_owned_tree(case_root, root_identity)


def run_probe(writer, verifier, store):
    global CURRENT_STAGE, CURRENT_REPORT
    CURRENT_STAGE = "preflight"
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "concurrent_lock_tested": False, "ssh_disconnect_tested": False,
              "cases": [], "sentinel_survived": False}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="session_probe.py")
    require_store(store)
    with tempfile.TemporaryDirectory(prefix="vivac-session-", dir="/tmp") as temporary:
        root = Path(temporary).resolve()
        root.chmod(0o700)
        (root / "home").mkdir(mode=0o700)
        fixture = life.create_fixture(root, writer, verifier)
        ssh_agent, ssh_add, comment, parsed, challenge, message, env = fixture
        sentinel_socket = root / "sentinel.sock"
        with life.owned_sentinel(ssh_agent, root, sentinel_socket) as (sentinel, pidfd):
            life.wait_socket(sentinel_socket, sentinel, sentinel.pid)
            code, _ = native.pty_run([ssh_add, "-k", str(root / "fixture-key")],
                                     life.private_env(root, sentinel_socket), [native.PASSWORD])
            require(code == 0, "sentinel credential load failed")
            for name in CASES:
                report["cases"].append(run_case(writer, verifier, store, root, name))
                require(sentinel.poll() is None and not life.pidfd_ready(pidfd, 0), "sentinel was affected")
                signature = native.sign(sentinel_socket, parsed["public_blob"], message)
                native.verify(verifier, root, env, native.proof(challenge, signature), "sentinel-" + name, True)
            report["sentinel_survived"] = True
    report["status"] = "observed"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--writer", type=Path, required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--store-root", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        require_report_directory(args.report)
        writer, verifier = args.writer.resolve(strict=True), args.verifier.resolve(strict=True)
        store = args.store_root
        require(writer.is_file() and verifier.is_file() and os.access(writer, os.X_OK)
                and os.access(verifier, os.X_OK), "required helper executable is unavailable")
        result = run_probe(writer, verifier, store)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Session observations recorded; custody_accredited=false")
        return 0
    except (ProbeError, life.ProbeError, native.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        failure = str(error) if isinstance(error, (ProbeError, life.ProbeError, native.ProbeError)) else type(error).__name__
    except Exception as error:
        # Package fixture exception types are imported lazily; never expose their payloads.
        failure = type(error).__name__
    try:
        result = dict(CURRENT_REPORT or {})
        result.update({"version": 1, "status": "incomplete", "custody_accredited": False,
                       "stage": CURRENT_STAGE, "failure": failure})
        require_report_directory(args.report)
        life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
    except (ProbeError, OSError):
        pass
    print("Session probe failed; no result is accredited.", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
