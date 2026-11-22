#!/usr/bin/env python3
"""Disposable Rust loader observations across isolated SSH transport loss."""
import argparse
import array
import json
import os
from pathlib import Path
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
import rust_launcher_probe as spawn_probe
import rust_loader_probe as loader
import session_probe as session
import ssh_lifecycle_probe as ssh

fcntl = loader.fcntl
CASES = tuple(mode + "-" + suffix for mode in ("pty", "pipe") for suffix in
              ("normal-close", "disconnect-prompt", "disconnect-handoff", "disconnect-before-ready", "disconnect-ready"))
UNCERTAIN = []
PHASE = "preflight"
ProbeError = loader.ProbeError
require = loader.require


def parse_final(payload, disconnected):
    require(0 < len(payload) <= 8192, "SSH final packet size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    require(isinstance(value, dict) and {"ssh_disconnect_observed", "ssh_disconnect_cause"} <= set(value),
            "SSH final fields are absent")
    require(value["ssh_disconnect_observed"] is disconnected, "SSH disconnect observation differs")
    require(value["ssh_disconnect_cause"] in ("eof", "eio", "sighup") if disconnected
            else value["ssh_disconnect_cause"] is None, "SSH disconnect cause differs")
    baseline = {key: item for key, item in value.items() if key not in
                ("ssh_disconnect_observed", "ssh_disconnect_cause")}
    loader.parse_final(json.dumps(baseline).encode("ascii"), "normal")
    return value


def validate_transfer(marker, descriptors, truncated, valid):
    require(marker == b"I" and len(descriptors) == 5 and not truncated and valid,
            "initial descriptor transfer differs")


def parse_bound_stage(payload, expected, identity):
    value = loader.parse_stage(payload, expected)
    require(all(value[key] == identity[key] for key in ("agent_pid", "loader_pid", "creator_tid")),
            "native stage identity changed")
    return value


def receive_initial(control):
    descriptors = []
    try:
        width = array.array("i").itemsize
        marker, ancillary, flags, _ = control.recvmsg(2, socket.CMSG_SPACE(6 * width), socket.MSG_CMSG_CLOEXEC)
        valid = True
        for level, kind, data in ancillary:
            valid &= level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS and len(data) % width == 0
            if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                received = array.array("i")
                received.frombytes(data[:len(data) - len(data) % width])
                descriptors.extend(received)
        validate_transfer(marker, descriptors, flags & (socket.MSG_CTRUNC | socket.MSG_TRUNC), valid)
        require(all(fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC for fd in descriptors),
                "initial descriptor close-on-exec differs")
        result, descriptors = descriptors, []
        return result
    finally:
        for descriptor in descriptors:
            os.close(descriptor)


