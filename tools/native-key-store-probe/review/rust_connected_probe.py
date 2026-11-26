#!/usr/bin/env python3
"""Observe connected sibling identity without accrediting endpoint authority."""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import traceback

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_ipc_probe as ipc
import rust_isolation_probe as isolation
import rust_launcher_probe as launcher
import session_probe as session

CASES = {"roundtrip", "wrong-generation", "wrong-store", "wrong-role", "forwarded-sender",
         "peer-exit", "historical-listener"}
SCOPE_FIELDS = ipc.SCOPE_FIELDS | {"endpoint_authority_accredited"}
IDENTITY_FIELDS = {"host_pid", "creator_tid", "server_pid", "server_token", "client_pid", "client_token",
                   "injector_pid", "injector_token", "server_connected_peer_pid", "client_connected_peer_pid"}
SIDE_FIELDS = {side + "_" + field for side in ("server", "client") for field in (
    "handshake_completed", "hello_error", "accepted_probes", "refusals", "poisoned", "sticky_refusal",
    "context_captured", "context_identities_verified", "context_valid_after_exchange",
    "context_peer_exit_refused", "context_sticky_refusal")}
ROOT_FIELDS = SCOPE_FIELDS | IDENTITY_FIELDS | SIDE_FIELDS | {
    "version", "case", "status", "listener_origin_parent", "server_exit_code", "client_exit_code",
    "injector_exit_code", "server_exit_observed", "owned_exits_observed", "resources_closed_after_exit",
    "pressure_threads", "pressure_iterations"}
EXTERNAL_FIELDS = {"root_identity_verified", "server_identity_verified", "client_identity_verified",
                   "injector_identity_verified", "server_exit_observed_externally", "client_exit_observed_externally",
                   "injector_exit_observed_externally", "host_exit_code", "client_retained_until_observation",
                   "server_retained_until_observation", "injector_retained_until_observation",
                   "resources_closed_after_observed_exit"}
REPORT_FIELDS = SCOPE_FIELDS | {"version", "experiment", "status", "cases", "resource_limits",
                              "resources_closed_after_observed_exit", "helper_sha256", "observer_sha256", "kernel_release"}
UNCERTAIN = []
require = abandon.require


class KernelCapabilityUnavailable(ValueError):
    def __init__(self, original_errno):
        super().__init__("SO_PEERPIDFD is unavailable")
        self.errno = original_errno


def require_peer_pidfd_capability():
    # This self-pair checks kernel support only; it establishes no peer authority.
    left, right = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET | socket.SOCK_CLOEXEC)
    with left, right:
        require(not left.get_inheritable() and not right.get_inheritable())
        for endpoint in (left, right):
            endpoint.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        try:
            descriptor = left.getsockopt(socket.SOL_SOCKET, 77)
        except OSError as error:
            if error.errno in (errno.ENOPROTOOPT, errno.EOPNOTSUPP):
                raise KernelCapabilityUnavailable(error.errno) from error
            raise
        require(type(descriptor) is int and descriptor >= 0)
        os.close(descriptor)



def positive(value):
    require(type(value) is int and 0 < value <= 0xffffffffffffffff)


def validate_identity(value, case, *, early=False, expected=None):
    require(type(value) is dict and set(value) == IDENTITY_FIELDS)
    for field in IDENTITY_FIELDS - {"injector_pid", "injector_token"}:
        positive(value[field])
    require(value["host_pid"] == value["creator_tid"]
            and len({value[field] for field in ("host_pid", "server_pid", "client_pid")}) == 3
            and value["server_token"] != value["client_token"])
    require(value["server_connected_peer_pid"] == value["client_pid"]
            and value["client_connected_peer_pid"] == value["host_pid" if case == "historical-listener" else "server_pid"])
    if case == "forwarded-sender" and not early:
        positive(value["injector_pid"])
        positive(value["injector_token"])
        require(value["injector_pid"] not in {value[field] for field in ("host_pid", "server_pid", "client_pid")})
    else:
        require(value["injector_pid"] is None and value["injector_token"] is None)
    require(expected is None or value == expected, "connected receipt identity changed")


