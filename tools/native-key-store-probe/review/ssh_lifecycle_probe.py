#!/usr/bin/env python3
"""Disposable supervisor lifetime checks over an isolated SSH connection."""

import argparse
import array
import errno
import json
import os
from pathlib import Path
import select
import shlex
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time

import lifecycle_probe as life
import loader_probe as loader
import openssh_probe as native
import session_probe as session


TIMEOUT = 15
CASES = tuple(mode + "-" + suffix for mode in ("pty", "pipe")
              for suffix in ("normal-close", "disconnect-prompt", "disconnect-secret-handoff", "disconnect-ready"))
CURRENT_STAGE = "preflight"
CURRENT_REPORT = None
UNCERTAIN_BUNDLES = []


class ProbeError(Exception):
    pass


class SSHDisconnected(ProbeError):
    def __init__(self, cause):
        self.cause = cause
        super().__init__("SSH connection ended")


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def safe_error(error, phase):
    if isinstance(error, loader.WorkerFailed):
        return dict(error.diagnostic)
    return {"phase": phase, "error_type": type(error).__name__,
            "errno": error.errno if isinstance(error, OSError) else None,
            "code": "os-error" if isinstance(error, OSError) else "probe-error"}


def receive(control):
    value = life.receive_json(control, time.monotonic() + TIMEOUT)
    if isinstance(value, dict) and value.get("kind") == "worker-error":
        error = loader.WorkerFailed(loader.validate_diagnostic(value.get("diagnostic")))
        cleanup = value.get("cleanup_diagnostic")
        error.cleanup_diagnostic = None if cleanup is None else loader.validate_diagnostic(cleanup)
        raise error
    return value


def receive_master(control):
    control.settimeout(TIMEOUT)
    received, accepted = [], False
    try:
        size = array.array("i").itemsize
        payload, controls, flags, _ = control.recvmsg(1, socket.CMSG_SPACE(8 * size))
        valid = True
        for level, kind, raw in controls:
            if level != socket.SOL_SOCKET or kind != socket.SCM_RIGHTS:
                valid = False
                continue
            descriptors = array.array("i")
            descriptors.frombytes(raw[:len(raw) - len(raw) % size])
            received.extend(descriptors)
            valid = valid and len(raw) % size == 0
        if payload == b"E":
            require(not controls and not received and not flags & socket.MSG_CTRUNC,
                    "worker error descriptor frame was refused")
            receive(control)
            raise ProbeError("worker error frame lacked its diagnostic")
        require(payload == b"F" and valid and not flags & socket.MSG_CTRUNC and len(received) == 1,
                "PTY descriptor transfer was refused")
        descriptor = received[0]
        os.set_inheritable(descriptor, False)
        require(stat.S_ISCHR(os.fstat(descriptor).st_mode) and os.isatty(descriptor)
                and Path(os.ttyname(descriptor)).name == "ptmx", "received descriptor is not a PTY master")
        accepted = True
        return descriptor
    finally:
        if not accepted:
            errors = []
            for descriptor in received:
                try:
                    os.close(descriptor)
                except OSError as error:
                    errors.append(error)
            if errors:
                raise errors[0]


class SSHChannel:
    def __init__(self, control, input_fd=0):
        self.control, self.input_fd = control, input_fd
        self.hup = False

    def on_hup(self, number, frame):
        self.hup = True

    def command(self):
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            if self.hup:
                raise SSHDisconnected("sighup")
            readable = select.select([self.control, self.input_fd], [], [], 0.05)[0]
            if self.hup:
                raise SSHDisconnected("sighup")
            if self.input_fd in readable:
                try:
                    value = os.read(self.input_fd, 1)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    raise SSHDisconnected("eio") from None
                if not value:
                    raise SSHDisconnected("eof")
                raise ProbeError("unexpected data on the SSH lifetime channel")
            if self.control in readable:
                return receive(self.control)
        raise ProbeError("SSH lifetime channel deadline expired")


