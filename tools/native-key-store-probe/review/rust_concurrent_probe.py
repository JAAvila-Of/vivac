#!/usr/bin/env python3
"""Disposable Rust signing/lock overlap with synthetic credentials."""
import argparse
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

import heap_probe as heap
import lifecycle_probe as life
import openssh_probe as native
import provider_probe as providers
import rust_launcher_probe as spawn
import rust_loader_probe as loader
import session_probe as session

CASES = ("completed-before-lock", "lock-before-native-send", "lock-native-response-pending",
         "lock-after-native-reply", "lock-before-publication", "parallel-lock-callers",
         "lease-change-before-publication", "session-abandon")
SEAMS = dict(zip(CASES, ("before-native-send", "before-native-send", "native-response-pending",
                        "after-native-reply", "before-publication", "before-native-send", "before-publication",
                        "before-native-send")))
FINAL_FIELDS = {"case", "status", "selected_seam", "request_outcome", "state", "active",
                "native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published",
                "lock_callers", "lock_acknowledgments", "workers_joined", "close_acknowledged",
                "resources_closed_after_drain", "closing_observed_while_paused", "request_late_refused",
                "drain_wait_observed", "busy_refused", "closed_refused", "creator_tid", "agent_pid", "loader_pid",
                "agent_exit_code", "agent_exit_signal", "loader_exit_code", "loader_exit_signal",
                "pressure_iterations", "version", "custody_accredited", "full_session_accredited", "concurrent_lock_tested"}
ABANDON_FINAL_FIELDS = {"case", "status", "version", "selected_seam", "request_outcome", "state", "active",
                        "native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published",
                        "workers_joined", "lock_callers", "lock_acknowledgments", "close_acknowledged",
                        "resources_closed_after_drain", "admission_uncertain", "readmission_refused",
                        "descriptors_retained", "retained_descriptor_count", "drop_elapsed_ns", "creator_tid",
                        "agent_pid", "loader_pid", "loader_exit_code", "loader_exit_signal", "pressure_iterations",
                        "custody_accredited", "full_session_accredited", "concurrent_lock_tested", "callback_uncertain"}
UNCERTAIN = []
PHASE = "preflight"
require = loader.require


def bound_stage(payload, stage, identity):
    value = loader.parse_stage(payload, stage)
    require(all(value[field] == identity[field] for field in ("agent_pid", "loader_pid", "creator_tid")),
            "concurrent startup identity changed")
    return value


def packet(channel, fields, stage):
    payload = channel.recv(8193)
    require(0 < len(payload) <= 8192, "concurrent packet size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    require(isinstance(value, dict) and set(value) == set(fields) and value.get("stage") == stage,
            "concurrent packet identity differs")
    return value