def validate_result(value, case, identity=None, *, observation=False):
    require(type(value) is dict and set(value) == ROOT_FIELDS | ({"stage"} if observation else set()))
    require(case in CASES and value["case"] == case and value["status"] == "observed")
    require(not observation or value["stage"] == "observation")
    validate_identity({key: value[key] for key in IDENTITY_FIELDS}, case, expected=identity)
    require(all(value[field] is False for field in SCOPE_FIELDS))
    require(value["listener_origin_parent"] is (case == "historical-listener"))
    mismatch = case in ("wrong-generation", "wrong-store", "wrong-role")
    for side in ("server", "client"):
        context_exited = side == "client" and case == "peer-exit"
        for field, expected in (("context_captured", True), ("context_identities_verified", True),
                                ("context_valid_after_exchange", not context_exited),
                                ("context_peer_exit_refused", context_exited),
                                ("context_sticky_refusal", context_exited)):
            require(value[side + "_" + field] is expected)
        hello = value[side + "_handshake_completed"]
        require(type(hello) is bool)
        if not (case == "historical-listener" and side == "server"):
            require(hello is not (mismatch or case == "historical-listener"))
        error = value[side + "_hello_error"]
        require(error is None if hello else error in ("invalid-data", "permission-denied"))
        if mismatch:
            require(error == "invalid-data", "hello mismatch was not rejected by frame validation")
        if case == "historical-listener" and side == "client":
            require(error == "permission-denied", "historical peer was not rejected by sender identity")
        bad = not hello or side == "server" and case == "forwarded-sender" or side == "client" and case == "peer-exit"
        for field in ("poisoned", "sticky_refusal"):
            require(value[side + "_" + field] is bad)
        for field, expected in (("refusals", int(bad)), ("accepted_probes", 2 if case == "roundtrip" else 0)):
            require(type(value[side + "_" + field]) is int and value[side + "_" + field] == expected)
    for field, expected in (("version", 1), ("pressure_threads", 4)):
        require(type(value[field]) is int and value[field] == expected)
    require(type(value["pressure_iterations"]) is int
            and (value["pressure_iterations"] == 0 if observation else value["pressure_iterations"] > 0))
    for field, expected in (("server_exit_observed", not observation or case == "peer-exit"),
                             ("owned_exits_observed", not observation), ("resources_closed_after_exit", not observation)):
        require(value[field] is expected)
    for field, present in (("server_exit_code", not observation or case == "peer-exit"),
                            ("client_exit_code", not observation),
                            ("injector_exit_code", not observation and case == "forwarded-sender")):
        require(type(value[field]) is int and value[field] == 0 if present else value[field] is None)


def parse_result(payload, case, identity=None, *, observation=False):
    value = abandon.decode(payload)
    validate_result(value, case, identity, observation=observation)
    return value


def validate_observation(report):
    require(type(report) is dict and set(report) == REPORT_FIELDS)
    require(type(report["version"]) is int and report["version"] == 1
            and report["experiment"] == "rust-connected" and report["status"] == "observed")
    require(all(report[field] is False for field in SCOPE_FIELDS)
            and report["resources_closed_after_observed_exit"] is True and type(report["resource_limits"]) is dict)
    kernel = report["kernel_release"]
    require(type(kernel) is str and 0 < len(kernel) <= 128 and all(33 <= ord(byte) <= 126 for byte in kernel))
    for field in ("helper_sha256", "observer_sha256"):
        require(type(report[field]) is str and len(report[field]) == 64
                and all(byte in "0123456789abcdef" for byte in report[field]))
    rows = report["cases"]
    require(type(rows) is list and len(rows) == len(CASES) and all(type(row) is dict for row in rows)
            and {row.get("case") for row in rows} == CASES)
    for row in rows:
        require(set(row) == ROOT_FIELDS | EXTERNAL_FIELDS)
        validate_result({key: row[key] for key in ROOT_FIELDS}, row["case"])
        for field in EXTERNAL_FIELDS - {"host_exit_code"}:
            expected = row["case"] == "forwarded-sender" if field.startswith("injector_") else row["case"] != "peer-exit" if field == "server_retained_until_observation" else True
            require(row[field] is expected)
        require(type(row["host_exit_code"]) is int and row["host_exit_code"] == 0)


def ready(channel, stage, case, host_pid, previous=None):
    value = abandon.packet(channel, IDENTITY_FIELDS | {"case"}, stage)
    require(value["case"] == case and type(value["host_pid"]) is int and value["host_pid"] == host_pid)
    identity = {field: value[field] for field in IDENTITY_FIELDS}
    validate_identity(identity, case, early=stage == "ready" or stage == "peer-exited")
    if previous is not None:
        for field in IDENTITY_FIELDS - {"injector_pid", "injector_token"}:
            require(identity[field] == previous[field])
    return identity


