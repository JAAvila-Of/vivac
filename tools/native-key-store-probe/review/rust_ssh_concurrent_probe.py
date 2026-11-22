#!/usr/bin/env python3
"""Disposable SSH transport loss during shared Rust signing work."""
import argparse
import array
import json
import os
from pathlib import Path
import select
import shlex
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile

import heap_probe as heap
import lifecycle_probe as life
import openssh_probe as native
import provider_probe as providers
import rust_concurrent_probe as concurrent
import rust_launcher_probe as spawn
import rust_loader_probe as loader
import rust_ssh_loader_probe as ssh_loader
import session_probe as session
import ssh_lifecycle_probe as ssh

SUFFIXES = ("normal-close", "disconnect-before-native-send", "disconnect-native-response-pending",
            "disconnect-after-native-reply", "disconnect-before-publication", "disconnect-published")
CASES = tuple(mode + "-" + suffix for mode in ("pty", "pipe") for suffix in SUFFIXES)
FINAL_FIELDS = concurrent.FINAL_FIELDS | {"ssh_disconnect_observed", "ssh_disconnect_cause"}
EXTERNAL_FIELDS = {"ssh_mode", "ssh_stdin_is_tty", "ssh_client_exit_code", "supervisor_exit_observed",
                   "supervisor_exec_verified", "readiness_proof_verified", "published_proof_verified",
                   "lease_invalidated_observed", "pty_master_retained_through_exit", "pty_slave_private",
                   "sealed_snapshot", "retained_connection_refused", "new_connection_refused"}
CLOSING_FIELDS = {"stage", "case", "seam", "active", "close_ack", "lock_callers",
                  "ssh_disconnect_observed", "ssh_disconnect_cause"}
UNCERTAIN = []
PHASE = "preflight"
ProbeError = loader.ProbeError
require = loader.require
fcntl = loader.fcntl


def selected_seam(suffix):
    require(suffix in SUFFIXES, "SSH concurrent case differs")
    return "published" if suffix in ("normal-close", "disconnect-published") else suffix.removeprefix("disconnect-")


def validate_closing(value, suffix):
    require(selected_seam(suffix) != "published" and isinstance(value, dict) and set(value) == CLOSING_FIELDS
            and value["stage"] == "closing-paused" and value["case"] == suffix
            and value["seam"] == selected_seam(suffix) and value["active"] is True
            and value["close_ack"] is False and type(value["lock_callers"]) is int
            and value["lock_callers"] == 1 and value["ssh_disconnect_observed"] is True
            and value["ssh_disconnect_cause"] in ("eof", "eio", "sighup"),
            "SSH closing overlap or transport cause differs")