def parse_final(payload, case, identity):
    require(0 < len(payload) <= 8192, "concurrent final size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    require(isinstance(value, dict) and set(value) == FINAL_FIELDS, "concurrent final fields differ")
    for field in ("agent_pid", "loader_pid", "creator_tid"):
        require(type(value[field]) is int and value[field] == identity[field], "concurrent native identity changed")
    require(type(value["version"]) is int and value["version"] == 1 and value["case"] == case
            and value["status"] == "observed" and value["state"] == "CLOSED"
            and value["selected_seam"] == SEAMS[case], "concurrent final identity differs")
    completed = case == "completed-before-lock"
    lease = case == "lease-change-before-publication"
    closing = not (completed or lease)
    sent = int(case not in ("lock-before-native-send", "parallel-lock-callers"))
    replies = int(case in ("completed-before-lock", "lock-after-native-reply", "lock-before-publication",
                           "lease-change-before-publication"))
    verified = int(case in ("completed-before-lock", "lock-before-publication", "lease-change-before-publication"))
    expected = {"native_requests_sent": sent, "native_replies_received": replies,
                "proofs_verified": verified, "proofs_published": int(completed), "workers_joined": 1,
                "lock_callers": 2 if case == "parallel-lock-callers" else 1,
                "lock_acknowledgments": 2 if case == "parallel-lock-callers" else 1,
                "agent_exit_signal": 9, "loader_exit_code": 0}
    for field, expected_value in expected.items():
        require(type(value[field]) is int and value[field] == expected_value, "concurrent event count or exit differs")
    require(value["agent_exit_code"] is None and value["loader_exit_signal"] is None, "native exit cause differs")
    for field in ("pressure_iterations", "busy_refused", "closed_refused"):
        require(type(value[field]) is int and value[field] >= (1 if field != "busy_refused" or completed else 0),
                "concurrent control counter differs")
    booleans = {"active": False, "close_acknowledged": True, "resources_closed_after_drain": True,
                "closing_observed_while_paused": closing, "request_late_refused": closing,
                "drain_wait_observed": closing, "custody_accredited": False,
                "full_session_accredited": False, "concurrent_lock_tested": True}
    require(all(value[field] is expected_value for field, expected_value in booleans.items()),
            "concurrent closure evidence differs")
    require(value["request_outcome"] == ("published" if completed else "lease_invalidated" if lease else "cancelled_by_lock"),
            "concurrent request cause differs")
    return value


def parse_abandon_final(payload, identity):
    require(0 < len(payload) <= 8192, "abandon final size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    require(isinstance(value, dict) and set(value) == ABANDON_FINAL_FIELDS, "abandon final fields differ")
    for field in ("agent_pid", "loader_pid", "creator_tid"):
        require(type(value[field]) is int and value[field] == identity[field], "abandon identity changed")
    require(type(value["version"]) is int and value["version"] == 1
            and value["case"] == "session-abandon" and value["status"] == "observed"
            and value["selected_seam"] == "before-native-send" and value["state"] == "CLOSING"
            and value["request_outcome"] == "failed_after_abandon", "abandon final identity differs")
    integers = {"native_requests_sent": 0, "native_replies_received": 0, "proofs_verified": 0,
                "proofs_published": 0, "workers_joined": 1, "lock_callers": 1,
                "lock_acknowledgments": 0, "readmission_refused": 3, "retained_descriptor_count": 5,
                "loader_exit_code": 0}
    require(all(type(value[field]) is int and value[field] == expected for field, expected in integers.items()),
            "abandon event count differs")
    for field in ("drop_elapsed_ns", "pressure_iterations"):
        require(type(value[field]) is int and value[field] > 0, "abandon positive counter differs")
    booleans = {"active": False, "close_acknowledged": False, "resources_closed_after_drain": False,
                "admission_uncertain": True, "descriptors_retained": True, "custody_accredited": False,
                "full_session_accredited": False, "concurrent_lock_tested": False, "callback_uncertain": True}
    require(all(value[field] is expected for field, expected in booleans.items())
            and value["loader_exit_signal"] is None, "abandon retention evidence differs")
    return value


def retained_descriptor_identities(process, values):
    require(isinstance(values, list) and len(values) == 5, "abandon descriptor count differs")
    descriptors = set()
    for value in values:
        require(isinstance(value, list) and len(value) == 4 and all(type(item) is int for item in value),
                "abandon descriptor identity shape differs")
        fd, device, inode, mode = value
        require(fd >= 3 and fd not in descriptors and device >= 0 and inode > 0 and mode > 0,
                "abandon descriptor identity differs")
        descriptors.add(fd)
        actual = Path("/proc", str(process.pid), "fd", str(fd)).stat()
        require((actual.st_dev, actual.st_ino, actual.st_mode) == (device, inode, mode),
                "abandoned retained descriptor changed")
    require(process.poll() is None, "abandon helper exited before observer release")
    return values


def observe_abandon(process, channel, identity, pidfds, snapshot, package, master, connection, socket_path):
    channel.sendall(b"S")
    paused = packet(channel, ("stage", "case", "agent_pid", "loader_pid", "creator_tid", "active",
                              "close_ack", "descriptors"), "abandon-paused")
    require(paused["case"] == "session-abandon" and paused["active"] is True and paused["close_ack"] is False
            and all(type(paused[field]) is int and paused[field] == identity[field]
                    for field in ("agent_pid", "loader_pid", "creator_tid")), "abandon pause differs")
    descriptors = retained_descriptor_identities(process, paused["descriptors"])
    require(not life.pidfd_ready(pidfds["agent"], 0), "agent exited before session abandonment")
    package.revalidate_retained_package(snapshot)
    channel.settimeout(5)
    started = time.monotonic()
    channel.sendall(b"G")
    returned = packet(channel, ("stage", "case", "drop_elapsed_ns", "active", "close_ack",
                                "admission_uncertain", "readmission_refused", "descriptors"), "abandon-returned")
    require(time.monotonic() - started < 5 and returned["case"] == "session-abandon"
            and type(returned["drop_elapsed_ns"]) is int and returned["drop_elapsed_ns"] > 0
            and returned["active"] is True and returned["close_ack"] is False
            and returned["admission_uncertain"] is True
            and type(returned["readmission_refused"]) is int and returned["readmission_refused"] == 3
            and retained_descriptor_identities(process, returned["descriptors"]) == descriptors,
            "abandon drop did not return with custody retained")
    package.revalidate_retained_package(snapshot)
    channel.sendall(b"G")
    final = parse_abandon_final(channel.recv(8193), identity)
    require(final["drop_elapsed_ns"] == returned["drop_elapsed_ns"], "abandon timing changed")
    retained_descriptor_identities(process, descriptors)
    package.revalidate_retained_package(snapshot)
    require(life.pidfd_ready(pidfds["agent"], 0) and life.pidfd_ready(pidfds["loader"], 0),
            "abandon native exits remain unobserved")
    require(os.isatty(master) and spawn.connection_refused(connection, socket_path),
            "abandoned native connection remains usable")
    channel.sendall(b"X")
    output = spawn.bounded_output(process, time.monotonic() + 5)
    require(process.wait(timeout=3) == 0 and parse_abandon_final(output, identity) == final,
            "abandon helper did not exit normally after observer release")
    native_status = spawn.observe_exit(identity["agent_pid"], pidfds["agent"], time.monotonic() + 5)
    require(os.WIFSIGNALED(native_status) and os.WTERMSIG(native_status) == signal.SIGKILL,
            "abandoned agent SIGKILL was not independently reaped")
    return dict(final, readiness_proof_verified=True, published_proof_verified=False,
                retained_connection_refused=True, new_connection_refused=True, lease_invalidated_observed=False,
                pty_master_retained_through_exit=True, pty_slave_private=True, sealed_snapshot=True,
                helper_responsive_after_drop=True, retained_descriptors_independently_checked=True,
                agent_exit_signal=signal.SIGKILL, agent_exit_code=None, helper_exit_code=0,
                resources_retained_until_helper_exit=True)


def run_case(args, provider, root, case, state):
    global PHASE
    PHASE = "fixture"
    case_root = root / case
    case_root.mkdir(mode=0o700)
    (case_root / "home").mkdir(mode=0o700)
    env = life.private_env(case_root)
    path, snapshot, parsed, challenge, comment, secret = loader.create_fixture(args.writer, case_root, env)
    package = session.package_backend()
    cipher = metadata = secret_read = master = process = channel = peer = connection = None
    pidfds = {}
    observed = invalidated = False
    try:
        cipher, _ = package.make_sealed_key_memfd(snapshot["key_bytes"])
        require(loader.fcntl.fcntl(cipher, loader.fcntl.F_GET_SEALS) & loader.SEALS == loader.SEALS,
                "ciphertext seals differ")
        metadata = loader.sealed_metadata({"public_blob": parsed["public_blob"].hex(), "comment": comment,
                                           "challenge": native.canonical(challenge).hex(), "public_key": parsed["public_key"].hex()})
        secret_read, secret_write = os.pipe2(os.O_CLOEXEC)
        try:
            require(os.write(secret_write, secret) == len(secret), "secret pipe write differs")
        finally:
            os.close(secret_write)
        loader.wipe(secret)
        channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        channel.settimeout(25)
        agent_fd, loader_fd = provider.files["ssh-agent"].fd, provider.files["ssh-add"].fd
        socket_path = case_root / "agent.sock"
        arguments = [str(args.launcher_helper), "--concurrent-case", case]
        for name, descriptor in (("agent", agent_fd), ("loader", loader_fd), ("cipher", cipher),
                                 ("secret", secret_read), ("fixture", metadata), ("observer", peer.fileno())):
            arguments.extend(("--" + name + "-fd", str(descriptor)))
        arguments.extend(("--lease-dir-fd", str(snapshot["directory_fd"]), "--socket", str(socket_path)))
        state["uncertain"] = True
        process = subprocess.Popen(arguments, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, close_fds=True, pass_fds=(agent_fd, loader_fd, cipher,
                                   secret_read, metadata, peer.fileno(), snapshot["directory_fd"]))
        peer.close()
        peer = None
        os.close(secret_read)
        secret_read = None
        PHASE = "startup"
        master = loader.receive_master(channel)
        identity = loader.parse_stage(channel.recv(8193), "prompt")
        require(identity["creator_tid"] == process.pid, "native creator is not the retained main thread")
        for name in ("agent", "loader"):
            pid = identity[name + "_pid"]
            pidfds[name] = os.pidfd_open(pid)
            life.inspect_agent(pid, life.process_start(pid), str(provider.files[
                "ssh-agent" if name == "agent" else "ssh-add"].path), process.pid)
        slave = Path("/proc", str(identity["loader_pid"]), "fd/0").stat()
        require(stat.S_ISCHR(slave.st_mode) and slave.st_uid == os.geteuid()
                and stat.S_IMODE(slave.st_mode) == 0o600, "native private PTY differs")
        connection, retained_signature = None, None
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"H")
        bound_stage(channel.recv(8193), "handoff", identity)
        status = Path("/proc", str(identity["loader_pid"]), "status").read_text(encoding="ascii")
        require(any(line.startswith("State:") and line.split()[1] == "T" for line in status.splitlines()),
                "native loader stop was not observed")
        channel.sendall(b"L")
        frame = bound_stage(channel.recv(8193), "before-ready", identity)
        native.verify(args.verifier, case_root, env, native.proof(challenge, bytes.fromhex(frame["signature"])), "ready-proof", True)
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"R")
        bound_stage(channel.recv(8193), "ready", identity)
        ready = packet(channel, ("stage", "agent_pid", "loader_pid", "creator_tid"), "concurrent-ready")
        require(all(ready[field] == identity[field] for field in ("agent_pid", "loader_pid", "creator_tid")),
                "ready ownership changed")
        connection, retained_signature = life.open_signed_connection(socket_path, parsed["public_blob"], comment, native.canonical(challenge))
        native.verify(args.verifier, case_root, env, native.proof(challenge, retained_signature), "retained-proof", True)
        PHASE = "race"
        if case == "session-abandon":
            final = observe_abandon(process, channel, identity, pidfds, snapshot, package, master, connection, socket_path)
            provider.revalidate()
            observed = True
            state["uncertain"] = False
            return final
        channel.sendall(b"S")
        race = packet(channel, ("stage", "case", "seam", "agent_pid", "loader_pid", "creator_tid", "active"), "race-paused")
        require(race["case"] == case and race["seam"] == SEAMS[case] and race["active"] is True
                and all(race[field] == identity[field] for field in ("agent_pid", "loader_pid", "creator_tid")),
                "selected concurrent seam differs")
        if case == "lease-change-before-publication":
            descriptor = os.open("key.openssh", os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=snapshot["directory_fd"])
            try:
                value = snapshot["key_bytes"][-1] ^ 1
                require(os.pwrite(descriptor, bytes([value]), len(snapshot["key_bytes"]) - 1) == 1, "lease mutation was incomplete")
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
            try:
                package.revalidate_retained_package(snapshot)
            except package.LeaseInvalidated:
                invalidated = True
            require(invalidated, "lease mutation was not independently detected")
        else:
            package.revalidate_retained_package(snapshot)
        channel.sendall(b"G")
        published = case == "completed-before-lock"
        if published:
            proof = packet(channel, ("stage", "signature"), "published-proof")
            require(isinstance(proof["signature"], str) and len(proof["signature"]) == 128
                    and all(c in "0123456789abcdef" for c in proof["signature"]), "published proof encoding differs")
            native.verify(args.verifier, case_root, env, native.proof(challenge, bytes.fromhex(proof["signature"])), "published-proof", True)
            channel.sendall(b"V")
        elif not invalidated:
            closing = packet(channel, ("stage", "case", "seam", "active", "close_ack", "lock_callers"), "closing-paused")
            require(closing["case"] == case and closing["seam"] == SEAMS[case]
                    and closing["active"] is True and closing["close_ack"] is False
                    and type(closing["lock_callers"]) is int
                    and closing["lock_callers"] == (2 if case == "parallel-lock-callers" else 1),
                    "lock overlap before drain was not observed")
            channel.sendall(b"G")
        PHASE = "closure"
        final = parse_final(channel.recv(8193), case, identity)
        output = spawn.bounded_output(process, time.monotonic() + 5)
        require(process.wait(timeout=3) == 0, "concurrent helper did not complete")
        require(parse_final(output, case, identity) == final, "final stdout differs from control metadata")
        require(all(life.pidfd_ready(fd, 0) for fd in pidfds.values()), "native exits remain unobserved")
        require(os.isatty(master) and spawn.connection_refused(connection, socket_path), "post-lock capability remains usable")
        if not invalidated:
            package.revalidate_retained_package(snapshot)
        provider.revalidate()
        observed = True
        state["uncertain"] = False
        return dict(final, readiness_proof_verified=True, published_proof_verified=published,
                    retained_connection_refused=True, new_connection_refused=True, lease_invalidated_observed=invalidated,
                    pty_master_retained_through_exit=True, pty_slave_private=True, sealed_snapshot=True)
    finally:
        loader.wipe(secret)
        if not observed:
            state["uncertain"] = True
            UNCERTAIN.append((snapshot, cipher, metadata, secret_read, master, process, channel, peer, connection, pidfds))
        else:
            for descriptor in (*pidfds.values(), cipher, metadata, secret_read, master):
                if descriptor is not None:
                    os.close(descriptor)
            package.close_retained_package(snapshot)
            for value in (channel, peer, connection, process.stdout, process.stderr):
                if value is not None:
                    value.close()


