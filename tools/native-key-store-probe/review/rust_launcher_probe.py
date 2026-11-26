#!/usr/bin/env python3
"""Observe the candidate Rust spawn owner in a disposable Linux container."""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time

import lifecycle_probe as life
import provider_probe as providers
import session_probe as session

CASES = ("stable-thread", "exec-error", "creator-thread-exit", "owner-before-arm",
         "owner-after-arm", "native-agent", "native-agent-owner-death")
COMMON = {"version", "case", "status", "custody_accredited", "launcher_accredited", "threadsafe_fullflow"}
NORMAL_FLAGS = {
    "stable-thread": {"parent_poison_present", "nnp_after_exec", "signal_mask_empty", "fd_allowlist_verified",
                      "child_environment_clean", "canary_excluded", "shutdown_observed"},
    "exec-error": {"exec_failure_observed", "failed_exec_stdout_empty", "owner_reusable_after_error", "shutdown_observed"},
    "creator-thread-exit": {"creator_thread_exit_observed", "parent_process_alive", "child_sigkill_observed",
                            "held_control_through_exit", "deliberately_forgotten_owner"},
    "native-agent": {"empty_identities_observed", "native_credentials_checked", "native_sigkill_observed",
                     "shutdown_observed", "retained_connection_refused", "new_connection_refused"},
    "native-agent-owner-death": {"empty_identities_observed", "native_credentials_checked"},
}
EXTRA = {
    "stable-thread": {"creator_tid", "native_children_observed", "caller_threads_returned", "pressure_threads", "pressure_iterations"},
    "exec-error": {"exec_failure_raw_os_error", "exec_preexec_seams_observed"},
    "creator-thread-exit": {"creator_tid"},
    "native-agent": {"pid", "creator_tid", "pressure_iterations"},
    "native-agent-owner-death": {"pid", "creator_tid"},
}
CURRENT = None
UNCERTAIN = []

class ProbeError(Exception):
    pass

def require(ok, message):
    if not ok:
        raise ProbeError(message)

def positive_int(value):
    return type(value) is int and value > 0

def parse_result(data, case, status="observed"):
    require(0 < len(data) <= 32768, "Rust result size differs")
    result = json.loads(data.decode("ascii"))
    require(type(result) is dict and set(result) == COMMON | NORMAL_FLAGS[case] | EXTRA[case],
            "Rust result schema differs")
    require(type(result["version"]) is int and result["version"] == 1 and result["case"] == case
            and result["status"] == status, "Rust result identity differs")
    for field in ("custody_accredited", "launcher_accredited", "threadsafe_fullflow"):
        require(result[field] is False, "Rust probe claims full admission")
    require(all(result[key] is True for key in NORMAL_FLAGS[case]), "Rust observation is incomplete")
    for key in EXTRA[case] - {"exec_preexec_seams_observed"}:
        require(positive_int(result[key]), "Rust result counter differs")
    if case == "stable-thread":
        require(all(result[key] == 4 for key in ("native_children_observed", "caller_threads_returned", "pressure_threads")),
                "multithreaded pressure fixture differs")
    if case == "exec-error":
        require(result["exec_failure_raw_os_error"] == errno.ENOEXEC
                and result["exec_preexec_seams_observed"] == ["B", "A"], "exec failure was not observed")
    return result

def bounded_output(process, deadline, *, first_line=False):
    output, errors = bytearray(), bytearray()
    with selectors.DefaultSelector() as selector:
        for stream, label in ((process.stdout, "out"), (process.stderr, "err")):
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, label)
        while selector.get_map():
            require(time.monotonic() < deadline, "Rust process observation deadline expired")
            for key, _ in selector.select(0.03):
                try:
                    chunk = os.read(key.fd, 4096)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                (output if key.data == "out" else errors).extend(chunk)
                require(len(output) + len(errors) <= 32768, "Rust process output exceeded its bound")
            if first_line and b"\n" in output:
                require(output.count(b"\n") == 1 and not errors, "Rust readiness output differs")
                return bytes(output)
    require(not errors, "Rust process emitted unexpected diagnostics")
    return bytes(output)

def observe_exit(pid, descriptor, deadline):
    with selectors.DefaultSelector() as selector:
        selector.register(descriptor, selectors.EVENT_READ)
        while time.monotonic() < deadline:
            if selector.select(0.03):
                try:
                    actual, status = os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    continue
                if actual == pid:
                    return status
    raise ProbeError("native descendant exit was not observed")

def receive_seam(channel, expected, pid=None):
    packet = channel.recv(9)
    require(len(packet) == 8 and packet[:4] == expected.encode("ascii") + b"\0\0\0", "preexec seam packet differs")
    actual = struct.unpack(">I", packet[4:])[0]
    require(actual > 0 and (pid is None or actual == pid), "preexec child identity differs")
    return actual

def connection_refused(connection, path):
    retained = False
    connection.settimeout(1)
    try:
        connection.sendall(b"\0\0\0\1\x0b")
        retained = connection.recv(5) == b""
    except (OSError, socket.timeout):
        retained = True
    fresh = socket.socket(socket.AF_UNIX)
    try:
        fresh.settimeout(1)
        try:
            fresh.connect(str(path))
        except OSError:
            return retained
        return False
    finally:
        fresh.close()

