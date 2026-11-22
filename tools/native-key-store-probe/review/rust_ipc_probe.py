#!/usr/bin/env python3
"""Observe inherited IPC bootstrap boundaries using public fixtures only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_isolation_probe as isolation
import rust_launcher_probe as launcher
import session_probe as session

CASES = ("roundtrip", "wrong-generation", "wrong-store", "replay", "forwarded-sender",
         "ancillary-rights", "peer-exit", "receive-timeout", "truncated-frame", "oversized-frame")
SCOPE_FIELDS = {"custody_accredited", "product_recovery", "mcp_integration", "server_integration",
                "same_store_recovery", "lock_ack"}
IDENTITY_FIELDS = {"host_pid", "creator_tid", "server_pid", "injector_pid"}
ROOT_FIELDS = SCOPE_FIELDS | IDENTITY_FIELDS | {
    "version", "case", "status", "handshake_completed", "root_poisoned", "server_poisoned",
    "root_sticky_refusal", "server_sticky_refusal", "accepted_probes", "server_accepted_probes",
    "root_refusals", "server_refusals", "deadline_timed_out", "server_fd_count_before", "server_fd_count_after",
    "ancillary_fds_closed", "server_exit_code", "injector_exit_code", "owned_exits_observed",
    "resources_closed_after_exit", "pressure_threads", "pressure_iterations"}
EXTERNAL_FIELDS = {"root_identity_verified", "server_identity_verified", "injector_identity_verified",
                   "host_exit_code", "server_exit_observed", "injector_exit_observed",
                   "peers_retained_until_observation"}
REPORT_FIELDS = SCOPE_FIELDS | {"version", "experiment", "status", "cases", "resource_limits",
                              "resources_closed_after_exit", "helper_sha256", "observer_sha256"}
UNCERTAIN = []
require = abandon.require


def validate_result(value, case, identity=None, stage=None):
    require(type(value) is dict and set(value) == ROOT_FIELDS | ({"stage"} if stage else set()))
    require(case in CASES and value["case"] == case and value["status"] == "observed")
    require(stage is None or stage == "observation" and value["stage"] == stage)
    require(all(value[field] is False for field in SCOPE_FIELDS))
    root_bad = case in ("wrong-generation", "wrong-store", "peer-exit", "receive-timeout")
    server_bad = case in ("wrong-generation", "wrong-store", "replay", "forwarded-sender",
                          "ancillary-rights", "truncated-frame", "oversized-frame")
    accepted = 2 if case == "roundtrip" else 1 if case == "replay" else 0
    counts = {"version": 1, "accepted_probes": accepted, "server_accepted_probes": accepted,
              "root_refusals": int(root_bad), "server_refusals": int(server_bad), "pressure_threads": 4}
    for field, expected in counts.items():
        require(type(value[field]) is int and value[field] == expected)
    booleans = {"handshake_completed": case not in ("wrong-generation", "wrong-store", "receive-timeout"),
                "root_poisoned": root_bad, "server_poisoned": server_bad,
                "root_sticky_refusal": root_bad, "server_sticky_refusal": server_bad,
                "deadline_timed_out": case == "receive-timeout", "ancillary_fds_closed": case == "ancillary-rights",
                "owned_exits_observed": stage is None or case == "peer-exit", "resources_closed_after_exit": stage is None}
    require(all(value[field] is expected for field, expected in booleans.items()))
    for field in ("host_pid", "creator_tid", "server_pid", "server_fd_count_before"):
        require(type(value[field]) is int and value[field] > 0)
    require(value["host_pid"] == value["creator_tid"] and value["host_pid"] != value["server_pid"])
    if case == "forwarded-sender":
        require(type(value["injector_pid"]) is int and value["injector_pid"] > 0
                and len({value["host_pid"], value["server_pid"], value["injector_pid"]}) == 3)
    else:
        require(value["injector_pid"] is None)
    if case == "peer-exit":
        require(value["server_fd_count_after"] is None)
    else:
        require(type(value["server_fd_count_after"]) is int
                and 0 < value["server_fd_count_after"] <= value["server_fd_count_before"])
    server_exit = 0 if stage is None or case == "peer-exit" else None
    injector_exit = 0 if stage is None and case == "forwarded-sender" else None
    for field, expected in (("server_exit_code", server_exit), ("injector_exit_code", injector_exit)):
        require(value[field] is None if expected is None else type(value[field]) is int and value[field] == expected)
    require(type(value["pressure_iterations"]) is int
            and (value["pressure_iterations"] > 0 if stage is None else value["pressure_iterations"] == 0))
    if identity is not None:
        require(set(identity) == IDENTITY_FIELDS and all(value[field] == identity[field] for field in IDENTITY_FIELDS))


def parse_result(payload, case, identity, stage=None):
    value = abandon.decode(payload)
    validate_result(value, case, identity, stage)
    return value


def validate_observation(report):
    require(type(report) is dict and set(report) == REPORT_FIELDS)
    require(type(report["version"]) is int and report["version"] == 1
            and report["experiment"] == "rust-ipc" and report["status"] == "observed")
    require(all(report[field] is False for field in SCOPE_FIELDS)
            and report["resources_closed_after_exit"] is True and type(report["resource_limits"]) is dict)
    for field in ("helper_sha256", "observer_sha256"):
        require(type(report[field]) is str and len(report[field]) == 64
                and all(byte in "0123456789abcdef" for byte in report[field]))
    rows = report["cases"]
    require(type(rows) is list and len(rows) == 10 and all(type(row) is dict for row in rows)
            and {row.get("case") for row in rows} == set(CASES))
    for row in rows:
        require(set(row) == ROOT_FIELDS | EXTERNAL_FIELDS)
        validate_result({key: row[key] for key in ROOT_FIELDS}, row["case"])
        for field, expected in (("root_identity_verified", True), ("server_identity_verified", True),
                                 ("injector_identity_verified", row["case"] == "forwarded-sender"),
                                 ("server_exit_observed", True), ("injector_exit_observed", row["case"] == "forwarded-sender"),
                                 ("peers_retained_until_observation", row["case"] != "peer-exit")):
            require(row[field] is expected)
        require(type(row["host_exit_code"]) is int and row["host_exit_code"] == 0)


def ready(channel, stage, case, host_pid, expected=None):
    fields = {"case", "host_pid", "server_pid", "creator_tid"}
    if stage == "injector-ready":
        fields.add("injector_pid")
    value = abandon.packet(channel, fields, stage)
    require(value["case"] == case and type(value["host_pid"]) is int and value["host_pid"] == host_pid
            and type(value["creator_tid"]) is int and value["creator_tid"] == host_pid
            and type(value["server_pid"]) is int and value["server_pid"] > 0 and value["server_pid"] != host_pid)
    if expected is not None:
        require(value["server_pid"] == expected["server_pid"] and value["creator_tid"] == expected["creator_tid"])
    if stage == "injector-ready":
        require(type(value["injector_pid"]) is int and value["injector_pid"] > 0
                and len({host_pid, value["server_pid"], value["injector_pid"]}) == 3)
    return value


def public_store(root):
    root.mkdir(mode=0o700)
    key = b"public IPC key identity fixture\n"
    life.private_write(root / "key.openssh", key)
    life.private_write(root / "manifest.json", b'{"fixture":"public IPC identity"}\n')
    return key


def sealed_public_cipher(data):
    import fcntl
    fd = os.memfd_create("vivac-public-ipc-store", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    try:
        require(os.write(fd, data) == len(data))
        seals = fcntl.F_SEAL_SEAL | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_GROW | fcntl.F_SEAL_WRITE
        fcntl.fcntl(fd, fcntl.F_ADD_SEALS, seals)
        require(fcntl.fcntl(fd, fcntl.F_GET_SEALS) & seals == seals)
        return fd
    except BaseException:
        os.close(fd)
        raise


def run_case(helper, case, root, environment):
    case_root = root / case
    case_root.mkdir(mode=0o700)
    key = public_store(case_root / "a")
    require(public_store(case_root / "b") == key)
    directory = os.open(case_root / "a", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    other = os.open(case_root / "b", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    cipher = sealed_public_cipher(key)
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    channel.settimeout(5)
    process = None
    pidfds = []
    observed = False
    try:
        helper_path = helper.resolve(strict=True)
        metadata = helper_path.stat()
        executable = (helper_path, metadata.st_dev, metadata.st_ino)
        process = subprocess.Popen([str(helper), "--ipc-case", case, "--observer-fd", str(peer.fileno()),
                                    "--directory-fd", str(directory), "--cipher-fd", str(cipher),
                                    "--other-directory-fd", str(other)], env=environment,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   close_fds=True, pass_fds=(peer.fileno(), directory, cipher, other))
        peer.close()
        root_pidfd = os.pidfd_open(process.pid, 0)
        pidfds.append(root_pidfd)
        initial = ready(channel, "ready", case, process.pid)
        server_pid = initial["server_pid"]
        server_pidfd = os.pidfd_open(server_pid, 0)
        pidfds.append(server_pidfd)
        root_start = isolation.verify_process(process.pid, os.getpid(), executable, root_pidfd)
        server_start = isolation.verify_process(server_pid, process.pid, executable, server_pidfd)
        injector_pidfd = injector_pid = injector_start = None
        channel.sendall(b"G")
        if case not in ("wrong-generation", "wrong-store", "receive-timeout"):
            ready(channel, "handshake-ready", case, process.pid, initial)
            isolation.verify_process(process.pid, os.getpid(), executable, root_pidfd, root_start)
            isolation.verify_process(server_pid, process.pid, executable, server_pidfd, server_start)
            channel.sendall(b"G")
        if case == "forwarded-sender":
            injected = ready(channel, "injector-ready", case, process.pid, initial)
            injector_pid = injected["injector_pid"]
            injector_pidfd = os.pidfd_open(injector_pid, 0)
            pidfds.append(injector_pidfd)
            injector_start = isolation.verify_process(injector_pid, process.pid, executable, injector_pidfd)
            channel.sendall(b"G")
        identity = {"host_pid": process.pid, "creator_tid": initial["creator_tid"], "server_pid": server_pid,
                    "injector_pid": injector_pid}
        phase = parse_result(channel.recv(8193), case, identity, "observation")
        isolation.verify_process(process.pid, os.getpid(), executable, root_pidfd, root_start)
        if case == "peer-exit":
            require(abandon.pidfd_ready(server_pidfd), "peer exit did not reach the retained pidfd")
        else:
            isolation.verify_process(server_pid, process.pid, executable, server_pidfd, server_start)
        if injector_pid is not None:
            isolation.verify_process(injector_pid, process.pid, executable, injector_pidfd, injector_start)
        channel.sendall(b"G")
        result = parse_result(channel.recv(8193), case, identity)
        unchanged = ROOT_FIELDS - {"pressure_iterations", "server_exit_code", "injector_exit_code",
                                   "owned_exits_observed", "resources_closed_after_exit"}
        require(all(phase[field] == result[field] for field in unchanged), "completed result changed its observation")
        require(abandon.pidfd_ready(server_pidfd, 3)
                and (injector_pidfd is None or abandon.pidfd_ready(injector_pidfd, 3)))
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0 and abandon.pidfd_ready(root_pidfd)
                and parse_result(output, case, identity) == result)
        result.update(root_identity_verified=True, server_identity_verified=True,
                      injector_identity_verified=case == "forwarded-sender", host_exit_code=0,
                      server_exit_observed=True, injector_exit_observed=case == "forwarded-sender",
                      peers_retained_until_observation=case != "peer-exit")
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, directory, other, cipher, pidfds, case_root))
        else:
            channel.close()
            peer.close()
            process.stdout.close()
            process.stderr.close()
            for fd in (directory, other, cipher, *pidfds):
                os.close(fd)


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-ipc", status="incomplete", cases=[], resources_closed_after_exit=False)
    root = None
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_ipc_probe.py")
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        root = Path(tempfile.mkdtemp(prefix="rust-ipc-", dir=args.store_root))
        root.chmod(0o700)
        tree_identity = session.package_backend().validate_safe_tree_root(root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        environment = life.private_env(root)
        for case in CASES:
            pending = {"case": case, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args.launcher_helper, case, root, environment))
        session.package_backend().cleanup_owned_tree(root, tree_identity)
        root = None
        report.update(status="observed", resources_closed_after_exit=True)
        validate_observation(report)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", resources_closed_after_exit=False, failure={"kind": type(error).__name__})
        if root is not None:
            UNCERTAIN.append(root)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust IPC experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