class SSHSession(loader.LoadingSession):
    def __init__(self, writer, verifier, package, root, control, channel):
        super().__init__(writer, verifier, package, root, control)
        self.channel = channel

    def _launch_agent(self):
        super()._launch_agent(isolate_terminal=True)

    def stage(self, name):
        self.phase = self.last_seam = name
        life.send_json(self.control, {"kind": "stage", "seam": name,
                                     "secret_writes": self.loader.secret_writes,
                                     "loader_suspended_at_handoff": self.loader.handoff_suspended,
                                     "loader_returncode": self.loader.process.returncode,
                                     "loader_exit_observed": self.loader.exit_observed})
        require(self.channel.command() == {"command": "continue"}, "startup continuation was refused")


def worker(args):
    control = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    control.settimeout(TIMEOUT)
    control.connect(str(args.control_socket))
    channel = SSHChannel(control)
    previous = signal.signal(signal.SIGHUP, channel.on_hup)
    value = SSHSession(args.writer, args.verifier, args.package, args.root, control, channel)
    primary = cleanup = None
    outcome, cause = "failed", None
    try:
        life.send_json(control, {"kind": "worker", "pid": os.getpid(), "start": life.process_start(os.getpid()),
                                 "parent": os.getppid(), "stdin_is_tty": os.isatty(0)})
        require(channel.command() == {"command": "start"}, "SSH worker start barrier was refused")
        receipt = value.start(before_ready=lambda current: current.stage("before-readiness"))
        value.phase = value.last_seam = "ready"
        life.send_json(control, {"kind": "ready", "proof": receipt})
        require(channel.command() == {"command": "close"}, "ready close command was refused")
        value.lock()
        outcome = "ready_closed"
    except SSHDisconnected as error:
        outcome, cause = "ssh_disconnected", error.cause
    except BaseException as error:
        primary = error
    try:
        if value.state != "CLOSED":
            value.lock()
    except BaseException as error:
        cleanup = error
    try:
        if primary is not None or cleanup is not None:
            phase = value.loader.phase if value.phase == "loader-start" else value.phase
            if not value.master_transferred:
                control.sendall(b"E")
            life.send_json(control, {"kind": "worker-error", "diagnostic": loader.safe_error(primary or cleanup, phase),
                                     "cleanup_diagnostic": loader.safe_error(cleanup, phase) if primary is not None and cleanup is not None else None})
            return 1
        life.send_json(control, {"kind": "final", "outcome": outcome, "selected_seam": value.last_seam,
                                 "disconnect_observed": cause is not None, "disconnect_cause": cause,
                                 "readiness_published": value.ready_published,
                                 "close_acknowledged": value.close_acknowledged,
                                 "secret_writes": value.loader.secret_writes,
                                 "prompt_echo_disabled": value.loader.echo_disabled,
                                 "loader_suspended_at_handoff": value.loader.handoff_suspended,
                                 "loader_returncode": value.loader.process.returncode,
                                 "agent_returncode": value.agent.returncode,
                                 "resources_closed_after_exit": value.resources_closed_after_exit})
        return 0
    finally:
        signal.signal(signal.SIGHUP, previous)
        control.close()


def claim_worker(connection, metadata, server_pid, server_start, expected_tty):
    require(isinstance(metadata, dict) and set(metadata) == {"kind", "pid", "start", "parent", "stdin_is_tty"}
            and metadata["kind"] == "worker"
            and metadata["stdin_is_tty"] is expected_tty
            and all(type(metadata[name]) is int and metadata[name] > 0 for name in ("pid", "start", "parent")),
            "SSH worker metadata was refused")
    credentials = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    require(credentials == (metadata["pid"], os.geteuid(), os.getegid()), "SSH worker peer identity differs")
    descriptor = os.pidfd_open(metadata["pid"])
    try:
        require(life.process_start(metadata["pid"]) == metadata["start"], "SSH worker start identity changed")
        life.inspect_agent(metadata["pid"], metadata["start"], sys.executable, metadata["parent"])
        cursor = metadata["parent"]
        for _ in range(32):
            if cursor == server_pid:
                break
            require(cursor > 1, "SSH worker is outside the owned server tree")
            cursor = life.agent_credentials(cursor)["PPid"][0]
        require(cursor == server_pid and life.process_start(server_pid) == server_start,
                "SSH worker ancestry was refused")
        require(life.process_start(metadata["pid"]) == metadata["start"], "SSH worker changed during claim")
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def require_stage(value, expected):
    require(isinstance(value, dict) and value.get("kind") == "stage" and value.get("seam") == expected,
            "selected SSH startup seam was not reached")


