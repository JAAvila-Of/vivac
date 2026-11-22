#!/usr/bin/env python3
"""Disposable Rust loader/PTY lifecycle observations with synthetic credentials."""
import argparse
import array
import json
import os
from pathlib import Path
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
try:
    import fcntl
except ImportError:
    fcntl = None

import heap_probe as heap
import lifecycle_probe as life
import openssh_probe as native
import provider_probe as providers
import rust_launcher_probe as spawn_probe
import session_probe as session

CASES = ("normal", "cancel-prompt", "cancel-handoff", "cancel-before-ready", "owner-kill-prompt",
         "owner-kill-handoff", "owner-kill-before-ready", "loader-kill-prompt")
FIELDS = {"stage", "agent_pid", "loader_pid", "creator_tid", "secret_writes", "ready_published",
          "echo_disabled", "prompt_observed", "loader_stopped", "loader_exit_code", "loader_exit_signal",
          "agent_exit_code", "agent_exit_signal", "identity_bound", "proof_verified", "loader_failed",
          "close_ack", "resource_release_after_exit"}
SEALS = fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL if sys.platform == "linux" else 15
CURRENT = None
UNCERTAIN = []

class ProbeError(Exception):
    pass

def require(ok, message):
    if not ok:
        raise ProbeError(message)

def wipe(value):
    if isinstance(value, bytearray):
        value[:] = b"\0" * len(value)

