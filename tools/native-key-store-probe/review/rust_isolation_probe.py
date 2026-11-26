#!/usr/bin/env python3
"""Observe process-local quarantine without claiming product recovery."""
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
from types import SimpleNamespace

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_launcher_probe as launcher
import session_probe as session

CASES = {"process-boundary"}
SCOPE_FIELDS = {"custody_accredited", "product_recovery", "same_store_recovery",
                "mcp_integration", "server_integration", "close_ack"}
IDENTITY_FIELDS = {"host_pid", "creator_tid", "worker_a_pid", "worker_b_pid", "native_pid"}
HOST_FIELDS = IDENTITY_FIELDS | SCOPE_FIELDS | {
    "version", "case", "status", "host_admission_uncertain", "host_empty_owner_admitted", "host_health_responses",
    "worker_a_admission_poisoned", "worker_a_poison_refusals", "worker_a_drop_elapsed_ns",
    "worker_b_reserved", "worker_b_fifth_errno", "worker_b_readmitted", "worker_a_exit_code",
    "worker_b_exit_code", "worker_exits_observed", "pressure_threads", "pressure_iterations",
    "resources_closed_after_worker_exit"}
EXTERNAL_FIELDS = {"worker_a_marker_identity_verified", "native_alive_while_worker_b_completed",
                   "worker_a_alive_while_worker_b_completed", "native_exit_signal", "native_exit_observed",
                   "host_exit_code", "normal_worker_exit_commanded", "host_responsive_after_native_exit",
                   "resources_closed_after_observed_exit"}
REPORT_FIELDS = SCOPE_FIELDS | {"version", "experiment", "status", "cases", "resources_closed_after_observed_exit",
                              "resource_limits", "helper_sha256", "observer_sha256"}
UNCERTAIN = []
require = abandon.require


def validate_host(value, identity=None):
    require(type(value) is dict and set(value) == HOST_FIELDS, "host result schema differs")
    require(value["case"] == "process-boundary" and value["status"] == "observed")
    for field in SCOPE_FIELDS | {"host_admission_uncertain"}:
        require(value[field] is False, "process isolation cannot acknowledge custody or recovery")
    for field in ("host_empty_owner_admitted", "worker_a_admission_poisoned", "worker_b_readmitted",
                  "worker_exits_observed", "resources_closed_after_worker_exit"):
        require(value[field] is True)
    counts = {"version": 1, "host_health_responses": 2, "worker_a_poison_refusals": 3,
              "worker_b_reserved": 4, "worker_b_fifth_errno": 16, "worker_a_exit_code": 0,
              "worker_b_exit_code": 0, "pressure_threads": 4}
    for field, expected in counts.items():
        require(type(value[field]) is int and value[field] == expected)
    for field in IDENTITY_FIELDS | {"worker_a_drop_elapsed_ns", "pressure_iterations"}:
        require(type(value[field]) is int and 0 < value[field] <= 0xffffffffffffffff)
    require(value["creator_tid"] == value["host_pid"]
            and len({value[field] for field in IDENTITY_FIELDS - {"creator_tid"}}) == 4,
            "host or native ownership identity differs")
    if identity is not None:
        require(set(identity) == IDENTITY_FIELDS and all(value[field] == identity[field] for field in IDENTITY_FIELDS))


def parse_final(payload, identity):
    result = abandon.decode(payload)
    validate_host(result, identity)
    return result


def validate_observation(report):
    require(type(report) is dict and set(report) == REPORT_FIELDS)
    require(type(report["version"]) is int and report["version"] == 1
            and report["experiment"] == "rust-isolation" and report["status"] == "observed")
    require(all(report[field] is False for field in SCOPE_FIELDS))
    require(report["resources_closed_after_observed_exit"] is True and type(report["resource_limits"]) is dict)
    for field in ("helper_sha256", "observer_sha256"):
        require(type(report[field]) is str and len(report[field]) == 64
                and all(byte in "0123456789abcdef" for byte in report[field]))
    require(type(report["cases"]) is list and len(report["cases"]) == 1)
    value = report["cases"][0]
    require(type(value) is dict and set(value) == HOST_FIELDS | EXTERNAL_FIELDS)
    validate_host({field: value[field] for field in HOST_FIELDS})
    for field in EXTERNAL_FIELDS - {"native_exit_signal", "host_exit_code"}:
        require(value[field] is True)
    require(type(value["native_exit_signal"]) is int and value["native_exit_signal"] == 9
            and type(value["host_exit_code"]) is int and value["host_exit_code"] == 0)


def packet(channel, fields, stage):
    return abandon.packet(channel, fields, stage)