def require_private_terminal_session(pid, supervisor_pid):
    require(os.getsid(pid) == pid and os.getpgid(pid) == pid
            and os.getsid(pid) != os.getsid(supervisor_pid),
            "native child shares the SSH terminal session")


def release_observed(bundle):
    for name in ("supervisor_pidfd", "agent_pidfd", "loader_pidfd"):
        require(bundle.get(name) is not None and life.pidfd_ready(bundle[name], 0),
                "observer could not confirm all SSH native exits")
    for name in ("connection", "control", "listener"):
        if bundle.get(name) is not None:
            bundle[name].close()
            bundle[name] = None
    for name in ("supervisor_pidfd", "agent_pidfd", "loader_pidfd", "pty_master"):
        if bundle.get(name) is not None:
            os.close(bundle[name])
            bundle[name] = None
    package = session.package_backend()
    package.cleanup_owned_tree(bundle["package"], bundle["package_identity"])
    package.cleanup_owned_tree(bundle["root"], bundle["root_identity"])


def finish_case(bundle, primary, phase):
    if primary is not None and CURRENT_REPORT is not None:
        CURRENT_REPORT["primary_failure"] = safe_error(primary, phase)
        if getattr(primary, "cleanup_diagnostic", None) is not None:
            CURRENT_REPORT["worker_cleanup_failure"] = dict(primary.cleanup_diagnostic)
    try:
        client = bundle.get("client")
        if client is not None:
            if client.poll() is None:
                client.kill()
            client.wait(timeout=life.DEATH_TIMEOUT)
            if client.stdin is not None:
                client.stdin.close()
        if primary is not None:
            # Fallback cleanup cannot turn a failed observation into a passing case.
            for name in ("supervisor_pidfd", "loader_pidfd", "agent_pidfd"):
                if bundle.get(name) is not None and not life.pidfd_ready(bundle[name], 0):
                    life.stop_agent(bundle[name])
        release_observed(bundle)
    except BaseException as error:
        UNCERTAIN_BUNDLES.append(bundle)
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(error, "observer-cleanup")
        if primary is None:
            raise


