#!/usr/bin/env python3
"""Observe cooperative directory reservations using public disposable markers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_isolation_probe as isolation
import rust_launcher_probe as launcher
import session_probe as session

CASES = {"contention", "normal-release", "abandonment-retains", "death-is-not-exit",
         "replacement-binding", "invalidation-retains"}
SCOPE_FIELDS = {"custody_accredited", "product_recovery", "same_store_recovery",
                "mcp_integration", "server_integration", "lock_ack"}
IDENTITY_FIELDS = {"host_pid", "creator_tid", "worker_pid", "worker_token", "native_pid", "native_token"}
HOST_FIELDS = SCOPE_FIELDS | IDENTITY_FIELDS | {
    "version", "case", "status", "same_ofd_contention_refused", "different_store_available",
    "readmitted_before_worker_exit", "readmitted_after_worker_death", "reservation_retained_after_action",
    "lease_invalidated", "replacement_refused", "original_valid", "admission_poisoned", "drop_elapsed_ns",
    "worker_exit_code", "worker_exit_signal", "worker_exit_observed", "pressure_threads", "pressure_iterations",
    "resources_closed_after_worker_exit"}
EXTERNAL_FIELDS = {"process_identities_verified", "worker_alive_after_action", "native_alive_after_action",
                   "native_exit_observed", "native_exit_signal", "native_status_source", "host_exit_code",
                   "normal_worker_exit_commanded", "negative_control_pidfd_kill_verified",
                   "resources_closed_after_observed_exit"}
REPORT_FIELDS = SCOPE_FIELDS | {"version", "experiment", "status", "cases", "resource_limits",
                              "helper_sha256", "observer_sha256", "resources_closed_after_observed_exit"}
UNCERTAIN = []
require = abandon.require


def integer(value, positive=False):
    require(type(value) is int and (0 < value if positive else 0 <= value) and value <= 0xffffffffffffffff)


def validate_identity(value, case, expected=None):
    require(set(value) == IDENTITY_FIELDS)
    for field in ("host_pid", "creator_tid", "worker_pid", "worker_token"):
        integer(value[field], True)
    require(value["creator_tid"] == value["host_pid"] and value["worker_pid"] != value["host_pid"])
    native = case in ("abandonment-retains", "death-is-not-exit")
    if native:
        integer(value["native_pid"], True)
        require(value["native_pid"] not in (value["host_pid"], value["worker_pid"]))
    else:
        require(value["native_pid"] is None)
    if case == "abandonment-retains":
        integer(value["native_token"], True)
    else:
        require(value["native_token"] is None)
    require(expected is None or value == expected, "bound receipt identity changed")


def validate_host(value, identity=None, *, observation=False):
    require(type(value) is dict and set(value) == HOST_FIELDS, "host result schema differs")
    case = value["case"]
    require(case in CASES and value["status"] == "observed")
    validate_identity({field: value[field] for field in IDENTITY_FIELDS}, case, identity)
    for field in SCOPE_FIELDS:
        require(value[field] is False, "reservation observation cannot acknowledge custody or recovery")
    for field, expected in {
        "same_ofd_contention_refused": True, "different_store_available": True,
        "readmitted_before_worker_exit": case == "normal-release",
        "readmitted_after_worker_death": case == "death-is-not-exit",
        "reservation_retained_after_action": case not in ("normal-release", "death-is-not-exit"),
        "lease_invalidated": case == "invalidation-retains", "replacement_refused": case == "replacement-binding",
        "original_valid": case == "replacement-binding", "admission_poisoned": case == "abandonment-retains",
        "worker_exit_observed": not observation or case == "death-is-not-exit",
        "resources_closed_after_worker_exit": not observation,
    }.items():
        require(value[field] is expected, "reservation evidence differs: " + field)
    for field, expected in (("version", 1), ("pressure_threads", 4)):
        require(type(value[field]) is int and value[field] == expected)
    integer(value["pressure_iterations"], not observation)
    require(not observation or value["pressure_iterations"] == 0)
    integer(value["drop_elapsed_ns"], case == "abandonment-retains")
    require(case == "abandonment-retains" or value["drop_elapsed_ns"] == 0)
    if case == "death-is-not-exit":
        require(value["worker_exit_code"] is None and type(value["worker_exit_signal"]) is int
                and value["worker_exit_signal"] == 9)
    else:
        require(value["worker_exit_signal"] is None)
        if observation:
            require(value["worker_exit_code"] is None)
        else:
            require(type(value["worker_exit_code"]) is int and value["worker_exit_code"] == 0)


def parse_final(payload, identity=None):
    value = abandon.decode(payload)
    validate_host(value, identity)
    return value


def validate_case(value):
    require(type(value) is dict and set(value) == HOST_FIELDS | EXTERNAL_FIELDS)
    validate_host({field: value[field] for field in HOST_FIELDS})
    case = value["case"]
    native = case in ("abandonment-retains", "death-is-not-exit")
    for field, expected in {
        "process_identities_verified": True, "worker_alive_after_action": case != "death-is-not-exit",
        "native_alive_after_action": native, "native_exit_observed": native,
        "normal_worker_exit_commanded": case != "death-is-not-exit",
        "negative_control_pidfd_kill_verified": case == "death-is-not-exit",
        "resources_closed_after_observed_exit": True,
    }.items():
        require(value[field] is expected)
    require(type(value["host_exit_code"]) is int and value["host_exit_code"] == 0)
    if native:
        require(type(value["native_exit_signal"]) is int and value["native_exit_signal"] == 9
                and value["native_status_source"] == "subreaper")
    else:
        require(value["native_exit_signal"] is None and value["native_status_source"] == "none")


def validate_observation(report):
    require(type(report) is dict and set(report) == REPORT_FIELDS)
    require(type(report["version"]) is int and report["version"] == 1
            and report["experiment"] == "rust-reservation" and report["status"] == "observed")
    require(all(report[field] is False for field in SCOPE_FIELDS))
    require(report["resources_closed_after_observed_exit"] is True and type(report["resource_limits"]) is dict)
    for field in ("helper_sha256", "observer_sha256"):
        require(type(report[field]) is str and len(report[field]) == 64
                and all(byte in "0123456789abcdef" for byte in report[field]))
    cases = report["cases"]
    require(type(cases) is list and len(cases) == len(CASES) and all(type(case) is dict for case in cases)
            and {case.get("case") for case in cases} == CASES)
    for case in cases:
        validate_case(case)


def public_store(path):
    path.mkdir(mode=0o700)
    for name, payload in (("key.openssh", b"public reservation marker A"),
                          ("manifest.json", b'{"fixture":"public reservation marker"}\n')):
        descriptor = os.open(path / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_CLOEXEC, 0o600)
        try:
            require(os.write(descriptor, payload) == len(payload))
        finally:
            os.close(descriptor)


def mutate(case, root, original_a, original_b):
    if case == "replacement-binding":
        os.rename(root / "a", root / "a-old")
        os.rename(root / "b", root / "a")
        require(os.stat(root / "a-old").st_ino == os.fstat(original_a).st_ino
                and os.stat(root / "a").st_ino == os.fstat(original_b).st_ino)
    elif case == "invalidation-retains":
        descriptor = os.open(root / "a" / "key.openssh", os.O_WRONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        try:
            require(os.pwrite(descriptor, b"P", 0) == 1)
        finally:
            os.close(descriptor)


def run_case(helper, store, case, environment):
    root = store / case
    root.mkdir(mode=0o700)
    public_store(root / "a")
    public_store(root / "b")
    directories = [os.open(root / name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
                   for name in ("a", "b")]
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    child_channel, child_peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    for value in (channel, child_channel):
        value.settimeout(5)
    process = None
    pidfds = []
    observed = False
    try:
        helper_path = helper.resolve(strict=True)
        helper_stat = helper_path.stat()
        executable = (helper_path, helper_stat.st_dev, helper_stat.st_ino)
        arguments = [str(helper), "--reservation-case", case]
        descriptors = [peer.fileno(), child_peer.fileno(), *directories]
        for option, descriptor in zip(("--observer-fd", "--child-fd", "--store-a-fd", "--store-b-fd"), descriptors):
            arguments.extend((option, str(descriptor)))
        process = subprocess.Popen(arguments, env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, close_fds=True, pass_fds=tuple(descriptors))
        peer.close()
        child_peer.close()
        before = abandon.packet(channel, IDENTITY_FIELDS | {"case"}, "before")
        require(before["case"] == case)
        identity = {field: before[field] for field in IDENTITY_FIELDS}
        validate_identity(identity, case)
        require(identity["host_pid"] == process.pid)
        host_pidfd = os.pidfd_open(process.pid, 0)
        worker_pidfd = os.pidfd_open(identity["worker_pid"], 0)
        pidfds.extend((host_pidfd, worker_pidfd))
        host_start = isolation.verify_process(process.pid, os.getpid(), executable, host_pidfd)
        worker_start = isolation.verify_process(identity["worker_pid"], process.pid, executable, worker_pidfd)
        native_pidfd = native_start = None
        if identity["native_pid"] is not None:
            native_pidfd = os.pidfd_open(identity["native_pid"], 0)
            pidfds.append(native_pidfd)
            child = abandon.packet(child_channel, {"pid"}, "child-ready")
            require(type(child["pid"]) is int and child["pid"] == identity["native_pid"])
            native_start = isolation.verify_process(identity["native_pid"], identity["worker_pid"], executable, native_pidfd)
            abandon.ping(child_channel, identity["native_pid"])
        mutate(case, root, *directories)
        channel.sendall(b"G")
        envelope = abandon.packet(channel, {"payload"}, "observation")
        action = envelope["payload"]
        validate_host(action, identity, observation=True)
        require(action["case"] == case)
        isolation.verify_process(process.pid, os.getpid(), executable, host_pidfd, host_start)
        death = case == "death-is-not-exit"
        if death:
            require(abandon.pidfd_ready(worker_pidfd, 3))
        else:
            isolation.verify_process(identity["worker_pid"], process.pid, executable, worker_pidfd, worker_start)
        if native_pidfd is not None:
            parent = os.getpid() if death else identity["worker_pid"]
            isolation.verify_process(identity["native_pid"], parent, executable, native_pidfd, native_start)
            abandon.ping(child_channel, identity["native_pid"])
        native_status = None
        if death:
            # Only the verified direct descendant of this private fixture is signalled.
            isolation.verify_process(identity["native_pid"], os.getpid(), executable, native_pidfd, native_start)
            signal.pidfd_send_signal(native_pidfd, signal.SIGKILL, None, 0)
            native_status = launcher.observe_exit(identity["native_pid"], native_pidfd, time.monotonic() + 3)
        channel.sendall(b"G")
        exited = abandon.packet(channel, {"worker_pid", "worker_token", "exit_code", "exit_signal"}, "worker-exited")
        require(type(exited["worker_pid"]) is int and exited["worker_pid"] == identity["worker_pid"]
                and type(exited["worker_token"]) is int and exited["worker_token"] == identity["worker_token"])
        require(abandon.pidfd_ready(worker_pidfd, 3))
        if death:
            require(exited["exit_code"] is None and type(exited["exit_signal"]) is int and exited["exit_signal"] == 9)
        else:
            require(type(exited["exit_code"]) is int and exited["exit_code"] == 0 and exited["exit_signal"] is None)
        if native_pidfd is not None:
            require(abandon.pidfd_ready(native_pidfd, 3))
            if native_status is None:
                native_status = launcher.observe_exit(identity["native_pid"], native_pidfd, time.monotonic() + 3)
            require(os.WIFSIGNALED(native_status) and os.WTERMSIG(native_status) == signal.SIGKILL)
        isolation.verify_process(process.pid, os.getpid(), executable, host_pidfd, host_start)
        channel.sendall(b"G")
        result = parse_final(channel.recv(8193), identity)
        require(result["case"] == case)
        for field in HOST_FIELDS - {"pressure_iterations", "worker_exit_code", "worker_exit_observed", "resources_closed_after_worker_exit"}:
            require(result[field] == action[field], "action changed after external observation")
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0 and parse_final(output, identity) == result)
        result.update(process_identities_verified=True, worker_alive_after_action=not death,
                      native_alive_after_action=native_pidfd is not None, native_exit_observed=native_pidfd is not None,
                      native_exit_signal=9 if native_pidfd is not None else None,
                      native_status_source="subreaper" if native_pidfd is not None else "none", host_exit_code=0,
                      normal_worker_exit_commanded=not death, negative_control_pidfd_kill_verified=death,
                      resources_closed_after_observed_exit=True)
        validate_case(result)
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, child_channel, child_peer, directories, pidfds))
        else:
            for value in (channel, peer, child_channel, child_peer, process.stdout, process.stderr):
                value.close()
            for descriptor in (*directories, *pidfds):
                os.close(descriptor)


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-reservation", status="incomplete", cases=[],
                  resources_closed_after_observed_exit=False)
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_reservation_probe.py")
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        for case in sorted(CASES):
            report["cases"].append(run_case(args.launcher_helper, args.store_root, case, life.private_env(args.store_root)))
        report.update(status="observed", resources_closed_after_observed_exit=True)
        validate_observation(report)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", resources_closed_after_observed_exit=False, failure={"kind": type(error).__name__})
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust reservation experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
