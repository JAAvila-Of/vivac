#!/usr/bin/env python3
"""Observe bounded owner abandonment in an isolated disposable Linux process."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import time

import lifecycle_probe as life
import rust_launcher_probe as launcher
import session_probe as session

CASES = ("admission-limit", "pidfd-failure", "owner-abandon")
HELPER_FIELDS = {"version", "case", "status", "custody_accredited", "product_recovery", "close_ack",
                 "owners_reserved", "fifth_owner_errno", "readmission_after_release", "admission_poisoned",
                 "poison_refusals", "spawn_errno", "shutdown_errno", "drop_elapsed_ns", "marker_retained",
                 "marker_fd", "marker_dev", "marker_ino", "child_pid"}
EXTERNAL_FIELDS = {"drop_response_observed", "helper_responsive_after_drop", "marker_identity_verified",
                   "child_alive_after_drop", "normal_exit_commanded", "helper_exit_code", "child_exit_observed",
                   "child_exit_signal", "child_status_source", "resources_closed_after_exit"}
REPORT_FIELDS = {"version", "experiment", "status", "custody_accredited", "product_recovery", "close_ack",
                 "resources_closed_after_exit", "cases", "helper_sha256", "observer_sha256", "resource_limits"}
UNCERTAIN = []


def require(value, message="abandonment evidence differs"):
    if not value:
        raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate packet field")
        result[key] = value
    return result


def decode(payload):
    require(0 < len(payload) <= 8192, "packet size differs")
    result = json.loads(payload.decode("ascii"), object_pairs_hook=unique_object)
    require(type(result) is dict, "packet is not an object")
    return result


def validate_helper(value, case):
    require(set(value) == HELPER_FIELDS and case in CASES and value["case"] == case)
    require(type(value["version"]) is int and value["version"] == 1 and value["status"] == "observed")
    for field in ("custody_accredited", "product_recovery", "close_ack"):
        require(value[field] is False, "abandonment cannot acknowledge closure or recovery")
    admission, failed = case == "admission-limit", case == "pidfd-failure"
    expected = {"owners_reserved": 4 if admission else 1, "poison_refusals": 0 if admission else 3}
    for field, count in expected.items():
        require(type(value[field]) is int and value[field] == count)
    for field, number in (("fifth_owner_errno", 16 if admission else None),
                          ("spawn_errno", 24 if failed else None), ("shutdown_errno", 110 if failed else None)):
        require(value[field] is None if number is None else type(value[field]) is int and value[field] == number)
    require(value["readmission_after_release"] is admission and value["admission_poisoned"] is (not admission))
    require(type(value["drop_elapsed_ns"]) is int and 0 <= value["drop_elapsed_ns"] <= 0xffffffffffffffff)
    if admission:
        require(value["drop_elapsed_ns"] == 0)
        require(all(value[field] is None for field in ("marker_retained", "marker_fd", "marker_dev", "marker_ino", "child_pid")))
    else:
        require(type(value["marker_retained"]) is bool and (not failed or value["marker_retained"]))
        for field in ("marker_fd", "marker_dev", "marker_ino", "child_pid"):
            require(type(value[field]) is int and value[field] >= (3 if field == "marker_fd" else 0))
        require(value["child_pid"] > 0 and value["marker_ino"] > 0)


def parse_helper(payload, case, stage):
    result = decode(payload)
    require(result.get("stage") == stage and set(result) == HELPER_FIELDS | {"stage"})
    validate_helper({key: value for key, value in result.items() if key != "stage"}, case)
    return result


def validate_case(value):
    require(type(value) is dict and set(value) == HELPER_FIELDS | EXTERNAL_FIELDS)
    case = value.get("case")
    validate_helper({key: value[key] for key in HELPER_FIELDS}, case)
    admission, failed = case == "admission-limit", case == "pidfd-failure"
    require(type(value["helper_exit_code"]) is int and value["helper_exit_code"] == 0)
    for field, expected in (("drop_response_observed", not admission),
                             ("helper_responsive_after_drop", not admission),
                             ("child_alive_after_drop", failed), ("normal_exit_commanded", True),
                             ("child_exit_observed", not admission), ("resources_closed_after_exit", True),
                             ("marker_identity_verified", bool(value["marker_retained"]))):
        require(value[field] is expected)
    if admission:
        require(value["child_exit_signal"] is None and value["child_status_source"] == "none")
    elif failed:
        require(type(value["child_exit_signal"]) is int and value["child_exit_signal"] == 9
                and value["child_status_source"] == "subreaper")
    else:
        require((value["child_exit_signal"] is None and value["child_status_source"] == "pidfd")
                or (type(value["child_exit_signal"]) is int and value["child_exit_signal"] == 9
                    and value["child_status_source"] == "subreaper"))


def validate_observation(report):
    require(type(report) is dict and set(report) == REPORT_FIELDS)
    require(type(report["version"]) is int and report["version"] == 1
            and report["experiment"] == "rust-abandon" and report["status"] == "observed")
    for field in ("custody_accredited", "product_recovery", "close_ack"):
        require(report[field] is False)
    require(report["resources_closed_after_exit"] is True and type(report["resource_limits"]) is dict)
    for field in ("helper_sha256", "observer_sha256"):
        require(type(report[field]) is str and len(report[field]) == 64
                and all(byte in "0123456789abcdef" for byte in report[field]))
    cases = report["cases"]
    require(type(cases) is list and len(cases) == len(CASES)
            and all(type(case) is dict for case in cases) and {case.get("case") for case in cases} == set(CASES))
    for case in cases:
        validate_case(case)


def packet(channel, fields, stage):
    value = decode(channel.recv(8193))
    require(set(value) == set(fields) | {"stage"} and value.get("stage") == stage)
    return value


def pidfd_ready(descriptor, timeout=0):
    return bool(select.select([descriptor], [], [], timeout)[0])


def ping(channel, pid):
    channel.sendall(b"P")
    value = packet(channel, {"pid"}, "child-alive")
    require(type(value["pid"]) is int and value["pid"] == pid)


def marker_matches(process, marker, value):
    expected = os.fstat(marker)
    actual = os.stat("/proc/" + str(process.pid) + "/fd/" + str(value["marker_fd"]))
    require(actual.st_dev == expected.st_dev == value["marker_dev"]
            and actual.st_ino == expected.st_ino == value["marker_ino"], "retained marker identity changed")


def run_case(helper, case, environment):
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    child_channel, child_peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    marker = os.memfd_create("vivac-public-abandon-marker", os.MFD_CLOEXEC)
    os.write(marker, b"public abandonment marker")
    process = None
    pidfd = None
    observed = False
    for value in (channel, child_channel):
        value.settimeout(3)
    try:
        process = subprocess.Popen([str(helper), "--abandon-case", case, "--observer-fd", str(peer.fileno()),
                                    "--child-fd", str(child_peer.fileno()), "--marker-fd", str(marker)],
                                   env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, close_fds=True,
                                   pass_fds=(peer.fileno(), child_peer.fileno(), marker))
        peer.close()
        child_peer.close()
        if case == "admission-limit":
            ready = packet(channel, {"case"}, "admission-ready")
            require(ready["case"] == case)
            channel.sendall(b"G")
            response = parse_helper(channel.recv(8193), case, "responsive")
        else:
            ready = packet(channel, {"case", "pid", "shutdown_errno"}, "before-drop")
            require(ready["case"] == case and type(ready["pid"]) is int and ready["pid"] > 0)
            require(ready["shutdown_errno"] == (110 if case == "pidfd-failure" else None))
            child = packet(child_channel, {"pid"}, "child-ready")
            require(type(child["pid"]) is int and child["pid"] == ready["pid"])
            pidfd = os.pidfd_open(child["pid"], 0)
            require(not pidfd_ready(pidfd))
            ping(child_channel, child["pid"])
            channel.sendall(b"G")
            dropped = parse_helper(channel.recv(8193), case, "drop-returned")
            require(dropped["child_pid"] == child["pid"] and process.poll() is None)
            if dropped["marker_retained"]:
                marker_matches(process, marker, dropped)
            if case == "pidfd-failure":
                require(not pidfd_ready(pidfd))
                ping(child_channel, child["pid"])
            else:
                require(pidfd_ready(pidfd, 3), "abandoned pidfd child did not exit")
            channel.sendall(b"G")
            response = parse_helper(channel.recv(8193), case, "responsive")
            require({key: value for key, value in dropped.items() if key != "stage"}
                    == {key: value for key, value in response.items() if key != "stage"})
            require(process.poll() is None, "Drop response required process termination")
            if case == "pidfd-failure":
                require(not pidfd_ready(pidfd))
                ping(child_channel, child["pid"])
                marker_matches(process, marker, response)
        channel.sendall(b"X")
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0, "helper did not return normally")
        helper_value = decode(output)
        require(helper_value == {key: value for key, value in response.items() if key != "stage"})
        result = dict(helper_value, drop_response_observed=case != "admission-limit",
                      helper_responsive_after_drop=case != "admission-limit",
                      marker_identity_verified=bool(helper_value["marker_retained"]),
                      child_alive_after_drop=case == "pidfd-failure", normal_exit_commanded=True,
                      helper_exit_code=0, child_exit_observed=case != "admission-limit",
                      child_exit_signal=None, child_status_source="none", resources_closed_after_exit=True)
        if pidfd is not None:
            require(pidfd_ready(pidfd, 3), "descendant remained alive after normal helper exit")
            try:
                status = launcher.observe_exit(helper_value["child_pid"], pidfd, time.monotonic() + 3)
            except launcher.ProbeError:
                require(case == "owner-abandon", "missing descendant parent-death status")
                result["child_status_source"] = "pidfd"
            else:
                require(os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL)
                result.update(child_exit_signal=9, child_status_source="subreaper")
        validate_case(result)
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, child_channel, child_peer, marker, pidfd))
        else:
            for value in (channel, peer, child_channel, child_peer, process.stdout, process.stderr):
                value.close()
            os.close(marker)
            if pidfd is not None:
                os.close(pidfd)


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-abandon", "status": "incomplete", "cases": [],
              "custody_accredited": False, "product_recovery": False, "close_ack": False,
              "resources_closed_after_exit": False}
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_abandon_probe.py")
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        environment = life.private_env(args.store_root)
        for case in CASES:
            report["cases"].append(run_case(args.launcher_helper, case, environment))
        report.update(status="observed", resources_closed_after_exit=True)
        validate_observation(report)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", resources_closed_after_exit=False, failure={"kind": type(error).__name__})
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust abandonment experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