class SSHTransport:
    def __init__(self, root, *, diagnostic=False):
        self.root, self.server = root, None
        self.env = life.private_env(root)
        self.client_key = root / "transport-client"
        self.host_key = root / "transport-host"
        self.known_hosts = root / "known_hosts"
        self.diagnostic = diagnostic

    def start(self):
        import terminal_probe as terminal

        self.ssh = shutil.which("ssh", path=os.defpath)
        keygen = shutil.which("ssh-keygen", path=os.defpath)
        self.sshd = shutil.which("sshd", path=os.defpath + os.pathsep + "/usr/sbin:/sbin")
        require(self.ssh and keygen and self.sshd, "SSH fixture executables are unavailable")
        public = terminal.spawn_transport_key(keygen, self.client_key, self.env, "vivac transport fixture")
        host = terminal.spawn_transport_key(keygen, self.host_key, self.env, "vivac host fixture")
        authorized = self.root / "authorized_keys"
        native.private_write(authorized, public.read_bytes())
        host_parts = host.read_text(encoding="ascii").split()
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            self.port = reservation.getsockname()[1]
        native.private_write(self.known_hosts, ("[127.0.0.1]:" + str(self.port) + " "
                                              + " ".join(host_parts[:2]) + "\n").encode("ascii"))
        config = self.root / "sshd_config"
        lines = ["Port " + str(self.port), "ListenAddress 127.0.0.1", "HostKey " + str(self.host_key),
                 "PidFile " + str(self.root / "sshd.pid"), "AuthorizedKeysFile " + str(authorized),
                 "StrictModes yes", "PasswordAuthentication no", "KbdInteractiveAuthentication no",
                 "PermitRootLogin no", "AllowUsers nobody", "PubkeyAuthentication yes", "UsePAM no",
                 "AllowAgentForwarding no", "AllowTcpForwarding no", "X11Forwarding no", "PermitTunnel no",
                 "PermitTTY yes", "MaxAuthTries 2", "LogLevel ERROR"]
        native.private_write(config, ("\n".join(lines) + "\n").encode("ascii"))
        command = [self.sshd, "-D", "-e", "-f", str(config)]
        if self.diagnostic:
            log = self.root / "sshd-diagnostic"
            native.private_write(log, b"")
            command += ["-E", str(log), "-oLogLevel=DEBUG1"]
        self.server = subprocess.Popen(command, env=self.env,
                                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL, start_new_session=True)
        self.server_start = life.process_start(self.server.pid)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            require(self.server.poll() is None, "isolated SSH server exited before readiness")
            try:
                with socket.create_connection(("127.0.0.1", self.port), timeout=0.1):
                    return
            except OSError:
                time.sleep(0.025)
        raise ProbeError("isolated SSH server readiness timed out")

    def launch(self, mode, writer, verifier, package_path, root, control_path):
        remote = ["env", "VIVAC_OPENSSH_DISPOSABLE=1", "python3", "-B", str(Path(__file__).resolve()),
                  "--internal-worker", "--writer", str(writer), "--verifier", str(verifier),
                  "--package", str(package_path), "--root", str(root), "--control-socket", str(control_path)]
        arguments = [self.ssh, "-tt" if mode == "pty" else "-T", "-F", "/dev/null", "-p", str(self.port),
                     "-i", str(self.client_key), "-oIdentitiesOnly=yes", "-oBatchMode=yes",
                     "-oStrictHostKeyChecking=yes", "-oUserKnownHostsFile=" + str(self.known_hosts),
                     "-oGlobalKnownHostsFile=/dev/null", "-oForwardAgent=no", "-oClearAllForwardings=yes",
                     "-oControlMaster=no", "-oControlPath=none", "-oConnectTimeout=5",
                     "nobody@127.0.0.1", "exec " + shlex.join(remote)]
        return subprocess.Popen(arguments, env=self.env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, close_fds=True)

    def close(self):
        if self.server is not None:
            self.server.terminate()
            try:
                self.server.wait(timeout=life.DEATH_TIMEOUT)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait(timeout=life.DEATH_TIMEOUT)