def worker_a(channel, stage, worker_pid=None):
    envelope = packet(channel, {"worker_pid", "payload"}, "worker-a")
    require(type(envelope["worker_pid"]) is int and envelope["worker_pid"] > 0
            and (worker_pid is None or envelope["worker_pid"] == worker_pid))
    payload = envelope["payload"]
    if stage == "before-drop":
        require(type(payload) is dict and set(payload) == {"stage", "case", "pid", "shutdown_errno"}
                and payload["stage"] == stage and payload["case"] == "pidfd-failure"
                and type(payload["pid"]) is int and payload["pid"] > 0
                and type(payload["shutdown_errno"]) is int and payload["shutdown_errno"] == 110)
    else:
        abandon.parse_helper(json.dumps(payload).encode("ascii"), "pidfd-failure", stage)
    return envelope["worker_pid"], payload


def health(channel, stage, host_pid, worker_pid, responses, creator_tid=None):
    fields = {"host_pid", "creator_tid", "worker_a_pid", "host_admission_uncertain",
              "host_empty_owner_admitted", "pressure_threads", "health_responses"}
    value = packet(channel, fields, stage)
    for field, expected in (("host_pid", host_pid), ("worker_a_pid", worker_pid),
                             ("creator_tid", host_pid), ("pressure_threads", 4), ("health_responses", responses)):
        require(type(value[field]) is int and value[field] == expected)
    require(value["host_admission_uncertain"] is False and value["host_empty_owner_admitted"] is True)
    require(creator_tid is None or creator_tid == value["creator_tid"])
    return value["creator_tid"]


def verify_process(pid, parent, executable, pidfd, expected_start=None, *, proc_root=Path("/proc"), uid=None, gid=None):
    require(type(pid) is int and pid > 0 and type(parent) is int and parent > 0)
    require(not abandon.pidfd_ready(pidfd), "bound process exited before inspection")
    started = life.process_start(pid)
    require(type(started) is int and started > 0 and (expected_start is None or started == expected_start),
            "bound process start identity changed")
    uid = os.geteuid() if uid is None else uid
    gid = os.getegid() if gid is None else gid
    directory = proc_root / str(pid)
    with (directory / "status").open("rb") as source:
        status = source.read(16385)
    require(len(status) <= 16384, "process status exceeded its bound")
    credentials = {}
    required = {"PPid", "Uid", "Gid", "NoNewPrivs", "CapEff"}
    for line in status.decode("ascii").splitlines():
        key, separator, value = line.partition(":")
        if separator and key in required:
            require(key not in credentials, "duplicate process credential field")
            credentials[key] = [int(item, 16 if key == "CapEff" else 10) for item in value.split()]
    require(set(credentials) == required and credentials["PPid"] == [parent]
            and credentials["Uid"] == [uid] * 4 and credentials["Gid"] == [gid] * 4
            and credentials["NoNewPrivs"] == [1] and credentials["CapEff"] == [0],
            "process parent or credentials differ")
    path = directory / "exe"
    before = path.stat()
    require((before.st_dev, before.st_ino) == executable[1:] and path.resolve(strict=True) == executable[0],
            "process executable differs from the admitted helper")
    after = path.stat()
    require((after.st_dev, after.st_ino) == executable[1:]
            and life.process_start(pid) == started and not abandon.pidfd_ready(pidfd),
            "bound process changed during inspection")
    return started