def worker(args):
    control = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    control.settimeout(20)
    control.connect(str(args.control_socket))
    credentials = struct.unpack("3i", control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    require(credentials[1:] == (os.geteuid(), os.getegid()), "bootstrap observer credentials differ")
    metadata = {"kind": "worker", "pid": os.getpid(), "start": life.process_start(os.getpid()),
                "parent": os.getppid(), "stdin_is_tty": os.isatty(0)}
    control.sendall(json.dumps(metadata, separators=(",", ":")).encode("ascii"))
    descriptors = receive_initial(control)
    arguments = [str(args.launcher_helper), "--loader-case", "normal", "--ssh-lifetime", "stdin"]
    for name, descriptor in zip(("agent", "loader", "cipher", "secret", "fixture"), descriptors):
        arguments.extend(("--" + name + "-fd", str(descriptor)))
    arguments.extend(("--observer-fd", str(control.fileno()), "--socket", str(args.socket)))
    for descriptor in (*descriptors, control.fileno()):
        os.set_inheritable(descriptor, True)
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    os.execve(args.launcher_helper, arguments, life.private_env(args.root))


class SSHTransport(ssh.SSHTransport):
    def launch(self, mode, helper, socket_path, root, control_path):
        remote = ["env", "VIVAC_OPENSSH_DISPOSABLE=1", "python3", "-B", str(Path(__file__).resolve()),
                  "--internal-worker", "--launcher-helper", str(helper), "--socket", str(socket_path),
                  "--root", str(root), "--control-socket", str(control_path)]
        arguments = [self.ssh, "-tt" if mode == "pty" else "-T", "-F", "/dev/null", "-p", str(self.port),
                     "-i", str(self.client_key), "-oIdentitiesOnly=yes", "-oBatchMode=yes",
                     "-oStrictHostKeyChecking=yes", "-oUserKnownHostsFile=" + str(self.known_hosts),
                     "-oGlobalKnownHostsFile=/dev/null", "-oForwardAgent=no", "-oClearAllForwardings=yes",
                     "-oControlMaster=no", "-oControlPath=none", "-oConnectTimeout=5",
                     "nobody@127.0.0.1", "exec " + shlex.join(remote)]
        return subprocess.Popen(arguments, env=self.env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, close_fds=True)


def inspect_supervisor(metadata, helper, expected_tty, transport):
    pid = metadata["pid"]
    require(life.process_start(pid) == metadata["start"], "supervisor start identity changed")
    observed = Path("/proc", str(pid), "exe").stat()
    expected = helper.stat()
    require((observed.st_dev, observed.st_ino) == (expected.st_dev, expected.st_ino),
            "supervisor executable inode differs")
    life.inspect_agent(pid, metadata["start"], str(helper), metadata["parent"])
    cursor = metadata["parent"]
    for _ in range(32):
        if cursor == transport.server.pid:
            break
        require(cursor > 1, "supervisor left the owned SSH server ancestry")
        cursor = life.agent_credentials(cursor)["PPid"][0]
    require(cursor == transport.server.pid and life.process_start(cursor) == transport.server_start,
            "supervisor SSH server identity changed")
    descriptor = os.open(Path("/proc", str(pid), "fd/0"), os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC | os.O_NOCTTY)
    try:
        require(os.isatty(descriptor) is expected_tty, "supervisor SSH stdin kind differs")
    finally:
        os.close(descriptor)
    require(life.process_start(pid) == metadata["start"], "supervisor changed during exec verification")


def run_case(helper, writer, verifier, provider, root, transport, name, state):
    global PHASE
    mode, suffix = name.split("-", 1)
    disconnected = suffix != "normal-close"
    selected = suffix.removeprefix("disconnect-") if disconnected else "ready"
    case_root = root / name
    case_root.mkdir(mode=0o700)
    (case_root / "home").mkdir(mode=0o700)
    environment = life.private_env(case_root)
    _, snapshot, parsed, challenge, comment, secret = loader.create_fixture(writer, case_root, environment)
    package = session.package_backend()
    cipher = metadata_fd = secret_read = master = supervisor_fd = None
    listener = channel = connection = client = None
    pidfds = {}
    observed = False
    state["uncertain"] = True
    try:
        PHASE = "fixture"
        cipher, _ = package.make_sealed_key_memfd(snapshot["key_bytes"])
        require(fcntl.fcntl(cipher, fcntl.F_GET_SEALS) & loader.SEALS == loader.SEALS, "cipher seals differ")
        metadata_fd = loader.sealed_metadata({"public_blob": parsed["public_blob"].hex(), "comment": comment,
                        "challenge": native.canonical(challenge).hex(), "public_key": parsed["public_key"].hex()})
        secret_read, secret_write = os.pipe2(os.O_CLOEXEC)
        try:
            require(os.write(secret_write, secret) == len(secret), "secret pipe write was incomplete")
        finally:
            os.close(secret_write)
        loader.wipe(secret)
        control_path, socket_path = case_root / "control.sock", case_root / "agent.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        listener.bind(str(control_path))
        control_path.chmod(0o600)
        listener.listen(1)
        listener.settimeout(20)
        PHASE = "bootstrap"
        client = transport.launch(mode, helper, socket_path, case_root, control_path)
        channel, _ = listener.accept()
        channel.settimeout(20)
        metadata = json.loads(channel.recv(8193).decode("ascii"), object_pairs_hook=heap.unique_object)
        supervisor_fd = ssh.claim_worker(channel, metadata, transport.server.pid, transport.server_start, mode == "pty")
        initial = array.array("i", (provider.files["ssh-agent"].fd, provider.files["ssh-add"].fd,
                                     cipher, secret_read, metadata_fd))
        require(channel.sendmsg([b"I"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, initial)]) == 1,
                "bootstrap transfer was incomplete")
        os.close(secret_read)
        secret_read = None
        PHASE = "prompt"
        master = loader.receive_master(channel)
        frame = loader.parse_stage(channel.recv(8193), "prompt")
        identity = frame
        inspect_supervisor(metadata, helper, mode == "pty", transport)
        pids = {"agent": frame["agent_pid"], "loader": frame["loader_pid"]}
        for component, pid in pids.items():
            pidfds[component] = os.pidfd_open(pid)
            life.inspect_agent(pid, life.process_start(pid), str(provider.files[
                "ssh-agent" if component == "agent" else "ssh-add"].path), metadata["pid"])
        slave = Path("/proc", str(pids["loader"]), "fd/0").stat()
        require(stat.S_ISCHR(slave.st_mode) and slave.st_uid == os.geteuid()
                and stat.S_IMODE(slave.st_mode) == 0o600, "private loader slave mode differs")
        connection = socket.socket(socket.AF_UNIX)
        connection.settimeout(3)
        connection.connect(str(socket_path))
        stopped = proof_verified = False
        if selected != "prompt":
            PHASE = "handoff"
            package.revalidate_retained_package(snapshot)
            channel.sendall(b"H")
            frame = parse_bound_stage(channel.recv(8193), "handoff", identity)
            status = Path("/proc", str(pids["loader"]), "status").read_text(encoding="ascii")
            stopped = any(line.startswith("State:") and line.split()[1] == "T" for line in status.splitlines())
            require(stopped and frame["loader_stopped"], "loader handoff stop was not observed")
        if selected in ("before-ready", "ready"):
            PHASE = "before-ready"
            package.revalidate_retained_package(snapshot)
            channel.sendall(b"L")
            frame = parse_bound_stage(channel.recv(8193), "before-ready", identity)
            native.verify(verifier, case_root, environment, native.proof(challenge, bytes.fromhex(frame["signature"])),
                          "independent-proof", True)
            proof_verified = True
        if selected == "ready":
            PHASE = "ready"
            package.revalidate_retained_package(snapshot)
            channel.sendall(b"R")
            frame = parse_bound_stage(channel.recv(8193), "ready", identity)
            require(frame["ready_published"], "readiness was not published")
        PHASE = "disconnect" if disconnected else "close"
        if disconnected:
            client.kill()
            require(client.wait(timeout=5) == -signal.SIGKILL, "SSH client did not exit by SIGKILL")
        else:
            channel.sendall(b"C")
        frame = parse_bound_stage(channel.recv(8193), "closed", identity)
        require(frame["close_ack"] and frame["resource_release_after_exit"] and not frame["loader_failed"],
                "Rust close acknowledgement differs")
        require(frame["ready_published"] is (selected == "ready") and frame["secret_writes"] == int(stopped)
                and frame["agent_exit_signal"] == signal.SIGKILL and frame["agent_exit_code"] is None,
                "Rust closed native metadata differs")
        require((frame["loader_exit_code"] == 0 and frame["loader_exit_signal"] is None) if proof_verified else
                (frame["loader_exit_signal"] == signal.SIGKILL and frame["loader_exit_code"] is None),
                "Rust loader exit differs")
        final = parse_final(channel.recv(8193), disconnected)
        require(all(final[field] == frame[field] for field in loader.FIELDS), "final stage changed closure metadata")
        require(life.pidfd_ready(supervisor_fd, 5), "Rust supervisor exit was not observed")
        if not disconnected:
            require(client.wait(timeout=5) == 0, "normal SSH client did not complete")
        require(all(life.pidfd_ready(fd, 0) for fd in pidfds.values()), "native exits remain unobserved")
        require(os.isatty(master), "retained private PTY master was lost")
        require(spawn_probe.connection_refused(connection, socket_path), "closed agent remains reachable")
        package.revalidate_retained_package(snapshot)
        provider.revalidate()
        observed = True
        state["uncertain"] = False
        return {"case": name, "status": "observed", "selected_seam": selected, "secret_writes": int(stopped),
                "ready_published": selected == "ready", "independent_proof_verified": proof_verified,
                "agent_exit_observed": True, "loader_exit_observed": True, "pty_master_retained_through_exit": True,
                "loader_stopped_observed": stopped, "close_ack": True, "owner_sigkill_observed": False,
                "loader_failed_observed": False, "retained_connection_refused": True, "new_connection_refused": True,
                "lease_revalidated": True, "sealed_snapshot": True, "pty_slave_private": True,
                "ssh_mode": mode, "ssh_stdin_is_tty": mode == "pty", "ssh_disconnect_observed": disconnected,
                "ssh_disconnect_cause": final["ssh_disconnect_cause"], "ssh_client_exit_code": client.returncode,
                "supervisor_exit_observed": True, "supervisor_exec_verified": True}
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
    parser.add_argument("--internal-worker", action="store_true")
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-ssh-loader", "status": "incomplete", "cases": [],
              "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
              "ssh_disconnect_tested": False, "loader_lifecycle_tested": False, "sentinel_survived": False}
    provider = sentinel = snapshot = root = transport = secret = None
    state = {"uncertain": False}
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable Linux is required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        if args.internal_worker:
            require(all(value is not None and value.is_absolute() for value in
                        (args.launcher_helper, args.root, args.socket, args.control_socket)), "bootstrap paths differ")
            return worker(args)
        require(all(value is not None for value in (args.writer, args.verifier, args.launcher_helper,
                                                    args.store_root, args.report)), "observer arguments are absent")
        report["resource_limits"] = life.require_resources(expected_script="rust_ssh_loader_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        args.launcher_helper = args.launcher_helper.resolve(strict=True)
        root = Path(tempfile.mkdtemp(prefix="rust-ssh-loader-", dir=args.store_root)).resolve()
        root.chmod(0o700)
        package = session.package_backend()
        identity = package.validate_safe_tree_root(root)
        (root / "home").mkdir(mode=0o700)
        environment = life.private_env(root)
        provider = providers.Provider()
        provider.open()
        version, architecture = provider.package(environment)
        report["provenance"] = {"package_version": version, "package_arch": architecture, "components": provider.components()}
        report["helper_sha256"] = spawn_probe.file_hash(args.launcher_helper)
        report["observer_sha256"] = spawn_probe.file_hash(Path(__file__))
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
        require(code == 0, "independent sentinel load failed")
        loader.wipe(secret)
        secret = None
        for name in CASES:
            pending = {"case": name, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args.launcher_helper, args.writer, args.verifier, provider, root, transport, name, state))
            PHASE = "sentinel"
            require(sentinel.process.poll() is None and not life.pidfd_ready(sentinel.pidfd, 0), "independent sentinel was affected")
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
        report.update(status="observed", ssh_disconnect_tested=True, loader_lifecycle_tested=True,
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
        print("Rust SSH loader experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