def run_case(writer, verifier, store, root, transport, name):
    global CURRENT_STAGE
    CURRENT_STAGE = name
    mode, suffix = name.split("-", 1)
    normal = suffix == "normal-close"
    seam = "ready" if normal else suffix.removeprefix("disconnect-")
    package = session.package_backend()
    case_root = Path(tempfile.mkdtemp(prefix="sc-", dir=root))
    case_root.chmod(0o700)
    root_identity = package.validate_safe_tree_root(case_root)
    (case_root / "home").mkdir(mode=0o700)
    package_path = store / name
    package.create_positive(writer, package_path, life.private_env(case_root))
    package_identity = package.validate_safe_tree_root(package_path)
    manifest, _ = package.read_manifest(package_path)
    parsed = native.parse_key(package_path / "key.openssh")
    challenge = native.challenge(parsed["public_key"], manifest["device_id"], manifest["credential_id"])
    message = native.canonical(challenge)
    comment = "vivac.device-key.v1:" + manifest["device_id"] + ":" + manifest["credential_id"]
    bundle = {"root": case_root, "root_identity": root_identity, "package": package_path,
              "package_identity": package_identity}
    primary = None
    phase = "observer-listen"
    try:
        listener = bundle["listener"] = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        control_path = case_root / "ctl.sock"
        listener.bind(str(control_path))
        control_path.chmod(0o600)
        listener.listen(1)
        listener.settimeout(TIMEOUT)
        phase = "observer-launch-ssh"
        client = bundle["client"] = transport.launch(mode, writer, verifier, package_path, case_root, control_path)
        control, _ = listener.accept()
        bundle["control"] = control
        metadata = receive(control)
        bundle["supervisor_pidfd"] = claim_worker(control, metadata, transport.server.pid, transport.server_start, mode == "pty")
        supervisor_pid = metadata["pid"]
        life.send_json(control, {"command": "start"})
        phase = "observer-await-master"
        bundle["pty_master"] = receive_master(control)
        children = receive(control)
        require(children.get("kind") == "children" and children.get("echo_disabled") is True,
                "native child metadata was refused")
        for kind in ("agent", "loader"):
            bundle[kind + "_pidfd"] = loader.claim_child(children[kind + "_pid"], children[kind + "_start"], supervisor_pid)
            life.inspect_agent(children[kind + "_pid"], children[kind + "_start"],
                               shutil.which("ssh-agent" if kind == "agent" else "ssh-add", path=os.defpath), supervisor_pid)
            require_private_terminal_session(children[kind + "_pid"], supervisor_pid)
        connection = bundle["connection"] = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(TIMEOUT)
        connection.connect(children["socket"])
        life.exchange(connection, b"\x0b")
        pre_ready = False
        observed = None
        for current in ("prompt", "secret-handoff", "before-readiness"):
            phase = "observer-await-" + current
            stage = receive(control)
            require_stage(stage, current)
            observed = current
            if current == "secret-handoff":
                require(stage.get("secret_writes") == 1 and stage.get("loader_suspended_at_handoff") is True
                        and not life.pidfd_ready(bundle["loader_pidfd"], 0), "loader handoff was not a live suspended seam")
            if current == "before-readiness":
                require(stage.get("loader_returncode") == 0 and stage.get("loader_exit_observed") is True
                        and life.pidfd_ready(bundle["loader_pidfd"], 0), "native load did not complete before readiness")
                connection.close()
                connection, signature = life.open_signed_connection(Path(children["socket"]), parsed["public_blob"], comment, message)
                bundle["connection"] = connection
                native.verify(verifier, case_root, life.private_env(case_root), native.proof(challenge, signature), "ssh-pre-ready", True)
                pre_ready = True
            if current == seam:
                break
            life.send_json(control, {"command": "continue"})
        if seam == "ready":
            phase = "observer-await-ready"
            ready = receive(control)
            require(ready.get("kind") == "ready", "SSH worker did not publish readiness")
            native.verify(verifier, case_root, life.private_env(case_root), ready["proof"], "ssh-ready", True)
            observed = "ready"
        require(observed == seam, "selected SSH seam was not reached")
        alive = not life.pidfd_ready(bundle["loader_pidfd"], 0)
        require(alive is (seam != "ready"), "native loader liveness differs at the selected SSH seam")
        require(client.poll() is None and not life.pidfd_ready(bundle["supervisor_pidfd"], 0),
                "SSH transport ended before its selected trigger")
        phase = "observer-trigger"
        if normal:
            life.send_json(control, {"command": "close"})
        else:
            client.kill()
            client.wait(timeout=life.DEATH_TIMEOUT)
            require(client.returncode == -signal.SIGKILL, "SSH client kill was not observed")
        phase = "observer-await-final"
        final = receive(control)
        require(final.get("kind") == "final" and final.get("selected_seam") == seam
                and final.get("outcome") == ("ready_closed" if normal else "ssh_disconnected"),
                "SSH closure reached the wrong cause or seam")
        require(final.get("disconnect_observed") is (not normal)
                and (final.get("disconnect_cause") is None if normal else final.get("disconnect_cause") in ("eof", "eio", "sighup")),
                "SSH disconnect cause was not observed")
        require(final.get("readiness_published") is (seam == "ready")
                and final.get("close_acknowledged") is True and final.get("resources_closed_after_exit") is True,
                "SSH closure was not acknowledged after resource release")
        require(final.get("loader_returncode") == (0 if seam == "ready" else -signal.SIGKILL)
                and final.get("agent_returncode") == -signal.SIGKILL,
                "native exit status differs after SSH closure")
        require(final.get("secret_writes") == (0 if seam == "prompt" else 1)
                and final.get("prompt_echo_disabled") is True
                and final.get("loader_suspended_at_handoff") is (seam != "prompt"),
                "SSH case crossed the wrong secret handoff boundary")
        for kind in ("loader", "agent", "supervisor"):
            require(life.pidfd_ready(bundle[kind + "_pidfd"]), "SSH native process survived closure")
        client.wait(timeout=TIMEOUT)
        require(client.returncode == (0 if normal else -signal.SIGKILL), "SSH client exit status differs")
        require(life.request_after_death(connection, parsed["public_blob"], message) in ("transport_refused", "eof", "refused"),
                "retained connection signed after SSH closure")
        require(life.new_connection_after_death(Path(children["socket"]), parsed["public_blob"], message)
                in ("connect_refused", "transport_refused", "eof", "refused"), "new connection signed after SSH closure")
        return {"case": name, "ssh_mode": mode, "selected_seam": seam, "selected_seam_reached": True,
                "trigger": "close" if normal else "ssh-client-kill", "outcome": final["outcome"],
                "disconnect_observed": final["disconnect_observed"], "disconnect_cause": final["disconnect_cause"],
                "readiness_published": final["readiness_published"], "close_acknowledged": True,
                "secret_writes": final["secret_writes"], "prompt_echo_disabled": True, "pty_master_retained": True,
                "loader_alive_at_trigger": alive, "loader_suspended_at_handoff": final["loader_suspended_at_handoff"],
                "loader_exit_observed": True, "agent_exit_observed": True, "supervisor_exit_observed": True,
                "loader_returncode": final["loader_returncode"], "agent_returncode": final["agent_returncode"],
                "ssh_client_returncode": client.returncode, "resources_closed_after_exit": True,
                "retained_connection_refused": True, "new_connection_refused": True,
                "pre_ready_signature_verified": pre_ready, "processes_drained": True}
    except BaseException as error:
        primary = error
        raise
    finally:
        finish_case(bundle, primary, phase)