def run_case(helper, case, store, environment):
    root = store / case
    root.mkdir(mode=0o700)
    key = ipc.public_store(root / "a")
    require(ipc.public_store(root / "b") == key)
    directory = os.open(root / "a", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW)
    other = os.open(root / "b", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW)
    cipher = ipc.sealed_public_cipher(key)
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    channel.settimeout(6)
    process = None
    pidfds = []
    observed = False
    try:
        helper_path = helper.resolve(strict=True)
        metadata = helper_path.stat()
        executable = (helper_path, metadata.st_dev, metadata.st_ino)
        process = subprocess.Popen([str(helper), "--connected-case", case, "--observer-fd", str(peer.fileno()),
                                    "--directory-fd", str(directory), "--other-directory-fd", str(other),
                                    "--cipher-fd", str(cipher), "--socket", str(root / "endpoint.sock")],
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   env=environment, close_fds=True, pass_fds=(peer.fileno(), directory, other, cipher))
        peer.close()
        identity = ready(channel, "ready", case, process.pid)
        descriptors = {}
        starts = {}
        for side, parent in (("host", os.getpid()), ("server", process.pid), ("client", process.pid)):
            descriptors[side] = os.pidfd_open(identity[side + "_pid"], 0)
            pidfds.append(descriptors[side])
            starts[side] = isolation.verify_process(identity[side + "_pid"], parent, executable, descriptors[side])

        def verify(side, parent):
            return isolation.verify_process(identity[side + "_pid"], parent, executable, descriptors[side], starts[side])

        channel.sendall(b"G")
        if case == "peer-exit":
            ready(channel, "peer-exited", case, process.pid, identity)
            require(abandon.pidfd_ready(descriptors["server"], 3))
            verify("host", os.getpid())
            verify("client", process.pid)
            channel.sendall(b"G")
        if case == "forwarded-sender":
            identity = ready(channel, "injector-ready", case, process.pid, identity)
            descriptors["injector"] = os.pidfd_open(identity["injector_pid"], 0)
            pidfds.append(descriptors["injector"])
            starts["injector"] = isolation.verify_process(identity["injector_pid"], identity["client_pid"], executable, descriptors["injector"])
            for side, parent in (("host", os.getpid()), ("server", process.pid), ("client", process.pid)):
                verify(side, parent)
            channel.sendall(b"G")
        action = parse_result(channel.recv(8193), case, identity, observation=True)
        verify("host", os.getpid())
        verify("client", process.pid)
        if case == "peer-exit":
            require(abandon.pidfd_ready(descriptors["server"]))
        else:
            verify("server", process.pid)
        if case == "forwarded-sender":
            verify("injector", identity["client_pid"])
        channel.sendall(b"G")
        result = parse_result(channel.recv(8193), case, identity)
        for field in ROOT_FIELDS - {"server_exit_code", "client_exit_code", "injector_exit_code",
                                    "server_exit_observed", "owned_exits_observed", "resources_closed_after_exit", "pressure_iterations"}:
            require(result[field] == action[field], "connected evidence changed after observation")
        for side in ("server", "client", "injector"):
            if side in descriptors:
                require(abandon.pidfd_ready(descriptors[side], 3))
        output = launcher.bounded_output(process, time.monotonic() + 3)
        require(process.wait(timeout=2) == 0 and parse_result(output, case, identity) == result)
        result.update(root_identity_verified=True, server_identity_verified=True, client_identity_verified=True,
                      injector_identity_verified=case == "forwarded-sender", server_exit_observed_externally=True,
                      client_exit_observed_externally=True, injector_exit_observed_externally=case == "forwarded-sender",
                      host_exit_code=0, client_retained_until_observation=True,
                      server_retained_until_observation=case != "peer-exit",
                      injector_retained_until_observation=case == "forwarded-sender",
                      resources_closed_after_observed_exit=True)
        observed = True
        return result
    finally:
        if not observed:
            UNCERTAIN.append((process, channel, peer, directory, other, cipher, pidfds))
        else:
            for value in (channel, peer, process.stdout, process.stderr):
                value.close()
            for descriptor in (directory, other, cipher, *pidfds):
                os.close(descriptor)


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-connected", status="incomplete", cases=[], resources_closed_after_observed_exit=False)
    checkpoint = "preflight"
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1")
        report["kernel_release"] = os.uname().release
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1)
        report["resource_limits"] = life.require_resources(expected_script="rust_connected_probe.py")
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        report["helper_sha256"] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report["observer_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        checkpoint = "capability"
        require_peer_pidfd_capability()
        for case in sorted(CASES):
            checkpoint = "case:" + case
            report["cases"].append(run_case(args.launcher_helper, case, args.store_root, life.private_env(args.store_root)))
        report.update(status="observed", resources_closed_after_observed_exit=True)
        validate_observation(report)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        failure = {"kind": type(error).__name__}
        if isinstance(error, KernelCapabilityUnavailable):
            failure.update(option="SO_PEERPIDFD", errno=error.errno)
        elif isinstance(error, OSError):
            if type(error.errno) is int:
                failure["errno"] = error.errno
            failure.update(checkpoint=checkpoint, traceback=[
                {"module": frame.filename.replace("\\", "/").rsplit("/", 1)[-1],
                 "function": frame.name, "line": frame.lineno}
                for frame in traceback.extract_tb(error.__traceback__, limit=-8)
            ])
        report.update(status="incomplete", resources_closed_after_observed_exit=False, failure=failure)
        life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        print("Rust connected-peer experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