def parse_final(payload, suffix, identity):
    require(0 < len(payload) <= 8192, "SSH concurrent final size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    require(isinstance(value, dict) and set(value) == FINAL_FIELDS, "SSH concurrent final fields differ")
    seam = selected_seam(suffix)
    published = seam == "published"
    disconnected = suffix != "normal-close"
    for field in ("agent_pid", "loader_pid", "creator_tid"):
        require(type(value[field]) is int and value[field] == identity[field], "SSH native identity changed")
    require(type(value["version"]) is int and value["version"] == 1 and value["case"] == suffix
            and value["status"] == "observed" and value["state"] == "CLOSED"
            and value["selected_seam"] == seam, "SSH concurrent final identity differs")
    counts = {"published": (1, 1, 1, 1), "before-native-send": (0, 0, 0, 0),
              "native-response-pending": (1, 0, 0, 0), "after-native-reply": (1, 1, 0, 0),
              "before-publication": (1, 1, 1, 0)}[seam]
    expected = dict(zip(("native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published"), counts))
    expected.update(lock_callers=1, lock_acknowledgments=1, workers_joined=1, agent_exit_signal=9, loader_exit_code=0)
    require(all(type(value[field]) is int and value[field] == expected_value for field, expected_value in expected.items()),
            "SSH concurrent count or native exit differs")
    require(value["agent_exit_code"] is None and value["loader_exit_signal"] is None, "SSH native exit cause differs")
    for field in ("pressure_iterations", "closed_refused", "busy_refused"):
        minimum = int(field != "busy_refused" or published)
        require(type(value[field]) is int and value[field] >= minimum, "SSH concurrent control counter differs")
    booleans = {"active": False, "close_acknowledged": True, "resources_closed_after_drain": True,
                "closing_observed_while_paused": not published, "request_late_refused": not published,
                "drain_wait_observed": not published, "custody_accredited": False,
                "full_session_accredited": False, "concurrent_lock_tested": True,
                "ssh_disconnect_observed": disconnected}
    require(all(value[field] is expected_value for field, expected_value in booleans.items()),
            "SSH concurrent closure evidence differs")
    require(value["request_outcome"] == ("published" if published else "cancelled_by_ssh"),
            "SSH concurrent request outcome differs")
    cause = value["ssh_disconnect_cause"]
    require(cause in ("eof", "eio", "sighup") if disconnected else cause is None,
            "SSH concurrent transport cause differs")
    return value


def validate_transfer(marker, descriptors, truncated, valid):
    require(marker == b"I" and len(descriptors) == 6 and not truncated and valid,
            "SSH initial capability transfer differs")


def receive_initial(control):
    descriptors = []
    try:
        width = array.array("i").itemsize
        marker, ancillary, flags, _ = control.recvmsg(2, socket.CMSG_SPACE(7 * width), socket.MSG_CMSG_CLOEXEC)
        valid = True
        for level, kind, data in ancillary:
            valid &= level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS and len(data) % width == 0
            if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                received = array.array("i")
                received.frombytes(data[:len(data) - len(data) % width])
                descriptors.extend(received)
        validate_transfer(marker, descriptors, flags & (socket.MSG_CTRUNC | socket.MSG_TRUNC), valid)
        require(all(fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC for fd in descriptors),
                "SSH initial capability inheritance differs")
        result, descriptors = descriptors, []
        return result
    finally:
        for descriptor in descriptors:
            os.close(descriptor)


def worker(args):
    control = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    control.settimeout(25)
    control.connect(str(args.control_socket))
    credentials = struct.unpack("3i", control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    require(credentials[1:] == (os.geteuid(), os.getegid()), "SSH bootstrap observer credentials differ")
    metadata = {"kind": "worker", "pid": os.getpid(), "start": life.process_start(os.getpid()),
                "parent": os.getppid(), "stdin_is_tty": os.isatty(0)}
    control.sendall(json.dumps(metadata, separators=(",", ":")).encode("ascii"))
    descriptors = receive_initial(control)
    arguments = [str(args.launcher_helper), "--concurrent-ssh-case", args.case, "--ssh-lifetime", "stdin"]
    for name, descriptor in zip(("agent", "loader", "cipher", "secret", "fixture"), descriptors[:5]):
        arguments.extend(("--" + name + "-fd", str(descriptor)))
    arguments.extend(("--observer-fd", str(control.fileno()), "--lease-dir-fd", str(descriptors[5]),
                      "--socket", str(args.socket)))
    for descriptor in (*descriptors, control.fileno()):
        os.set_inheritable(descriptor, True)
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    os.execve(args.launcher_helper, arguments, life.private_env(args.root))


class SSHTransport(ssh.SSHTransport):
    def launch(self, mode, helper, socket_path, root, control_path, suffix):
        remote = ["env", "VIVAC_OPENSSH_DISPOSABLE=1", "python3", "-B", str(Path(__file__).resolve()),
                  "--internal-worker", "--launcher-helper", str(helper), "--socket", str(socket_path),
                  "--root", str(root), "--control-socket", str(control_path), "--case", suffix]
        arguments = [self.ssh, "-tt" if mode == "pty" else "-T", "-F", "/dev/null", "-p", str(self.port),
                     "-i", str(self.client_key), "-oIdentitiesOnly=yes", "-oBatchMode=yes",
                     "-oStrictHostKeyChecking=yes", "-oUserKnownHostsFile=" + str(self.known_hosts),
                     "-oGlobalKnownHostsFile=/dev/null", "-oForwardAgent=no", "-oClearAllForwardings=yes",
                     "-oControlMaster=no", "-oControlPath=none", "-oConnectTimeout=5",
                     "nobody@127.0.0.1", "exec " + shlex.join(remote)]
        return subprocess.Popen(arguments, env=self.env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, close_fds=True)


def run_case(args, provider, root, transport, name, state):
    global PHASE
    mode, suffix = name.split("-", 1)
    selected = selected_seam(suffix)
    published = selected == "published"
    disconnected = suffix != "normal-close"
    PHASE = "fixture"
    case_root = root / name
    case_root.mkdir(mode=0o700)
    (case_root / "home").mkdir(mode=0o700)
    environment = life.private_env(case_root)
    _, snapshot, parsed, challenge, comment, secret = loader.create_fixture(args.writer, case_root, environment)
    package = session.package_backend()
    cipher = metadata_fd = secret_read = master = supervisor_fd = None
    listener = channel = connection = client = None
    pidfds = {}
    observed = False
    state["uncertain"] = True
    try:
        cipher, _ = package.make_sealed_key_memfd(snapshot["key_bytes"])
        require(fcntl.fcntl(cipher, fcntl.F_GET_SEALS) & loader.SEALS == loader.SEALS, "SSH ciphertext seals differ")
        metadata_fd = loader.sealed_metadata({"public_blob": parsed["public_blob"].hex(), "comment": comment,
                         "challenge": native.canonical(challenge).hex(), "public_key": parsed["public_key"].hex()})
        secret_read, secret_write = os.pipe2(os.O_CLOEXEC)
        try:
            require(os.write(secret_write, secret) == len(secret), "SSH secret pipe write was incomplete")
        finally:
            os.close(secret_write)
        loader.wipe(secret)
        control_path, socket_path = case_root / "control.sock", case_root / "agent.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        listener.bind(str(control_path))
        control_path.chmod(0o600)
        listener.listen(1)
        listener.settimeout(25)
        PHASE = "bootstrap"
        client = transport.launch(mode, args.launcher_helper, socket_path, case_root, control_path, suffix)
        channel, _ = listener.accept()
        channel.settimeout(25)
        metadata = json.loads(channel.recv(8193).decode("ascii"), object_pairs_hook=heap.unique_object)
        supervisor_fd = ssh.claim_worker(channel, metadata, transport.server.pid, transport.server_start, mode == "pty")
        initial = array.array("i", (provider.files["ssh-agent"].fd, provider.files["ssh-add"].fd,
                                   cipher, secret_read, metadata_fd, snapshot["directory_fd"]))
        require(channel.sendmsg([b"I"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, initial)]) == 1,
                "SSH bootstrap transfer was incomplete")
        os.close(secret_read)
        secret_read = None
        PHASE = "startup"
        master = loader.receive_master(channel)
        identity = loader.parse_stage(channel.recv(8193), "prompt")
        ssh_loader.inspect_supervisor(metadata, args.launcher_helper, mode == "pty", transport)
        require(identity["creator_tid"] == metadata["pid"], "SSH creator is not the retained main thread")
        for component in ("agent", "loader"):
            pid = identity[component + "_pid"]
            pidfds[component] = os.pidfd_open(pid)
            life.inspect_agent(pid, life.process_start(pid), str(provider.files[
                "ssh-agent" if component == "agent" else "ssh-add"].path), metadata["pid"])
        slave = Path("/proc", str(identity["loader_pid"]), "fd/0").stat()
        require(stat.S_ISCHR(slave.st_mode) and slave.st_uid == os.geteuid()
                and stat.S_IMODE(slave.st_mode) == 0o600, "SSH native PTY ownership differs")
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"H")
        concurrent.bound_stage(channel.recv(8193), "handoff", identity)
        status = Path("/proc", str(identity["loader_pid"]), "status").read_text(encoding="ascii")
        require(any(line.startswith("State:") and line.split()[1] == "T" for line in status.splitlines()),
                "SSH native loader stop was not observed")
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"L")
        stage = concurrent.bound_stage(channel.recv(8193), "before-ready", identity)
        native.verify(args.verifier, case_root, environment, native.proof(challenge, bytes.fromhex(stage["signature"])),
                      "readiness-proof", True)
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"R")
        concurrent.bound_stage(channel.recv(8193), "ready", identity)
        ready = concurrent.packet(channel, ("stage", "agent_pid", "loader_pid", "creator_tid"), "concurrent-ready")
        require(all(ready[field] == identity[field] for field in ("agent_pid", "loader_pid", "creator_tid")),
                "SSH ready ownership changed")
        connection, signature = life.open_signed_connection(socket_path, parsed["public_blob"], comment, native.canonical(challenge))
        native.verify(args.verifier, case_root, environment, native.proof(challenge, signature), "retained-proof", True)
        PHASE = "race"
        channel.sendall(b"S")
        race = concurrent.packet(channel, ("stage", "case", "seam", "agent_pid", "loader_pid", "creator_tid", "active"), "race-paused")
        work_seam = "before-native-send" if published else selected
        require(race["case"] == suffix and race["seam"] == work_seam and race["active"] is True
                and all(race[field] == identity[field] for field in ("agent_pid", "loader_pid", "creator_tid")),
                "SSH selected signing seam differs")
        require(client.poll() is None and not life.pidfd_ready(supervisor_fd, 0)
                and not life.pidfd_ready(pidfds["agent"], 0), "SSH transport or agent ended before its trigger")
        package.revalidate_retained_package(snapshot)
        if published:
            channel.sendall(b"G")
            proof = concurrent.packet(channel, ("stage", "signature"), "published-proof")
            require(isinstance(proof["signature"], str) and len(proof["signature"]) == 128
                    and all(c in "0123456789abcdef" for c in proof["signature"]), "SSH published signature encoding differs")
            native.verify(args.verifier, case_root, environment, native.proof(challenge, bytes.fromhex(proof["signature"])),
                          "published-proof", True)
            require(client.poll() is None, "SSH client ended before publication verification")
            channel.sendall(b"V")
            held = concurrent.packet(channel, ("stage", "case", "seam", "active", "workers_joined", "proofs_published"), "published-paused")
            require(held["case"] == suffix and held["seam"] == "published" and held["active"] is False
                    and type(held["workers_joined"]) is int and held["workers_joined"] == 1
                    and type(held["proofs_published"]) is int and held["proofs_published"] == 1,
                    "SSH publication boundary differs")
        PHASE = "disconnect" if disconnected else "close"
        if disconnected:
            require(client.poll() is None and not life.pidfd_ready(supervisor_fd, 0), "SSH transport ended before trigger")
            client.kill()
            require(client.wait(timeout=5) == -signal.SIGKILL, "SSH client SIGKILL was not observed")
        else:
            channel.sendall(b"C")
        closing_cause = None
        if not published:
            closing = concurrent.packet(channel, CLOSING_FIELDS, "closing-paused")
            validate_closing(closing, suffix)
            closing_cause = closing["ssh_disconnect_cause"]
            require(all(life.pidfd_ready(fd, 0) for fd in pidfds.values()), "SSH closing did not observe native stop")
            require(not select.select([channel], [], [], 0)[0], "SSH closure was published before worker release")
            channel.sendall(b"G")
        PHASE = "closure"
        final = parse_final(channel.recv(8193), suffix, identity)
        require(closing_cause is None or final["ssh_disconnect_cause"] == closing_cause,
                "SSH disconnect cause changed after drain")
        require(life.pidfd_ready(supervisor_fd, 5), "SSH Rust supervisor exit was not observed")
        if not disconnected:
            require(client.wait(timeout=5) == 0, "normal SSH client did not complete")
        require(all(life.pidfd_ready(fd, 0) for fd in pidfds.values()), "SSH native exits remain unobserved")
        require(os.isatty(master) and spawn.connection_refused(connection, socket_path), "SSH post-close capability remains usable")
        package.revalidate_retained_package(snapshot)
        provider.revalidate()
        observed = True
        state["uncertain"] = False
        return dict(final, case=name, ssh_mode=mode, ssh_stdin_is_tty=mode == "pty",
                    ssh_client_exit_code=client.returncode, supervisor_exit_observed=True, supervisor_exec_verified=True,
                    readiness_proof_verified=True, published_proof_verified=published, lease_invalidated_observed=False,
                    pty_master_retained_through_exit=True, pty_slave_private=True, sealed_snapshot=True,
                    retained_connection_refused=True, new_connection_refused=True)
    finally:
        loader.wipe(secret)
        if not observed:
            state["uncertain"] = True
            UNCERTAIN.append((snapshot, cipher, metadata_fd, secret_read, master, supervisor_fd,
                              listener, channel, connection, client, pidfds))
        else:
            for descriptor in (*pidfds.values(), cipher, metadata_fd, secret_read, master, supervisor_fd):
                if descriptor is not None:
                    os.close(descriptor)
            package.close_retained_package(snapshot)
            for value in (listener, channel, connection, client.stdin):
                if value is not None:
                    value.close()