def parse_stage(payload, expected=None):
    require(0 < len(payload) <= 8192, "loader stage size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    stage = value.get("stage")
    require(stage in ("prompt", "handoff", "before-ready", "ready", "closed"), "loader stage name differs")
    require(set(value) == FIELDS | ({"signature"} if stage == "before-ready" else set()), "loader stage fields differ")
    require(expected is None or stage == expected, "loader stage ordering differs")
    for name in ("agent_pid", "loader_pid", "creator_tid"):
        require(type(value[name]) is int and value[name] > 0, "loader stage identity differs")
    require(type(value["secret_writes"]) is int and value["secret_writes"] in (0, 1), "secret handoff count differs")
    for component in ("agent", "loader"):
        code, sig = value[component + "_exit_code"], value[component + "_exit_signal"]
        require(not (code is not None and sig is not None)
                and (code is None or type(code) is int and 0 <= code <= 255)
                and (sig is None or type(sig) is int and 0 < sig < 65), "native exit metadata differs")
    for name in FIELDS - {"stage", "agent_pid", "loader_pid", "creator_tid", "secret_writes",
                          "loader_exit_code", "loader_exit_signal", "agent_exit_code", "agent_exit_signal"}:
        require(type(value[name]) is bool, "loader stage flag differs")
    if stage in ("prompt", "handoff", "before-ready"):
        require(not value["ready_published"], "readiness preceded admission")
    if stage == "prompt":
        require(value["prompt_observed"] and value["echo_disabled"] and value["secret_writes"] == 0,
                "prompt observation differs")
    if stage == "handoff":
        require(value["loader_stopped"] and value["secret_writes"] == 1, "handoff stop observation differs")
    if stage == "before-ready":
        signature = value["signature"]
        require(isinstance(signature, str) and len(signature) == 128
                and all(c in "0123456789abcdef" for c in signature), "public signature encoding differs")
        require(value["identity_bound"] and value["proof_verified"] and value["loader_exit_code"] == 0,
                "loaded capability was not validated")
    return value

def receive_master(connection):
    received = []
    try:
        size = array.array("i").itemsize
        packet, controls, flags, _ = connection.recvmsg(1, socket.CMSG_SPACE(2 * size), socket.MSG_CMSG_CLOEXEC)
        valid = True
        for level, kind, data in controls:
            valid &= level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS and len(data) % size == 0
            if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                values = array.array("i")
                values.frombytes(data[:len(data) - len(data) % size])
                received.extend(values)
        require(packet == b"F" and valid and not flags & socket.MSG_CTRUNC and len(received) == 1,
                "PTY master transfer differs")
        descriptor = received[0]
        require(fcntl.fcntl(descriptor, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
                and stat.S_ISCHR(os.fstat(descriptor).st_mode) and os.isatty(descriptor)
                and Path(os.ttyname(descriptor)).name == "ptmx", "transferred object is not a private PTY master")
        received.clear()
        return descriptor
    finally:
        for descriptor in received:
            os.close(descriptor)

def parse_final(payload, case):
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    extra = {"version", "case", "status", "pressure_iterations", "custody_accredited", "launcher_accredited", "full_session_accredited"}
    require(set(value) == FIELDS | extra and value["case"] == case and value["status"] == "observed"
            and type(value["version"]) is int and value["version"] == 1, "Rust loader final identity differs")
    require(all(value[field] is False for field in ("custody_accredited", "launcher_accredited", "full_session_accredited")),
            "Rust loader result claims untested admission")
    require(type(value["pressure_iterations"]) is int and value["pressure_iterations"] > 0,
            "Rust loader had no concurrent pressure")
    parse_stage(json.dumps({key: value[key] for key in FIELDS}).encode("ascii"), "closed")
    return value

def sealed_metadata(value):
    contents = json.dumps(value, separators=(",", ":")).encode("ascii")
    require(len(contents) <= 8192, "fixture metadata is too large")
    descriptor = os.memfd_create("vivac-loader-public-metadata", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    try:
        require(os.write(descriptor, contents) == len(contents), "fixture metadata write was incomplete")
        os.lseek(descriptor, 0, os.SEEK_SET)
        fcntl.fcntl(descriptor, fcntl.F_ADD_SEALS, SEALS)
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise

def create_fixture(writer, parent, environment):
    package = session.package_backend()
    secret = bytearray(os.urandom(32).hex().encode("ascii"))
    path = parent / "package"
    device, credential = "0" * 25 + "1", "0" * 25 + "2"
    comment = "vivac.device-key.v1:" + device + ":" + credential
    try:
        code, _ = package.create(writer, path, comment, environment,
                                  input_data=bytes(secret) + b"\n" + bytes(secret) + b"\n")
        require(code == 0, "encrypted package fixture creation failed")
        code, _ = package.check(writer, path, environment)
        require(code == 0, "encrypted package profile admission failed")
        snapshot = package.open_retained_package(path)
        package.revalidate_retained_package(snapshot)
        parsed = native.parse_key_bytes(snapshot["key_bytes"])
        require(parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["kdf"] == "bcrypt"
                and parsed["rounds"] == 64, "encrypted fixture profile differs")
        challenge = native.challenge(parsed["public_key"], device, credential)
        return path, snapshot, parsed, challenge, comment, secret
    except BaseException:
        wipe(secret)
        raise

def run_case(helper, writer, verifier, provider, root, environment, case, state):
    case_root = root / case
    case_root.mkdir(mode=0o700)
    (case_root / "home").mkdir(mode=0o700)
    env = life.private_env(case_root)
    _, snapshot, parsed, challenge, comment, secret = create_fixture(writer, case_root, env)
    package = session.package_backend()
    cipher = metadata = secret_read = master = process = channel = peer = connection = None
    pidfds = {}
    observed = False
    try:
        cipher, _ = package.make_sealed_key_memfd(snapshot["key_bytes"])
        require(fcntl.fcntl(cipher, fcntl.F_GET_SEALS) & SEALS == SEALS, "ciphertext snapshot seals differ")
        metadata = sealed_metadata({"public_blob": parsed["public_blob"].hex(), "comment": comment,
                                    "challenge": native.canonical(challenge).hex(), "public_key": parsed["public_key"].hex()})
        secret_read, secret_write = os.pipe2(os.O_CLOEXEC)
        try:
            require(os.write(secret_write, secret) == len(secret), "secret fixture pipe write was incomplete")
        finally:
            os.close(secret_write)
        channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        channel.settimeout(20)
        agent_fd, loader_fd = provider.files["ssh-agent"].fd, provider.files["ssh-add"].fd
        socket_path = case_root / "agent.sock"
        arguments = [str(helper), "--loader-case", case, "--agent-fd", str(agent_fd), "--loader-fd", str(loader_fd),
                     "--cipher-fd", str(cipher), "--secret-fd", str(secret_read), "--fixture-fd", str(metadata),
                     "--observer-fd", str(peer.fileno()), "--socket", str(socket_path)]
        state["uncertain"] = True
        process = subprocess.Popen(arguments, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   close_fds=True, pass_fds=(agent_fd, loader_fd, cipher, secret_read, metadata, peer.fileno()))
        peer.close()
        peer = None
        os.close(secret_read)
        secret_read = None
        master = receive_master(channel)
        frame = parse_stage(channel.recv(8193), "prompt")
        require(frame["prompt_observed"] and frame["echo_disabled"] and frame["secret_writes"] == 0
                and not frame["ready_published"], "prompt did not gate secret handoff")
        pids = {"agent": frame["agent_pid"], "loader": frame["loader_pid"]}
        for name, pid in pids.items():
            pidfds[name] = os.pidfd_open(pid)
            expected = provider.files["ssh-agent" if name == "agent" else "ssh-add"].path
            life.inspect_agent(pid, life.process_start(pid), str(expected), process.pid)
        slave = (Path("/proc") / str(pids["loader"]) / "fd/0").stat()
        require(stat.S_ISCHR(slave.st_mode) and slave.st_uid == os.geteuid()
                and stat.S_IMODE(slave.st_mode) == 0o600, "native PTY slave permissions differ")
        connection = socket.socket(socket.AF_UNIX)
        connection.connect(str(socket_path))
        stopped = False
        proof_verified = False
        selected = "prompt"
        if case not in ("cancel-prompt", "owner-kill-prompt", "loader-kill-prompt"):
            package.revalidate_retained_package(snapshot)
            channel.sendall(b"H")
            frame = parse_stage(channel.recv(8193), "handoff")
            state_text = (Path("/proc") / str(pids["loader"]) / "status").read_text(encoding="ascii")
            stopped = any(line.startswith("State:") and line.split()[1] == "T" for line in state_text.splitlines())
            require(frame["loader_stopped"] and stopped and frame["secret_writes"] == 1
                    and not frame["ready_published"], "handoff was not stopped and bounded")
            selected = "handoff"
        if case in ("normal", "cancel-before-ready", "owner-kill-before-ready"):
            package.revalidate_retained_package(snapshot)
            channel.sendall(b"L")
            frame = parse_stage(channel.recv(8193), "before-ready")
            require(not frame["ready_published"], "readiness preceded independent proof validation")
            native.verify(verifier, case_root, env, native.proof(challenge, bytes.fromhex(frame["signature"])), "verified-proof", True)
            proof_verified = True
            selected = "before-ready"
        owner_killed = case.startswith("owner-kill-")
        loader_failed = case == "loader-kill-prompt"
        if case == "normal":
            package.revalidate_retained_package(snapshot)
            channel.sendall(b"R")
            frame = parse_stage(channel.recv(8193), "ready")
            require(frame["ready_published"], "validated test readiness was not published")
            selected = "ready"
        if owner_killed:
            process.kill()
            require(process.wait(timeout=3) == -signal.SIGKILL, "Rust supervisor did not exit by SIGKILL")
            agent_status = spawn_probe.observe_exit(pids["agent"], pidfds["agent"], time.monotonic() + 5)
            require(os.WIFSIGNALED(agent_status) and os.WTERMSIG(agent_status) == signal.SIGKILL,
                    "native agent survived its Rust owner")
            if selected != "before-ready":
                loader_status = spawn_probe.observe_exit(pids["loader"], pidfds["loader"], time.monotonic() + 5)
                require(os.WIFSIGNALED(loader_status) and os.WTERMSIG(loader_status) == signal.SIGKILL,
                        "native loader survived its Rust owner")
            require(spawn_probe.bounded_output(process, time.monotonic() + 3) == b"", "supervisor death emitted unexpected stdout")
            close_ack = False
        else:
            if loader_failed:
                life.pidfd_signal(pidfds["loader"], signal.SIGKILL)
            channel.sendall(b"F" if loader_failed else b"C")
            frame = parse_stage(channel.recv(8193), "closed")
            require(frame["close_ack"] and frame["resource_release_after_exit"], "closure preceded observed native exits")
            require(frame["agent_exit_signal"] == signal.SIGKILL and frame["ready_published"] is (case == "normal"),
                    "agent closure or readiness differs")
            if proof_verified:
                require(frame["loader_exit_code"] == 0, "loaded native loader did not exit successfully")
            else:
                require(frame["loader_exit_signal"] == signal.SIGKILL, "cancelled native loader exit differs")
            require(frame["loader_failed"] is loader_failed, "selected loader-failure cause differs")
            output = spawn_probe.bounded_output(process, time.monotonic() + 5)
            require(process.wait(timeout=3) == 0, "Rust loader case did not complete")
            parse_final(output, case)
            close_ack = True
        require(life.pidfd_ready(pidfds["agent"], 0) and life.pidfd_ready(pidfds["loader"], 0), "native exit remains unobserved")
        require(spawn_probe.connection_refused(connection, socket_path), "closed agent remains usable")
        package.revalidate_retained_package(snapshot)
        provider.revalidate()
        observed = True
        state["uncertain"] = False
        return {"case": case, "status": "observed", "selected_seam": selected, "secret_writes": int(stopped),
                "ready_published": case == "normal", "independent_proof_verified": proof_verified,
                "agent_exit_observed": True, "loader_exit_observed": True, "pty_master_retained_through_exit": True,
                "loader_stopped_observed": stopped, "close_ack": close_ack, "owner_sigkill_observed": owner_killed,
                "loader_failed_observed": loader_failed, "retained_connection_refused": True,
                "new_connection_refused": True, "lease_revalidated": True, "sealed_snapshot": True,
                "pty_slave_private": True}
    finally:
        wipe(secret)
        if not observed:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            state["uncertain"] = True
            UNCERTAIN.append((snapshot, cipher, metadata, secret_read, master, process, channel, peer, connection, pidfds))
        else:
            for descriptor in (*pidfds.values(), cipher, metadata, secret_read, master):
                if descriptor is not None:
                    os.close(descriptor)
            package.close_retained_package(snapshot)
            for value in (channel, peer, connection):
                if value:
                    value.close()
            for stream in (process.stdout, process.stderr):
                stream.close()

def main():
    global CURRENT
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-loader", "status": "incomplete", "cases": [],
              "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
              "ssh_disconnect_tested": False, "loader_lifecycle_tested": False, "sentinel_survived": False}
    CURRENT = report
    state = {"uncertain": False}
    provider = sentinel = None
    snapshot = root = None
    secret = None
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable Linux is required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        report["resource_limits"] = life.require_resources(expected_script="rust_loader_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        root = Path(tempfile.mkdtemp(prefix="rust-loader-", dir=args.store_root))
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
        sentinel_root = root / "sentinel"
        sentinel_root.mkdir(mode=0o700)
        (sentinel_root / "home").mkdir(mode=0o700)
        path, snapshot, parsed, challenge, _, secret = create_fixture(args.writer, sentinel_root, environment)
        sentinel = providers.OwnedAgent(provider, root, "sentinel")
        sentinel.start()
        sentinel_env = life.private_env(root, sentinel.path)
        code, _ = provider.execute("ssh-add", ["-k", str(path / "key.openssh")], sentinel_env, responses=[secret])
        require(code == 0, "independent sentinel load failed")
        wipe(secret)
        secret = None
        for case in CASES:
            pending = {"case": case, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args.launcher_helper, args.writer, args.verifier, provider, root, environment, case, state))
            require(sentinel.process.poll() is None and not life.pidfd_ready(sentinel.pidfd, 0), "independent sentinel was affected")
            signature = native.sign(sentinel.path, parsed["public_blob"], native.canonical(challenge))
            native.verify(args.verifier, root, environment, native.proof(challenge, signature), "sentinel-" + case, True)
        sentinel.stop()
        sentinel.close()
        sentinel = None
        package.close_retained_package(snapshot)
        snapshot = None
        provider.close()
        provider = None
        package.cleanup_owned_tree(root, identity)
        root = None
        report.update(status="observed", loader_lifecycle_tested=True, sentinel_survived=True, resources_closed_after_exit=True)
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        wipe(secret)
        report.update(status="incomplete", failure={"kind": type(error).__name__})
        UNCERTAIN.append((provider, sentinel, snapshot, root))
        try:
            life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        except OSError:
            pass
        print("Rust loader experiment failed.", file=sys.stderr)
        return 1

if __name__ == "__main__":
    sys.exit(main())