def run_case(helper, environment):
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    native_channel, native_peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    marker = os.memfd_create("vivac-public-isolation-marker", os.MFD_CLOEXEC)
    os.write(marker, b"public isolation marker")
    channel.settimeout(5)
    native_channel.settimeout(3)
    process = None
    pidfds = []
    observed = False
    try:
        helper_path = helper.resolve(strict=True)
        helper_stat = helper_path.stat()
        executable = (helper_path, helper_stat.st_dev, helper_stat.st_ino)
        process = subprocess.Popen([str(helper), "--isolation-case", "process-boundary",
                                   "--observer-fd", str(peer.fileno()), "--child-fd", str(native_peer.fileno()),
                                   "--marker-fd", str(marker)], env=environment, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, close_fds=True,
                                   pass_fds=(peer.fileno(), native_peer.fileno(), marker))
        peer.close()
        native_peer.close()
        a_pid, before = worker_a(channel, "before-drop")
        native_pid = before["pid"]
        require(len({process.pid, a_pid, native_pid}) == 3)
        a_pidfd = os.pidfd_open(a_pid, 0)
        pidfds.append(a_pidfd)
        native_pidfd = os.pidfd_open(native_pid, 0)
        pidfds.append(native_pidfd)
        child = packet(native_channel, {"pid"}, "child-ready")
        require(type(child["pid"]) is int and child["pid"] == native_pid)
        require(not abandon.pidfd_ready(a_pidfd) and not abandon.pidfd_ready(native_pidfd))
        a_start = verify_process(a_pid, process.pid, executable, a_pidfd)
        native_start = verify_process(native_pid, a_pid, executable, native_pidfd)

        def verify_bound_processes():
            verify_process(a_pid, process.pid, executable, a_pidfd, a_start)
            verify_process(native_pid, a_pid, executable, native_pidfd, native_start)

        abandon.ping(native_channel, native_pid)
        channel.sendall(b"G")
        _, dropped = worker_a(channel, "drop-returned", a_pid)
        require(dropped["child_pid"] == native_pid)
        abandon.marker_matches(SimpleNamespace(pid=a_pid), marker, dropped)
        require(not abandon.pidfd_ready(a_pidfd) and not abandon.pidfd_ready(native_pidfd))
        verify_bound_processes()
        abandon.ping(native_channel, native_pid)
        channel.sendall(b"H")
        creator_tid = health(channel, "host-healthy", process.pid, a_pid, 1)
        completed = packet(channel, {"worker_pid", "payload", "exit_code"}, "worker-b-completed")
        b_pid = completed["worker_pid"]
        require(type(b_pid) is int and b_pid > 0 and len({process.pid, a_pid, native_pid, b_pid}) == 4)
        require(type(completed["exit_code"]) is int and completed["exit_code"] == 0)
        b_report = abandon.parse_helper(json.dumps(completed["payload"]).encode("ascii"), "admission-limit", "responsive")
        require(not abandon.pidfd_ready(a_pidfd) and not abandon.pidfd_ready(native_pidfd)
                and process.poll() is None, "worker quarantine escaped its process boundary")
        verify_bound_processes()
        abandon.ping(native_channel, native_pid)
        abandon.marker_matches(SimpleNamespace(pid=a_pid), marker, dropped)
        channel.sendall(b"G")
        _, responsive = worker_a(channel, "responsive", a_pid)
        require({key: value for key, value in responsive.items() if key != "stage"}
                == {key: value for key, value in dropped.items() if key != "stage"})
        require(not abandon.pidfd_ready(a_pidfd) and not abandon.pidfd_ready(native_pidfd))
        verify_bound_processes()
        abandon.ping(native_channel, native_pid)
        abandon.marker_matches(SimpleNamespace(pid=a_pid), marker, responsive)
        channel.sendall(b"G")
        exited = packet(channel, {"worker_a_pid", "worker_b_pid", "worker_a_exit_code", "worker_b_exit_code"}, "workers-exited")
        for field, expected in (("worker_a_pid", a_pid), ("worker_b_pid", b_pid),
                                 ("worker_a_exit_code", 0), ("worker_b_exit_code", 0)):
            require(type(exited[field]) is int and exited[field] == expected)
        require(abandon.pidfd_ready(a_pidfd, 3) and abandon.pidfd_ready(native_pidfd, 3))
        status = launcher.observe_exit(native_pid, native_pidfd, time.monotonic() + 3)
        require(os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL)
        require(process.poll() is None, "host response depended on its own termination")
        channel.sendall(b"H")
        health(channel, "host-health", process.pid, a_pid, 2, creator_tid)
        channel.sendall(b"C")
        identity = {"host_pid": process.pid, "creator_tid": creator_tid, "worker_a_pid": a_pid,
                    "worker_b_pid": b_pid, "native_pid": native_pid}
        result = parse_final(channel.recv(8193), identity)
        require(result["worker_a_drop_elapsed_ns"] == dropped["drop_elapsed_ns"]
                and result["worker_b_reserved"] == b_report["owners_reserved"])
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0 and parse_final(output, identity) == result)
        result.update(worker_a_marker_identity_verified=True, native_alive_while_worker_b_completed=True,
                      worker_a_alive_while_worker_b_completed=True, native_exit_signal=9, native_exit_observed=True,
                      host_exit_code=0, normal_worker_exit_commanded=True, host_responsive_after_native_exit=True,
                      resources_closed_after_observed_exit=True)
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, native_channel, native_peer, marker, pidfds))
        else:
            for value in (channel, peer, native_channel, native_peer, process.stdout, process.stderr):
                value.close()
            for descriptor in (marker, *pidfds):
                os.close(descriptor)


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-isolation", status="incomplete", cases=[],
                  resources_closed_after_observed_exit=False)
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_isolation_probe.py")
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        report["cases"].append(run_case(args.launcher_helper, life.private_env(args.store_root)))
        report.update(status="observed", resources_closed_after_observed_exit=True)
        validate_observation(report)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", resources_closed_after_observed_exit=False, failure={"kind": type(error).__name__})
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust isolation experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