def run_case(helper, provider, root, env, case, state):
    channel = peer = process = native_fd = connection = None
    deadline = time.monotonic() + 25
    observed = False
    arguments = [str(helper), "--case", case]
    passed = []
    socket_path = root / (case + ".sock")
    if case.startswith("native-agent"):
        agent = provider.files["ssh-agent"]
        agent.revalidate()
        arguments += ["--agent-fd", str(agent.fd), "--socket", str(socket_path)]
        passed.append(agent.fd)
    if case in ("owner-before-arm", "owner-after-arm"):
        channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        channel.settimeout(10)
        arguments += ["--observer-fd", str(peer.fileno())]
        passed.append(peer.fileno())
    state["uncertain"] = True
    try:
        process = subprocess.Popen(arguments, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   pass_fds=tuple(passed), env=env, close_fds=True)
        if peer:
            peer.close()
            peer = None
        if channel:
            pid = receive_seam(channel, "B")
            require(life.agent_credentials(pid)["PPid"] == [process.pid], "preexec parent differs")
            native_fd = os.pidfd_open(pid)
            if case == "owner-after-arm":
                channel.sendall(b"G")
                receive_seam(channel, "A", pid)
            process.kill()
            require(process.wait(timeout=2) == -signal.SIGKILL, "Rust owner did not exit by SIGKILL")
            if case == "owner-before-arm":
                channel.sendall(b"G")
                receive_seam(channel, "P", pid)
            status = observe_exit(pid, native_fd, deadline)
            require((os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL) if case == "owner-after-arm"
                    else not (os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0), "parent-death outcome differs")
            require(bounded_output(process, deadline) == b"", "preexec child unexpectedly executed the receipt program")
            result = {"case": case, "status": "observed", "seam": "A" if case.endswith("after-arm") else "B",
                      "child_exit_observed": True, "owner_sigkill_observed": True, "observer_endpoint_retained": True,
                      "native_exec_observed": False, "child_signal": os.WTERMSIG(status) if os.WIFSIGNALED(status) else None,
                      "child_code": os.WEXITSTATUS(status) if os.WIFEXITED(status) else None}
            if case == "owner-before-arm":
                result["parent_change_refusal_observed"] = True
        elif case == "native-agent-owner-death":
            ready = parse_result(bounded_output(process, deadline, first_line=True), case, "ready")
            pid = ready["pid"]
            native_fd = os.pidfd_open(pid)
            life.inspect_agent(pid, life.process_start(pid), str(provider.files["ssh-agent"].path), process.pid)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(str(socket_path))
            process.kill()
            require(process.wait(timeout=2) == -signal.SIGKILL, "Rust agent owner did not exit by SIGKILL")
            status = observe_exit(pid, native_fd, deadline)
            require(os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL, "native agent parent link did not survive exec")
            require(connection_refused(connection, socket_path), "native agent connection remains usable")
            require(bounded_output(process, deadline) == b"", "Rust owner death emitted additional output")
            result = {"case": case, "status": "observed", "readiness": ready, "child_sigkill_observed": True,
                      "owner_sigkill_observed": True, "retained_connection_refused": True,
                      "new_connection_refused": True, "native_credentials_checked": True}
        else:
            process.stdin.close()
            output = bounded_output(process, deadline)
            require(process.wait(timeout=2) == 0, "Rust probe case failed")
            result = parse_result(output, case)
        provider.revalidate()
        observed = True
        state["uncertain"] = False
        return result
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=2)
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream and not stream.closed:
                    stream.close()
        if native_fd is not None:
            os.close(native_fd)
        for value in (channel, peer, connection):
            if value:
                value.close()
        if not observed:
            state["uncertain"] = True

def file_hash(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    global CURRENT
    parser = argparse.ArgumentParser()
    for name in ("writer", "verifier", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-launcher", "status": "incomplete", "cases": [],
              "custody_accredited": False, "launcher_accredited": False, "full_loader_tested": False,
              "threadsafe_fullflow": False, "secret_channel_tested": False}
    CURRENT = report
    provider = None
    root = None
    state = {"uncertain": False}
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable Linux is required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        report["resource_limits"] = life.require_resources(expected_script="rust_launcher_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        root = Path(tempfile.mkdtemp(prefix="rust-launcher-", dir=args.store_root))
        root.chmod(0o700)
        identity = session.package_backend().validate_safe_tree_root(root)
        (root / "home").mkdir(mode=0o700)
        environment = life.private_env(root)
        environment["VIVAC_LAUNCHER_POISON"] = "present"
        provider = providers.Provider()
        provider.open()
        version, architecture = provider.package(environment)
        report["provenance"] = {"package_version": version, "package_arch": architecture, "components": provider.components()}
        report["helper_sha256"] = file_hash(args.launcher_helper)
        report["observer_sha256"] = file_hash(Path(__file__))
        for case in CASES:
            pending = {"case": case, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args.launcher_helper, provider, root, environment, case, state))
        require(not state["uncertain"], "native exit remains uncertain")
        provider.close()
        provider = None
        session.package_backend().cleanup_owned_tree(root, identity)
        root = None
        report.update(status="observed", resources_closed_after_exit=True)
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report["failure"] = {"kind": type(error).__name__}
        report["status"] = "incomplete"
        if state["uncertain"]:
            UNCERTAIN.append((provider, root))
        elif provider is not None:
            provider.close()
        try:
            life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        except OSError:
            pass
        print("Rust launcher experiment failed.", file=sys.stderr)
        return 1

if __name__ == "__main__":
    sys.exit(main())