def run_probe(writer, verifier, store):
    global CURRENT_REPORT
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "ssh_disconnect_tested": False, "cases": [], "sentinel_survived": False}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="ssh_lifecycle_probe.py")
    session.require_store(store)
    life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
    package = session.package_backend()
    root = Path(tempfile.mkdtemp(prefix="vivac-ssh-", dir="/tmp"))
    root.chmod(0o700)
    identity = package.validate_safe_tree_root(root)
    transport = SSHTransport(root)
    try:
        (root / "home").mkdir(mode=0o700)
        ssh_agent, ssh_add, comment, parsed, challenge, message, env = life.create_fixture(root, writer, verifier)
        transport.start()
        sentinel_socket = root / "sentinel.sock"
        with life.owned_sentinel(ssh_agent, root, sentinel_socket) as (sentinel, pidfd):
            life.wait_socket(sentinel_socket, sentinel, sentinel.pid)
            code, _ = native.pty_run([ssh_add, "-k", str(root / "fixture-key")], life.private_env(root, sentinel_socket), [native.PASSWORD])
            require(code == 0, "sentinel credential load failed")
            for name in CASES:
                report["cases"].append(run_case(writer, verifier, store, root, transport, name))
                require(sentinel.poll() is None and not life.pidfd_ready(pidfd, 0), "sentinel was affected")
                signature = native.sign(sentinel_socket, parsed["public_blob"], message)
                native.verify(verifier, root, env, native.proof(challenge, signature), "sentinel-" + name, True)
            report["sentinel_survived"] = True
    finally:
        transport.close()
        if not UNCERTAIN_BUNDLES:
            package.cleanup_owned_tree(root, identity)
    report["ssh_disconnect_tested"] = True
    report["status"] = "observed"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("writer", "verifier", "store-root", "report", "package", "root", "control-socket"):
        parser.add_argument("--" + name, type=Path)
    parser.add_argument("--internal-worker", action="store_true")
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "process is missing no_new_privs")
        if args.internal_worker:
            return worker(args)
        require(args.writer is not None and args.verifier is not None and args.store_root is not None
                and args.report is not None, "required experiment arguments are absent")
        session.require_report_directory(args.report)
        result = run_probe(args.writer.resolve(strict=True), args.verifier.resolve(strict=True), args.store_root)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("SSH lifetime observations recorded; custody_accredited=false")
        return 0
    except Exception as error:
        if args.report is not None:
            try:
                session.require_report_directory(args.report)
                result = dict(CURRENT_REPORT or {})
                result.update({"version": 1, "status": "incomplete", "custody_accredited": False,
                               "stage": CURRENT_STAGE, "failure": safe_error(error, CURRENT_STAGE)})
                life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
            except (OSError, session.ProbeError):
                pass
        print("SSH lifetime probe failed; no result is accredited.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