def main():
    global PHASE
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-concurrent", "status": "incomplete", "cases": [],
              "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
              "ssh_disconnect_tested": False, "concurrent_lock_tested": False, "sentinel_survived": False}
    state = {"uncertain": False}
    provider = sentinel = snapshot = root = secret = None
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable Linux is required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        report["resource_limits"] = life.require_resources(expected_script="rust_concurrent_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        root = Path(tempfile.mkdtemp(prefix="rust-gate-", dir=args.store_root))
        root.chmod(0o700)
        package = session.package_backend()
        identity = package.validate_safe_tree_root(root)
        (root / "home").mkdir(mode=0o700)
        env = life.private_env(root)
        provider = providers.Provider()
        provider.open()
        version, architecture = provider.package(env)
        report["provenance"] = {"package_version": version, "package_arch": architecture, "components": provider.components()}
        report["helper_sha256"] = spawn.file_hash(args.launcher_helper)
        report["observer_sha256"] = spawn.file_hash(Path(__file__))
        sentinel_root = root / "sentinel"
        sentinel_root.mkdir(mode=0o700)
        (sentinel_root / "home").mkdir(mode=0o700)
        path, snapshot, parsed, challenge, _, secret = loader.create_fixture(args.writer, sentinel_root, env)
        sentinel = providers.OwnedAgent(provider, root, "sentinel")
        sentinel.start()
        code, _ = provider.execute("ssh-add", ["-k", str(path / "key.openssh")], life.private_env(root, sentinel.path), responses=[secret])
        require(code == 0, "independent sentinel load failed")
        loader.wipe(secret)
        secret = None
        for case in CASES:
            pending = {"case": case, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args, provider, root, case, state))
            PHASE = "sentinel"
            require(sentinel.process.poll() is None and not life.pidfd_ready(sentinel.pidfd, 0), "independent sentinel was affected")
            signature = native.sign(sentinel.path, parsed["public_blob"], native.canonical(challenge))
            native.verify(args.verifier, root, env, native.proof(challenge, signature), "sentinel-" + case, True)
        sentinel.stop()
        sentinel.close()
        sentinel = None
        package.close_retained_package(snapshot)
        snapshot = None
        provider.close()
        provider = None
        package.cleanup_owned_tree(root, identity)
        root = None
        report.update(status="observed", concurrent_lock_tested=True, sentinel_survived=True, resources_closed_after_exit=True)
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        loader.wipe(secret)
        report.update(status="incomplete", failure={"phase": PHASE, "kind": type(error).__name__})
        UNCERTAIN.append((provider, sentinel, snapshot, root))
        try:
            life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        except OSError:
            pass
        print("Rust concurrent experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
