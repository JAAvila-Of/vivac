"""Observe a candidate root reader against administrator-owned disposable fixtures."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import sys
import time

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_isolation_probe as isolation
import rust_launcher_probe as launcher
import session_probe as session

CASES = {"valid", "wrong-root-owner", "writable-ancestor", "wrong-prefix-mode", "wrong-slot-owner",
         "binding-owner", "binding-mode", "binding-malformed", "binding-oversized", "binding-identity",
         "ancestor-symlink", "binding-symlink", "missing-binding", "state-mode", "state-acl",
         "snapshot-private-replacement", "unrelated-entry", "binding-hardlink", "binding-fifo"}
SCOPE_FIELDS = {"custody_accredited", "creation_admission", "producer_origin_checked", "root_bootstrap_accredited",
                "provisioning_accredited", "endpoint_authority_accredited", "product_recovery", "same_store_recovery",
                "mcp_integration", "server_integration", "lock_ack"}
ROOT_FIELDS = SCOPE_FIELDS | {"version", "case", "status", "identity", "reader_opened", "snapshot_invalidated",
    "access_policy_checks", "resources_closed_after_exit", "pressure_threads", "pressure_iterations"}
EXTERNAL_FIELDS = {"process_identities_verified", "host_retained_until_observation", "host_exit_code",
    "resources_closed_after_observed_exit", "fixture_unchanged", "binding_unchanged", "fixture_owners_verified"}
REPORT_FIELDS = SCOPE_FIELDS | {"version", "experiment", "status", "cases", "resource_limits", "kernel_release",
    "resources_closed_after_observed_exit", "candidate_root_reader_observed", "helper_sha256", "observer_sha256"}
UNCERTAIN = []
require = abandon.require
DIAGNOSTIC_STAGES = {"setup", "acl-prepare", "baseline-snapshot", "root-open", "observer-create", "helper-start",
    "before-packet", "before-validation", "host-identity", "start-action", "action-packet", "action-validation",
    "host-revalidation", "after-snapshot", "fixture-comparison", "finish-action", "final-packet",
    "evidence-comparison", "host-exit", "stdout-validation", "final-snapshot", "case-validation",
    "report-validation", "report-write", "fixture-owners"}

class ReaderRefusal(ValueError):
    def __init__(self, diagnostic):
        require(type(diagnostic) is str and 0 < len(diagnostic) <= 512 and diagnostic.isascii())
        self.diagnostic = diagnostic
        super().__init__("candidate reader refused")

def parse_action_packet(payload):
    value = abandon.decode(payload)
    if set(value) == {"stage", "diagnostic"} and value["stage"] == "reader-refusal":
        raise ReaderRefusal(value["diagnostic"])
    require(set(value) == {"stage", "payload"} and value["stage"] == "observation")
    return value["payload"]

def failure_diagnostic(error, diagnostic):
    kind = type(error).__name__
    value = {"kind":kind if kind in {"ValueError", "OSError", "TimeoutError", "FileNotFoundError", "ReaderRefusal",
            "PermissionError", "BrokenPipeError", "ConnectionResetError", "ChildProcessError"} else "Exception",
            "case":diagnostic.get("case") if diagnostic.get("case") in CASES else "none",
            "stage":diagnostic.get("stage") if diagnostic.get("stage") in DIAGNOSTIC_STAGES else "unknown"}
    if type(error) is ReaderRefusal:
        value["diagnostic"] = error.diagnostic
    return value

def validate_identity(value, expected=None):
    require(type(value) is dict and set(value) == {"host_pid", "creator_tid", "children"})
    require(type(value["host_pid"]) is int and 0 < value["host_pid"] <= 0xffffffffffffffff)
    require(type(value["creator_tid"]) is int and value["creator_tid"] == value["host_pid"])
    require(type(value["children"]) is list and value["children"] == [])
    require(expected is None or value == expected, "candidate reader identity changed")

def validate_result(value, case, identity=None, *, observation=False):
    require(type(value) is dict and set(value) == ROOT_FIELDS)
    require(case in CASES and value["case"] == case and value["status"] == "observed")
    validate_identity(value["identity"], identity)
    require(all(value[field] is False for field in SCOPE_FIELDS), "candidate reading cannot establish operational admission")
    for field, count in {"version":1,"pressure_threads":4,"access_policy_checks":int(case == "state-acl")}.items():
        require(type(value[field]) is int and value[field] == count)
    for field, expected in {
        "reader_opened":case in {"valid", "snapshot-private-replacement", "unrelated-entry"},
        "snapshot_invalidated":case == "snapshot-private-replacement",
        "resources_closed_after_exit":not observation,
    }.items():
        require(value[field] is expected, "candidate reader evidence differs: " + field)
    require(type(value["pressure_iterations"]) is int and 0 <= value["pressure_iterations"] <= 0xffffffffffffffff)
    require(value["pressure_iterations"] == 0 if observation else value["pressure_iterations"] > 0)

def validate_case(value):
    require(type(value) is dict and set(value) == ROOT_FIELDS | EXTERNAL_FIELDS)
    validate_result({field:value[field] for field in ROOT_FIELDS}, value["case"])
    for field in EXTERNAL_FIELDS - {"host_exit_code", "fixture_unchanged"}:
        require(value[field] is True)
    require(value["fixture_unchanged"] is (value["case"] != "snapshot-private-replacement"))
    require(type(value["host_exit_code"]) is int and value["host_exit_code"] == 0)

def validate_observation(value):
    require(type(value) is dict and set(value) == REPORT_FIELDS)
    require(type(value["version"]) is int and value["version"] == 1 and value["experiment"] == "rust-root-reader"
            and value["status"] == "observed")
    require(all(value[field] is False for field in SCOPE_FIELDS))
    require(value["candidate_root_reader_observed"] is True and value["resources_closed_after_observed_exit"] is True)
    require(type(value["cases"]) is list and len(value["cases"]) == len(CASES)
            and all(type(case) is dict for case in value["cases"])
            and {case.get("case") for case in value["cases"]} == CASES)
    for case in value["cases"]: validate_case(case)
    for field in ("helper_sha256", "observer_sha256"):
        require(type(value[field]) is str and len(value[field]) == 64 and all(c in "0123456789abcdef" for c in value[field]))
    require(type(value["kernel_release"]) is str and 0 < len(value["kernel_release"]) <= 128
            and all(33 <= ord(c) <= 126 for c in value["kernel_release"]))
    limits = value["resource_limits"]
    require(type(limits) is dict and set(limits) == {"memory_bytes", "swap_bytes", "pids", "cpu_quota", "cpu_period", "temporary_bytes"})
    require(all(type(limits[field]) is int for field in limits))
    require(0 < limits["memory_bytes"] <= 536870912 and limits["swap_bytes"] == 0
            and 0 < limits["pids"] <= 128 and 0 < limits["cpu_quota"] <= limits["cpu_period"]
            and 0 < limits["temporary_bytes"] <= 134217728)

def parse_result(payload, case, identity=None, *, observation=False):
    value = abandon.decode(payload)
    validate_result(value, case, identity, observation=observation)
    return value

def fixture_snapshot(root):
    # Compare selected metadata and bounded regular bytes, excluding access times
    # and directory timestamps. This observation is not a syscall trace.
    entries = {}
    def visit(path, relative):
        require(len(entries) < 64)
        info = path.lstat()
        metadata = (info.st_mode, info.st_uid, info.st_gid, info.st_dev, info.st_ino, info.st_nlink)
        contents = None
        if stat.S_ISREG(info.st_mode):
            require(info.st_size <= 1024)
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK)
            try:
                require(os.fstat(fd).st_ino == info.st_ino and os.fstat(fd).st_dev == info.st_dev)
                contents = os.read(fd, 1025)
                require(len(contents) == info.st_size)
            finally: os.close(fd)
        elif stat.S_ISLNK(info.st_mode):
            contents = os.readlink(path)
        else:
            require(stat.S_ISDIR(info.st_mode) or stat.S_ISFIFO(info.st_mode))
        entries[relative] = (metadata, contents)
        if stat.S_ISDIR(info.st_mode):
            for child in sorted(path.iterdir()): visit(child, relative + "/" + child.name)
    visit(root, ".")
    return entries

def verify_fixture_owners(snapshot, case):
    require(case in CASES and "." in snapshot)
    lib = "lib-real" if case == "ancestor-symlink" else "lib"
    slot = "./var/" + lib + "/vivac/custody-v1/users/65534"
    state = slot + "/state"
    exceptions = {"wrong-root-owner":".", "wrong-slot-owner":slot, "binding-owner":slot + "/binding.json"}
    for relative, (metadata, _) in snapshot.items():
        private = relative == state or relative.startswith(state + "/")
        owner = 65534 if private or relative == exceptions.get(case) else 0
        require(metadata[1] == metadata[2] == owner, "fixture ownership differs")

def prepare_state_acl(root):
    state = root / "var/lib/vivac/custody-v1/users/65534/state"
    before = state.stat()
    require(before.st_uid == before.st_gid == 65534 and stat.S_IMODE(before.st_mode) == 0o700)
    payload = struct.pack("<I", 2)
    for tag, permissions, identifier in ((1,7,0xffffffff),(2,0,0),(4,0,0xffffffff),(16,0,0xffffffff),(32,0,0xffffffff)):
        payload += struct.pack("<HHI", tag, permissions, identifier)
    os.setxattr(state, "system.posix_acl_access", payload, follow_symlinks=False)
    after = state.stat()
    require(stat.S_IMODE(after.st_mode) == 0o700 and after.st_uid == before.st_uid and after.st_gid == before.st_gid
            and len(os.getxattr(state, "system.posix_acl_access", follow_symlinks=False)) > 0)

def require_fixture_store(store):
    require(store == Path("/store/run"), "fixture root is outside its dedicated mount")
    mount = Path("/store")
    require(not mount.is_symlink() and not store.is_symlink())
    package = session.package_backend()
    require(package.is_mountpoint(mount))
    metadata = store.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == metadata.st_gid == os.geteuid() == 65534
            and stat.S_IMODE(metadata.st_mode) == 0o700 and not package.acl_xattrs(store))
    require(package.filesystem_magic(store) == package.EXT_MAGIC)
    require({entry.name for entry in store.iterdir()} == CASES)

def run_case(helper, fixtures, case, environment, *, diagnostic=None):
    if diagnostic is None: diagnostic = {}
    diagnostic.update(case=case, stage="acl-prepare")
    root = fixtures / case
    if case == "state-acl": prepare_state_acl(root)
    diagnostic["stage"] = "baseline-snapshot"
    before_tree = fixture_snapshot(root)
    diagnostic["stage"] = "fixture-owners"
    verify_fixture_owners(before_tree, case)
    diagnostic["stage"] = "root-open"
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    diagnostic["stage"] = "observer-create"
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    channel.settimeout(8)
    process = None
    pidfds = []
    observed = False
    try:
        diagnostic["stage"] = "helper-start"
        resolved = helper.resolve(strict=True)
        metadata = resolved.stat()
        executable = (resolved, metadata.st_dev, metadata.st_ino)
        process = subprocess.Popen([str(helper), "--root-reader-case", case,
            "--observer-fd", str(peer.fileno()), "--root-fd", str(descriptor)],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment,
            close_fds=True, pass_fds=(peer.fileno(), descriptor))
        peer.close()
        diagnostic["stage"] = "before-packet"
        before = abandon.packet(channel, {"case", "identity"}, "before")
        diagnostic["stage"] = "before-validation"
        require(before["case"] == case)
        identity = before["identity"]
        validate_identity(identity)
        require(identity["host_pid"] == process.pid)
        diagnostic["stage"] = "host-identity"
        host_fd = os.pidfd_open(process.pid, 0)
        pidfds.append(host_fd)
        host_start = isolation.verify_process(process.pid, os.getpid(), executable, host_fd)
        diagnostic["stage"] = "start-action"
        channel.sendall(b"G")
        diagnostic["stage"] = "action-packet"
        action = parse_action_packet(channel.recv(8193))
        diagnostic["stage"] = "action-validation"
        validate_result(action, case, identity, observation=True)
        diagnostic["stage"] = "host-revalidation"
        isolation.verify_process(process.pid, os.getpid(), executable, host_fd, host_start)
        diagnostic["stage"] = "after-snapshot"
        after_tree = fixture_snapshot(root)
        diagnostic["stage"] = "fixture-comparison"
        fixture_unchanged = before_tree == after_tree
        binding = "./var/lib/vivac/custody-v1/users/65534/binding.json"
        binding_unchanged = before_tree.get(binding) == after_tree.get(binding)
        require(binding_unchanged and fixture_unchanged is (case != "snapshot-private-replacement"))
        diagnostic["stage"] = "finish-action"
        channel.sendall(b"G")
        diagnostic["stage"] = "final-packet"
        result = parse_result(channel.recv(8193), case, identity)
        diagnostic["stage"] = "evidence-comparison"
        for field in ROOT_FIELDS - {"resources_closed_after_exit", "pressure_iterations"}:
            require(result[field] == action[field], "candidate reader evidence changed after observation")
        diagnostic["stage"] = "host-exit"
        require(abandon.pidfd_ready(host_fd, 3))
        diagnostic["stage"] = "stdout-validation"
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0 and parse_result(output, case, identity) == result)
        diagnostic["stage"] = "final-snapshot"
        require(fixture_snapshot(root) == after_tree)
        result.update(process_identities_verified=True, host_retained_until_observation=True, host_exit_code=0,
            resources_closed_after_observed_exit=True, fixture_unchanged=fixture_unchanged, binding_unchanged=binding_unchanged,
            fixture_owners_verified=True)
        diagnostic["stage"] = "case-validation"
        validate_case(result)
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, descriptor, pidfds))
        else:
            for value in (channel, peer, process.stdout, process.stderr): value.close()
            for value in (descriptor, *pidfds): os.close(value)

def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "fixture-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-root-reader", status="incomplete", cases=[],
        resources_closed_after_observed_exit=False, candidate_root_reader_observed=False)
    diagnostic = {"case":"none", "stage":"setup"}
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        report["kernel_release"] = os.uname().release
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_root_reader_probe.py")
        session.require_report_directory(args.report)
        require_fixture_store(args.store_root)
        require(args.fixture_root == args.store_root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        for case in sorted(CASES):
            report["cases"].append(run_case(args.launcher_helper, args.fixture_root, case,
                life.private_env(args.store_root), diagnostic=diagnostic))
        diagnostic.update(case="none", stage="report-validation")
        report.update(status="observed", resources_closed_after_observed_exit=True, candidate_root_reader_observed=True)
        validate_observation(report)
        diagnostic["stage"] = "report-write"
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", resources_closed_after_observed_exit=False, candidate_root_reader_observed=False,
                      failure=failure_diagnostic(error, diagnostic))
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust candidate root reader experiment failed.", file=sys.stderr)
        return 1

if __name__ == "__main__": sys.exit(main())