def main():
    global PHASE
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report", "root", "socket", "control-socket"):
        parser.add_argument("--" + name, type=Path)
    parser.add_argument("--case", choices=SUFFIXES)
    parser.add_argument("--internal-worker", action="store_true")
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-ssh-concurrent", "status": "incomplete", "cases": [],
              "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
              "ssh_disconnect_tested": False, "concurrent_lock_tested": False, "sentinel_survived": False}
    provider = sentinel = snapshot = root = transport = secret = None
    state = {"uncertain": False}
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable Linux is required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "SSH observer no_new_privs differs")
        if args.internal_worker:
            require(args.case in SUFFIXES and all(value is not None and value.is_absolute() for value in
                    (args.launcher_helper, args.root, args.socket, args.control_socket)), "SSH bootstrap paths differ")
            return worker(args)
        require(all(value is not None for value in (args.writer, args.verifier, args.launcher_helper, args.store_root, args.report)),
                "SSH observer arguments are absent")
        report["resource_limits"] = life.require_resources(expected_script="rust_ssh_concurrent_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        args.launcher_helper = args.launcher_helper.resolve(strict=True)
        root = Path(tempfile.mkdtemp(prefix="rust-ssh-gate-", dir=args.store_root)).resolve()
        root.chmod(0o700)
        package = session.package_backend()
        identity = package.validate_safe_tree_root(root)
        (root / "home").mkdir(mode=0o700)
        environment = life.private_env(root)
        provider = providers.Provider()
        provider.open()
        version, architecture = provider.package(environment)
        report["provenance"] = {"package_version": version, "package_arch": architecture, "components": provider.components()}
        report["helper_sha256"] = spawn.file_hash(args.launcher_helper)
        report["observer_sha256"] = spawn.file_hash(Path(__file__))
        PHASE = "transport"
        transport = SSHTransport(root)
        transport.start()
        PHASE = "sentinel"
        sentinel_root = root / "sentinel"
        sentinel_root.mkdir(mode=0o700)
        (sentinel_root / "home").mkdir(mode=0o700)
        path, snapshot, parsed, challenge, _, secret = loader.create_fixture(args.writer, sentinel_root, environment)
        sentinel = providers.OwnedAgent(provider, root, "sentinel")
        sentinel.start()
        code, _ = provider.execute("ssh-add", ["-k", str(path / "key.openssh")],
                                   life.private_env(root, sentinel.path), responses=[secret])
        require(code == 0, "SSH independent sentinel load failed")
        loader.wipe(secret)
        secret = None
        for name in CASES:
            pending = {"case": name, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args, provider, root, transport, name, state))
            PHASE = "sentinel"
            require(sentinel.process.poll() is None and not life.pidfd_ready(sentinel.pidfd, 0), "SSH independent sentinel was affected")
            signature = native.sign(sentinel.path, parsed["public_blob"], native.canonical(challenge))
            native.verify(args.verifier, root, environment, native.proof(challenge, signature), "sentinel-" + name, True)
        PHASE = "cleanup"
        sentinel.stop()
        sentinel.close()
        sentinel = None
        package.close_retained_package(snapshot)
        snapshot = None
        transport.close()
        transport = None
        provider.close()
        provider = None
        package.cleanup_owned_tree(root, identity)
        root = None
        report.update(status="observed", ssh_disconnect_tested=True, concurrent_lock_tested=True,
                      sentinel_survived=True, resources_closed_after_exit=True)
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        loader.wipe(secret)
        report.update(status="incomplete", failure={"phase": PHASE, "kind": type(error).__name__})
        UNCERTAIN.append((provider, sentinel, snapshot, root, transport))
        if args.report is not None:
            try:
                session.require_report_directory(args.report)
                life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
            except Exception:
                pass
        print("Rust SSH concurrent experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
