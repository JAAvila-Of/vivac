"""Observe candidate metadata persistence without granting creation admission."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_isolation_probe as isolation
import rust_launcher_probe as launcher
import session_probe as session

CASES = {"roundtrip", "existing-destination", "concurrent-publishers", "snapshot-in-place",
         "snapshot-replacement", "symlink-refused", "hardlink-refused", "pre-rename-error",
         "post-rename-sync-error", "crash-before-rename", "crash-after-rename", "acl-refused"}
SCOPE_FIELDS = {"custody_accredited", "creation_admission", "producer_origin_checked", "root_bootstrap_accredited",
                "endpoint_authority_accredited", "product_recovery", "same_store_recovery", "mcp_integration",
                "server_integration", "lock_ack"}
ROOT_FIELDS = SCOPE_FIELDS | {"version", "case", "status", "identity", "successful_publications", "unpublished_refusals",
    "metadata_visible", "snapshot_invalidated", "alias_checks", "access_policy_checks", "reader_refused", "existing_unchanged",
    "durability_indeterminate", "injected_failure", "child_exits", "owned_exits_observed", "resources_closed_after_exit",
    "pressure_threads", "pressure_iterations"}
EXTERNAL_FIELDS = {"process_identities_verified", "host_retained_until_observation", "children_observed_alive_before_action",
    "children_retained_until_observation", "children_exits_observed_externally", "host_exit_code", "resources_closed_after_observed_exit"}
REPORT_FIELDS = SCOPE_FIELDS | {"version", "experiment", "status", "cases", "resource_limits", "kernel_release",
    "resources_closed_after_observed_exit", "metadata_store_observed", "helper_sha256", "observer_sha256"}
UNCERTAIN = []
require = abandon.require

def positive(value):
    require(type(value) is int and 0 < value <= 0xffffffffffffffff)

def validate_identity(value, case, expected=None):
    require(type(value) is dict and set(value) == {"host_pid", "creator_tid", "children"})
    positive(value["host_pid"])
    require(type(value["creator_tid"]) is int and value["creator_tid"] == value["host_pid"])
    count = 2 if case == "concurrent-publishers" else int(case.startswith("crash-"))
    require(type(value["children"]) is list and len(value["children"]) == count)
    pids, tokens = {value["host_pid"]}, set()
    for ordinal, child in enumerate(value["children"], 1):
        require(type(child) is dict and set(child) == {"pid", "token", "ordinal"})
        positive(child["pid"])
        positive(child["token"])
        require(type(child["ordinal"]) is int and child["ordinal"] == ordinal
                and child["pid"] not in pids and child["token"] not in tokens)
        pids.add(child["pid"])
        tokens.add(child["token"])
    require(expected is None or value == expected, "metadata publisher identity changed")

def validate_result(value, case, identity=None, *, observation=False):
    require(type(value) is dict and set(value) == ROOT_FIELDS)
    require(case in CASES and value["case"] == case and value["status"] == "observed")
    validate_identity(value["identity"], case, identity)
    require(all(value[field] is False for field in SCOPE_FIELDS), "metadata storage cannot establish admission")
    successful = case in {"roundtrip", "existing-destination", "concurrent-publishers", "snapshot-in-place",
                          "snapshot-replacement", "symlink-refused", "hardlink-refused", "acl-refused"}
    refused = case in {"existing-destination", "concurrent-publishers", "pre-rename-error",
                       "post-rename-sync-error", "crash-after-rename"}
    counts = {"version": 1, "successful_publications": int(successful), "unpublished_refusals": int(refused),
              "alias_checks": 2 if case == "symlink-refused" else 1 if case == "hardlink-refused" else 0,
              "access_policy_checks": 3 if case == "acl-refused" else 0,
              "pressure_threads": 4}
    for field, expected in counts.items():
        require(type(value[field]) is int and value[field] == expected, "metadata counter differs: " + field)
    expected = {
        "metadata_visible": case not in {"pre-rename-error", "crash-before-rename"},
        "snapshot_invalidated": case in {"snapshot-in-place", "snapshot-replacement"},
        "reader_refused": case in {"symlink-refused", "hardlink-refused", "pre-rename-error", "crash-before-rename", "acl-refused"},
        "existing_unchanged": case in {"existing-destination", "post-rename-sync-error", "crash-after-rename"},
        "durability_indeterminate": case in {"post-rename-sync-error", "crash-after-rename"},
        "owned_exits_observed": not observation or case.startswith("crash-"),
        "resources_closed_after_exit": not observation,
    }
    for field, state in expected.items():
        require(value[field] is state, "metadata evidence differs: " + field)
    injection = "before-rename" if case == "pre-rename-error" else "before-parent-sync" if case == "post-rename-sync-error" else "none"
    require(value["injected_failure"] == injection)
    require(type(value["pressure_iterations"]) is int and 0 <= value["pressure_iterations"] <= 0xffffffffffffffff)
    require(value["pressure_iterations"] == 0 if observation else value["pressure_iterations"] > 0)
    children = value["identity"]["children"]
    expected_children = [] if observation and not case.startswith("crash-") else children
    require(type(value["child_exits"]) is list and len(value["child_exits"]) == len(expected_children))
    for child, exited in zip(expected_children, value["child_exits"]):
        require(type(exited) is dict and set(exited) == {"pid", "token", "code", "signal"})
        require(type(exited["pid"]) is int and exited["pid"] == child["pid"]
                and type(exited["token"]) is int and exited["token"] == child["token"])
        if case.startswith("crash-"):
            require(exited["code"] is None and type(exited["signal"]) is int and exited["signal"] == 9)
        else:
            require(type(exited["code"]) is int and exited["code"] == 0 and exited["signal"] is None)

def validate_case(value):
    require(type(value) is dict and set(value) == ROOT_FIELDS | EXTERNAL_FIELDS)
    case = value["case"]
    validate_result({field: value[field] for field in ROOT_FIELDS}, case)
    for field, expected in {
        "process_identities_verified": True, "host_retained_until_observation": True,
        "children_observed_alive_before_action": bool(value["identity"]["children"]),
        "children_retained_until_observation": case == "concurrent-publishers",
        "children_exits_observed_externally": True, "resources_closed_after_observed_exit": True,
    }.items():
        require(value[field] is expected)
    require(type(value["host_exit_code"]) is int and value["host_exit_code"] == 0)

def validate_observation(value):
    require(type(value) is dict and set(value) == REPORT_FIELDS)
    require(value["version"] == 1 and type(value["version"]) is int and value["experiment"] == "rust-admission-storage"
            and value["status"] == "observed")
    require(all(value[field] is False for field in SCOPE_FIELDS))
    require(value["metadata_store_observed"] is True and value["resources_closed_after_observed_exit"] is True)
    require(type(value["cases"]) is list and len(value["cases"]) == len(CASES)
            and {case.get("case") for case in value["cases"]} == CASES)
    for case in value["cases"]:
        validate_case(case)
    for field in ("helper_sha256", "observer_sha256"):
        require(type(value[field]) is str and len(value[field]) == 64 and all(c in "0123456789abcdef" for c in value[field]))
    require(type(value["kernel_release"]) is str and 0 < len(value["kernel_release"]) <= 128
            and all(33 <= ord(c) <= 126 for c in value["kernel_release"]))
    limits = value["resource_limits"]
    require(type(limits) is dict and set(limits) == {"memory_bytes", "swap_bytes", "pids", "cpu_quota", "cpu_period", "temporary_bytes"})
    for field in limits:
        require(type(limits[field]) is int)
    require(0 < limits["memory_bytes"] <= 536870912 and limits["swap_bytes"] == 0
            and 0 < limits["pids"] <= 128 and 0 < limits["cpu_quota"] <= limits["cpu_period"]
            and 0 < limits["temporary_bytes"] <= 134217728)

def parse_result(payload, case, identity=None, *, observation=False):
    value = abandon.decode(payload)
    validate_result(value, case, identity, observation=observation)
    return value

def run_case(helper, store, case, environment):
    root = store / case
    root.mkdir(mode=0o700)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    channel.settimeout(8)
    process = None
    pidfds = []
    observed = False
    try:
        resolved = helper.resolve(strict=True)
        metadata = resolved.stat()
        executable = (resolved, metadata.st_dev, metadata.st_ino)
        process = subprocess.Popen([str(helper), "--admission-storage-case", case,
            "--observer-fd", str(peer.fileno()), "--root-fd", str(descriptor)],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment,
            close_fds=True, pass_fds=(peer.fileno(), descriptor))
        peer.close()
        before = abandon.packet(channel, {"case", "identity"}, "before")
        require(before["case"] == case)
        identity = before["identity"]
        validate_identity(identity, case)
        require(identity["host_pid"] == process.pid)
        host_fd = os.pidfd_open(process.pid, 0)
        pidfds.append(host_fd)
        host_start = isolation.verify_process(process.pid, os.getpid(), executable, host_fd)
        child_state = []
        for child in identity["children"]:
            pidfd = os.pidfd_open(child["pid"], 0)
            pidfds.append(pidfd)
            started = isolation.verify_process(child["pid"], process.pid, executable, pidfd)
            child_state.append((child, pidfd, started))
        channel.sendall(b"G")
        envelope = abandon.packet(channel, {"payload"}, "observation")
        action = envelope["payload"]
        validate_result(action, case, identity, observation=True)
        isolation.verify_process(process.pid, os.getpid(), executable, host_fd, host_start)
        for child, pidfd, started in child_state:
            if case.startswith("crash-"):
                require(abandon.pidfd_ready(pidfd, 3))
            else:
                isolation.verify_process(child["pid"], process.pid, executable, pidfd, started)
        channel.sendall(b"G")
        result = parse_result(channel.recv(8193), case, identity)
        for field in ROOT_FIELDS - {"child_exits", "owned_exits_observed", "resources_closed_after_exit", "pressure_iterations"}:
            require(result[field] == action[field], "metadata evidence changed after observation")
        for _, pidfd, _ in child_state:
            require(abandon.pidfd_ready(pidfd, 3))
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0 and parse_result(output, case, identity) == result)
        result.update(process_identities_verified=True, host_retained_until_observation=True,
            children_observed_alive_before_action=bool(child_state), children_retained_until_observation=case == "concurrent-publishers",
            children_exits_observed_externally=True, host_exit_code=0, resources_closed_after_observed_exit=True)
        validate_case(result)
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, descriptor, pidfds))
        else:
            for value in (channel, peer, process.stdout, process.stderr):
                value.close()
            for value in (descriptor, *pidfds):
                os.close(value)

def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-admission-storage", status="incomplete", cases=[],
                  resources_closed_after_observed_exit=False, metadata_store_observed=False)
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        report["kernel_release"] = os.uname().release
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_admission_storage_probe.py")
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        for case in sorted(CASES):
            report["cases"].append(run_case(args.launcher_helper, args.store_root, case, life.private_env(args.store_root)))
        report.update(status="observed", resources_closed_after_observed_exit=True, metadata_store_observed=True)
        validate_observation(report)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", resources_closed_after_observed_exit=False, metadata_store_observed=False,
                      failure={"kind": type(error).__name__})
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust admission storage experiment failed.", file=sys.stderr)
        return 1

if __name__ == "__main__":
    sys.exit(main())
