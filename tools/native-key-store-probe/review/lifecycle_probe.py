#!/usr/bin/env python3
"""Disposable Linux probe for native ssh-agent parent-death behavior."""

import argparse
from contextlib import ExitStack, contextmanager
import ctypes
import errno
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time

import openssh_probe as native


TIMEOUT = 15
DEATH_TIMEOUT = 3
OUTPUT_LIMIT = 4096
CURRENT_STAGE = "preflight"
CURRENT_REPORT = None
PDEATHSIG = 1
PR_GET_PDEATHSIG = 2
PR_SET_PDEATHSIG = 1
PR_GET_NO_NEW_PRIVS = 39
PR_SET_NO_NEW_PRIVS = 38
PR_SET_CHILD_SUBREAPER = 36


class ProbeError(Exception):
    pass


class ParentChanged(ProbeError):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def prctl(option, value=0):
    libc = ctypes.CDLL(None, use_errno=True)
    result = libc.prctl(option, value, 0, 0, 0)
    if result < 0:
        error = ctypes.get_errno()
        raise OSError(error, "prctl failed")
    return result


def agent_credentials(pid):
    values = {}
    for line in Path("/proc", str(pid), "status").read_text(encoding="ascii").splitlines():
        if line.startswith(("Uid:", "Gid:", "NoNewPrivs:", "PPid:")):
            key, value = line.split(":", 1)
            values[key] = [int(item) for item in value.split()]
    require(set(values) == {"Uid", "Gid", "NoNewPrivs", "PPid"}, "process credentials unavailable")
    return values


def process_start(pid):
    data = Path("/proc", str(pid), "stat").read_text(encoding="ascii")
    tail = data[data.rfind(")") + 2:].split()
    require(len(tail) > 19, "process start identity unavailable")
    return int(tail[19])


def executable(pid):
    return os.readlink(Path("/proc", str(pid), "exe"))


def pidfd_ready(pidfd, timeout=DEATH_TIMEOUT):
    poller = select.poll()
    poller.register(pidfd, select.POLLIN | select.POLLHUP | select.POLLERR)
    return bool(poller.poll(max(0, int(timeout * 1000))))


def pidfd_signal(pidfd, sig):
    require(hasattr(signal, "pidfd_send_signal"), "pidfd signaling is unavailable")
    signal.pidfd_send_signal(pidfd, sig)


def private_env(root, agent_socket=None):
    result = native.environment(root, agent_socket)
    result["VIVAC_OPENSSH_DISPOSABLE"] = "1"
    return result


def require_isolation():
    interfaces = {entry.name for entry in Path("/sys/class/net").iterdir()}
    require(interfaces == {"lo"}, "container network is not isolated to loopback")
    root_mount = None
    for line in Path("/proc/self/mountinfo").read_text(encoding="ascii").splitlines():
        fields = line.split()
        if len(fields) >= 6 and fields[4] == "/":
            root_mount = set(fields[5].split(","))
    require(root_mount is not None and "ro" in root_mount,
            "container root filesystem is not mounted read-only")


def require_unprivileged():
    require(os.geteuid() != 0, "disposable probe must not run as root")
    capability_line = next((line for line in Path("/proc/self/status").read_text(
        encoding="ascii").splitlines() if line.startswith("CapEff:")), None)
    require(capability_line is not None, "effective capability metadata is unavailable")
    require(int(capability_line.split(":", 1)[1].strip(), 16) == 0,
            "disposable probe has effective capabilities")


def require_resources(proc_root=Path("/proc"), *, expected_script="lifecycle_probe.py"):
    require(expected_script in ("lifecycle_probe.py", "session_probe.py", "concurrent_probe.py", "loader_probe.py", "ssh_lifecycle_probe.py", "provider_probe.py", "kdf_probe.py", "heap_probe.py", "generated_probe.py", "rust_launcher_probe.py", "rust_loader_probe.py", "rust_ssh_loader_probe.py", "rust_concurrent_probe.py", "rust_ssh_concurrent_probe.py", "rust_abandon_probe.py", "rust_isolation_probe.py", "rust_ipc_probe.py", "rust_reservation_probe.py", "rust_connected_probe.py", "rust_admission_storage_probe.py", "rust_root_reader_probe.py", "rust_provision_probe.py", "rust_fixed_root_probe.py", "rust_resident_probe.py", "rust_creation_binding_probe.py", "creation_draft_probe.py", "creation_native_probe.py", "unlock_input_probe.py", "exit_watch_probe.py", "registration_probe.py"),
            "deadline script is outside the experiment allowlist")
    if os.getpid() != 1:
        require(os.getppid() == 1, "observer requires PID 1 or its private deadline parent")
        expected_timeout = shutil.which("timeout", path=os.defpath)
        require(expected_timeout is not None and (proc_root / "1" / "exe").resolve(strict=True)
                == Path(expected_timeout).resolve(strict=True), "PID 1 is not the expected deadline executable")
        command = (proc_root / "1" / "cmdline").read_bytes().split(b"\0")
        deadline = b"240s"
        if expected_script in ("rust_fixed_root_probe.py", "rust_resident_probe.py"):
            phase_arguments = command[7:]
            require(len(phase_arguments) >= 2 and phase_arguments[0] == b"--phase"
                    and phase_arguments[1] in (b"setup", b"reader", b"mutate"), "fixed namespace phase was refused")
            deadline = b"30s" if phase_arguments[1] == b"mutate" else b"120s"
        prefix = [b"--signal=TERM", b"--kill-after=5s", deadline, b"python3", b"-B",
                  ("/probe/" + expected_script).encode("ascii")]
        require(bool(command[0]) and Path(os.fsdecode(command[0])).resolve(strict=True)
                == Path(expected_timeout).resolve(strict=True)
                and command[1:len(prefix) + 1] == prefix, "PID 1 deadline command was refused")
        values = {}
        for line in (proc_root / "1" / "status").read_text(encoding="ascii").splitlines():
            if line.startswith(("Uid:", "NoNewPrivs:")):
                key, value = line.split(":", 1)
                values[key] = [int(item) for item in value.split()]
        require(values.get("Uid") == [os.geteuid()] * 4 and values.get("NoNewPrivs") == [1],
                "PID 1 deadline credentials were refused")
    memberships = {}
    for line in (proc_root / "self" / "cgroup").read_text(encoding="ascii").splitlines():
        _, controllers, location = line.split(":", 2)
        for controller in controllers.split(","):
            memberships[controller] = location
    mounts = []
    for line in (proc_root / "self" / "mountinfo").read_text(encoding="ascii").splitlines():
        before, after = line.split(" - ", 1)
        fields, filesystem = before.split(), after.split()
        if filesystem[0] in ("cgroup", "cgroup2"):
            mounts.append((fields[3], Path(fields[4]), filesystem[0], set(filesystem[2].split(","))))

    def group_path(controller):
        for root, mount, kind, controllers in mounts:
            key = "" if kind == "cgroup2" else controller
            if key not in memberships or (kind == "cgroup" and controller not in controllers):
                continue
            location = memberships[key]
            require(".." not in Path(location).parts, "cgroup membership path was refused")
            try:
                relative = Path(location).relative_to(Path(root))
            except ValueError:
                continue
            return mount / relative, kind
        raise ProbeError("effective cgroup controller is unavailable")

    def integer(path):
        value = path.read_text(encoding="ascii").strip()
        require(value != "max", "unbounded cgroup limit was refused")
        try:
            return int(value)
        except ValueError as error:
            raise ProbeError("cgroup limit was malformed") from error

    memory, kind = group_path("memory")
    cpu, cpu_kind = group_path("cpu")
    pids, _ = group_path("pids")
    if kind == "cgroup2":
        memory_bytes = integer(memory / "memory.max")
        swap_bytes = integer(memory / "memory.swap.max")
        require(swap_bytes == 0, "additional swap must be disabled")
    else:
        memory_bytes = integer(memory / "memory.limit_in_bytes")
        combined = integer(memory / "memory.memsw.limit_in_bytes")
        require(combined == memory_bytes, "additional swap must be disabled")
        swap_bytes = 0
    if cpu_kind == "cgroup2":
        values = (cpu / "cpu.max").read_text(encoding="ascii").split()
        require(len(values) == 2 and values[0] != "max", "unbounded CPU quota was refused")
        quota, period = map(int, values)
    else:
        quota, period = integer(cpu / "cpu.cfs_quota_us"), integer(cpu / "cpu.cfs_period_us")
    pid_limit = integer(pids / "pids.max")
    require(0 < memory_bytes <= 512 * 1024 * 1024, "memory limit exceeds the fixture budget")
    require(0 < quota <= period, "CPU quota exceeds the fixture budget")
    require(0 < pid_limit <= 128, "PID limit exceeds the fixture budget")
    usage = os.statvfs("/tmp")
    temporary_bytes = usage.f_blocks * usage.f_frsize
    require(0 < temporary_bytes <= 128 * 1024 * 1024, "temporary filesystem exceeds the fixture budget")
    return {"memory_bytes": memory_bytes, "swap_bytes": swap_bytes, "pids": pid_limit,
            "cpu_quota": quota, "cpu_period": period, "temporary_bytes": temporary_bytes}


def unlink_owned_socket(path):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid(),
            "owned socket cleanup identity was refused")
    path.unlink()


def send_json(connection, value):
    payload = json.dumps(value, separators=(",", ":")).encode("ascii")
    require(len(payload) <= OUTPUT_LIMIT, "control metadata exceeded its bound")
    connection.settimeout(TIMEOUT)
    connection.sendall(struct.pack(">I", len(payload)) + payload)


def receive_command(connection):
    connection.settimeout(TIMEOUT)
    return connection.recv(1)


def receive_json(connection, deadline):
    def exact(size):
        data = bytearray()
        while len(data) < size:
            remaining = deadline - time.monotonic()
            require(remaining > 0, "control channel timed out")
            connection.settimeout(remaining)
            chunk = connection.recv(size - len(data))
            require(bool(chunk), "control channel closed")
            data.extend(chunk)
        return bytes(data)
    length = struct.unpack(">I", exact(4))[0]
    require(0 < length <= OUTPUT_LIMIT, "control metadata length was refused")
    return json.loads(exact(length).decode("ascii"))


def configure_parent_death(*, unlinked_control=False):
    prctl(PR_SET_NO_NEW_PRIVS, 1)
    if not unlinked_control:
        prctl(PR_SET_PDEATHSIG, signal.SIGKILL)
    armed = ctypes.c_int()
    libc = ctypes.CDLL(None, use_errno=True)
    require(libc.prctl(PR_GET_PDEATHSIG, ctypes.byref(armed), 0, 0, 0) == 0,
            "parent-death signal could not be read back")
    if unlinked_control:
        require(armed.value == 0, "unlinked control unexpectedly has a parent-death signal")
        return "unlinked"
    require(armed.value == signal.SIGKILL, "parent-death signal did not arm")
    return "armed"


def agent_exec(args):
    expected = args.expected_parent
    native.child_setup()
    if os.getppid() != expected:
        raise ParentChanged("executor parent changed before arming")
    sock = socket.socket(fileno=args.control_fd)
    send_json(sock, {"kind": "prearm", "pid": os.getpid(), "parent": expected})
    command = receive_command(sock)
    require(command in (b"A", b""), "invalid pre-arm barrier command")
    kind = configure_parent_death(unlinked_control=args.unlinked_control)
    if os.getppid() != expected:
        raise ParentChanged("executor parent changed between initial check and parent-death arm")
    send_json(sock, {"kind": kind, "pid": os.getpid(), "parent": expected})
    # A deterministic pre-exec barrier lets the observer kill the supervisor
    # after arming and before exec. The parent must remain the exact supervisor.
    command = receive_command(sock)
    require(command == b"G" and os.getppid() == expected, "parent vanished before native exec")
    executable_path = shutil.which("ssh-agent", path=os.defpath)
    require(executable_path is not None, "OpenSSH agent is unavailable")
    sock.close()
    os.execve(executable_path, [executable_path, "-D", "-a", args.agent_socket],
              private_env(Path(args.home), args.agent_socket))
    return 1


def supervisor(args):
    parent = os.getpid()
    child_control, executor_control = socket.socketpair()
    child = None
    with ExitStack() as channels:
        channels.callback(child_control.close)
        channels.callback(executor_control.close)
        observer_control = socket.socket(fileno=args.control_fd)
        channels.callback(observer_control.close)
        try:
            os.set_inheritable(child_control.fileno(), True)
            arguments = [sys.executable, str(Path(__file__).resolve()), "--internal-agent-exec",
                         "--expected-parent", str(parent), "--agent-socket", args.agent_socket,
                         "--home", args.home, "--control-fd", str(child_control.fileno())]
            if args.unlinked_control:
                arguments.append("--unlinked-control")
            child = subprocess.Popen(
                arguments,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                pass_fds=(child_control.fileno(),), close_fds=True,
            )
            child_control.close()
            executor_control.settimeout(TIMEOUT)
            metadata = receive_json(executor_control, time.monotonic() + TIMEOUT)
            require(metadata == {"kind": "prearm", "pid": child.pid, "parent": parent},
                    "executor pre-arm metadata did not match child identity")
            send_json(observer_control, metadata)
            require(receive_command(observer_control) == b"A", "observer did not release pre-arm barrier")
            executor_control.sendall(b"A")
            metadata = receive_json(executor_control, time.monotonic() + TIMEOUT)
            kind = "unlinked" if args.unlinked_control else "armed"
            require(metadata == {"kind": kind, "pid": child.pid, "parent": parent},
                    "executor readiness metadata did not match child identity")
            send_json(observer_control, {"kind": "executor", "pid": child.pid,
                                        "parent": parent, "socket": args.agent_socket})
            require(receive_command(observer_control) == b"G", "observer did not release native exec barrier")
            executor_control.sendall(b"G")
            # A successful ordinary return must leave death to the kernel,
            # rather than manufacturing the observation through cleanup.
            command = receive_command(observer_control)
            if command == b"R":
                status = reap_owned(child.pid)
                require(status is not None, "parent could not collect the native agent")
                child.returncode = os.waitstatus_to_exitcode(status)
                send_json(observer_control, {"kind": "agent-status", "pid": child.pid, "status": status})
                command = receive_command(observer_control)
            require(command == b"X", "observer did not release supervisor return")
            return 0
        except BaseException:
            native.stop(child)
            raise


def exchange(connection, request):
    connection.settimeout(TIMEOUT)
    connection.sendall(struct.pack(">I", len(request)) + request)
    length = struct.unpack(">I", native.receive_exact(connection, 4, time.monotonic() + TIMEOUT))[0]
    require(0 < length <= native.FRAME_LIMIT, "agent response length was refused")
    return native.receive_exact(connection, length, time.monotonic() + TIMEOUT)


def open_signed_connection(path, public_blob, expected_comment, message):
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        connection.settimeout(TIMEOUT)
        connection.connect(str(path))
        reply = exchange(connection, b"\x0b")
        reader = native.Reader(reply)
        require(reader.take(1) == b"\x0c" and reader.number() == 1,
                "native agent did not expose exactly one identity")
        require(reader.string(128) == public_blob, "native agent identity changed")
        require(reader.string(256) == expected_comment.encode("ascii"),
                "agent identity comment differs from the encrypted fixture")
        reader.finish()
        return connection, signature_on_connection(connection, public_blob, message)
    except BaseException:
        connection.close()
        raise


def signature_on_connection(connection, public_blob, message):
    request = b"\x0d" + native.wire_string(public_blob) + native.wire_string(message) + struct.pack(">I", 0)
    signature = exchange(connection, request)
    reader = native.Reader(signature)
    require(reader.take(1) == b"\x0e", "agent signature on the open connection was refused")
    signature_blob = native.Reader(reader.string(128))
    reader.finish()
    require(signature_blob.string(32) == b"ssh-ed25519", "agent signature algorithm was refused")
    value = signature_blob.string(64)
    signature_blob.finish()
    require(len(value) == 64, "agent signature length was refused")
    return value


def observe_unlinked_agent(root, name, pidfd, connection, agent_socket, public_blob, comment,
                           message, paths, proof_value, env):
    require(not pidfd_ready(pidfd, 0), "unlinked native agent did not survive its parent")
    signature = signature_on_connection(connection, public_blob, message)
    native.verify(paths["verifier"], root, env, native.proof(proof_value, signature),
                  name + "-retained-after", True)
    new_connection = None
    try:
        new_connection, signature = open_signed_connection(agent_socket, public_blob, comment, message)
        native.verify(paths["verifier"], root, env, native.proof(proof_value, signature),
                      name + "-new-after", True)
    finally:
        if new_connection is not None:
            new_connection.close()
    require(not pidfd_ready(pidfd, 0), "unlinked native agent exited during survival observation")
    return {"case": name, "death_trigger": "supervisor-kill-without-parent-death-signal",
            "native_agent_survived_parent": True, "unsafe_for_custody": True,
            "retained_connection_signature_verified_after_parent_death": True,
            "new_connection_signature_verified_after_parent_death": True}


def request_after_death(connection, public_blob, message):
    request = b"\x0d" + native.wire_string(public_blob) + native.wire_string(message) + struct.pack(">I", 0)
    try:
        connection.sendall(struct.pack(">I", len(request)) + request)
    except (BrokenPipeError, ConnectionResetError):
        return "transport_refused"
    try:
        first = connection.recv(4)
    except (BrokenPipeError, ConnectionResetError):
        return "transport_refused"
    if not first:
        return "eof"
    require(len(first) == 4, "agent returned a partial response length")
    length = struct.unpack(">I", first)[0]
    require(0 < length <= native.FRAME_LIMIT, "agent response length was malformed")
    reply = native.receive_exact(connection, length, time.monotonic() + TIMEOUT)
    require(reply in (b"\x05",) or reply[:1] in (b"\x0e",), "agent response type was malformed")
    return "signature" if reply[:1] == b"\x0e" else "refused"


def new_connection_after_death(path, public_blob, message):
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        try:
            connection.settimeout(TIMEOUT)
            connection.connect(str(path))
        except OSError as error:
            if error.errno in (errno.ENOENT, errno.ECONNREFUSED):
                return "connect_refused"
            raise
        result = request_after_death(connection, public_blob, message)
        return result
    finally:
        connection.close()


def create_fixture(root, writer, verifier):
    ssh_agent = shutil.which("ssh-agent", path=os.defpath)
    ssh_add = shutil.which("ssh-add", path=os.defpath)
    require(ssh_agent and ssh_add, "required OpenSSH executables are unavailable")
    device_id, credential_id = native.identifier(), native.identifier()
    comment = "vivac.device-key.v1:" + device_id + ":" + credential_id
    key_path = root / "fixture-key"
    env = private_env(root)
    writer_env = dict(env, VIVAC_OPENSSH_DISPOSABLE="1")
    code, _ = native.bounded_run([str(writer), str(key_path), comment], writer_env,
                                 native.PASSWORD + b"\n" + native.PASSWORD + b"\n")
    require(code == 0, "encrypted fixture creation failed")
    parsed = native.parse_key(key_path)
    require(parsed["cipher"] == "aes256-gcm@openssh.com", "fixture key is not encrypted")
    proof_value = native.challenge(parsed["public_key"], device_id, credential_id)
    message = native.canonical(proof_value)
    return ssh_agent, ssh_add, comment, parsed, proof_value, message, env


def launch_supervisor(root, socket_path, case, *, pause_before_arm=False, unlinked_control=False):
    parent_control, supervisor_control = socket.socketpair()
    process, child_pid, pidfd = None, None, None
    try:
        args = [sys.executable, str(Path(__file__).resolve()), "--internal-supervisor",
                "--agent-socket", str(socket_path), "--home", str(root / "home"),
                "--control-fd", str(supervisor_control.fileno())]
        if unlinked_control:
            args.append("--unlinked-control")
        process = subprocess.Popen(args, env=private_env(root), stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   pass_fds=(supervisor_control.fileno(),),
                                   preexec_fn=native.child_setup, close_fds=True)
        supervisor_control.close()
        parent_control.settimeout(TIMEOUT)
        metadata = receive_json(parent_control, time.monotonic() + TIMEOUT)
        require(metadata["kind"] == "prearm" and metadata["parent"] == process.pid,
                "supervisor reported an unexpected pre-arm executor")
        child_pid = metadata["pid"]
        pidfd = os.pidfd_open(child_pid)
        child_start = process_start(child_pid)
        if not pause_before_arm:
            parent_control.sendall(b"A")
            metadata = receive_json(parent_control, time.monotonic() + TIMEOUT)
            require(metadata["kind"] == "executor" and metadata["parent"] == process.pid,
                    "supervisor reported an unexpected executor")
            require(metadata["pid"] == child_pid and process_start(child_pid) == child_start,
                    "executor identity changed across arming")
        return process, parent_control, child_pid, child_start, metadata, pidfd
    except BaseException:
        cleanup_supervisor(process, parent_control, child_pid, pidfd)
        raise
    finally:
        supervisor_control.close()


def stop_supervisor(process):
    if process is not None and process.poll() is None:
        process.kill()
        process.wait(timeout=TIMEOUT)


def stop_agent(pidfd):
    if not pidfd_ready(pidfd, 0):
        try:
            pidfd_signal(pidfd, signal.SIGKILL)
        except ProcessLookupError:
            pass
        require(pidfd_ready(pidfd), "owned agent cleanup failed")


def cleanup_supervisor(process, control, child_pid, pidfd, socket_path=None):
    # ExitStack attempts every callback even if an earlier cleanup fails.
    with ExitStack() as cleanup:
        if socket_path is not None:
            cleanup.callback(unlink_owned_socket, socket_path)
        if child_pid is not None:
            cleanup.callback(reap_owned, child_pid)
        if pidfd is not None:
            cleanup.callback(os.close, pidfd)
            cleanup.callback(stop_agent, pidfd)
        cleanup.callback(control.close)
        cleanup.callback(stop_supervisor, process)


@contextmanager
def owned_supervisor(root, socket_path, case, *, pause_before_arm=False, unlinked_control=False):
    values = launch_supervisor(root, socket_path, case, pause_before_arm=pause_before_arm,
                               unlinked_control=unlinked_control)
    process, control, child_pid, _, _, pidfd = values
    try:
        yield values
    finally:
        cleanup_supervisor(process, control, child_pid, pidfd, socket_path)


def parent_agent_status(control, child_pid):
    control.sendall(b"R")
    metadata = receive_json(control, time.monotonic() + TIMEOUT)
    require(metadata.get("kind") == "agent-status" and metadata.get("pid") == child_pid,
            "parent reported an unexpected native agent status")
    status = metadata.get("status")
    require(type(status) is int and os.WIFSIGNALED(status)
            and os.WTERMSIG(status) == signal.SIGKILL,
            "parent did not report native agent SIGKILL")
    return status


def wait_socket(path, process, pid):
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        require(process.poll() is None, "supervisor exited before agent readiness")
        require(Path("/proc", str(pid)).exists(), "executor exited before agent readiness")
        if path.exists():
            try:
                native.identities(path)
                return
            except (OSError, native.ProbeError):
                pass
        time.sleep(0.02)
    raise ProbeError("native agent readiness timed out")


def inspect_agent(pid, expected_start, expected_exe, expected_parent):
    require(process_start(pid) == expected_start, "agent process identity changed")
    exe_observation = "verified"
    try:
        require(Path(executable(pid)).resolve(strict=True) == Path(expected_exe).resolve(strict=True),
                "agent executable identity changed")
    except PermissionError:
        argv0 = Path("/proc", str(pid), "cmdline").read_bytes().split(b"\0", 1)[0]
        require(argv0 == os.fsencode(expected_exe), "agent command identity changed")
        exe_observation = "exe_link_denied_cmdline_matches"
    values = agent_credentials(pid)
    require(values["PPid"] == [expected_parent], "agent parent identity changed")
    require(values["NoNewPrivs"] == [1], "agent lost no_new_privs across exec")
    require(len(values["Uid"]) == 4 and len(values["Gid"]) == 4,
            "agent credential tuple was incomplete")
    require(values["Uid"] == [os.geteuid()] * 4 and values["Gid"] == [os.getegid()] * 4,
            "agent credentials differ from the unprivileged observer")
    return {"credentials": values, "executable_observation": exe_observation}


def reap_owned(pid):
    deadline = time.monotonic() + DEATH_TIMEOUT
    try:
        while True:
            result, status = os.waitpid(pid, os.WNOHANG)
            if result == pid:
                return status
            if not Path("/proc", str(pid)).exists():
                return None
            require(time.monotonic() < deadline, "owned child reap timed out")
            time.sleep(0.01)
    except ChildProcessError:
        return None


def run_case(root, name, death, paths, fixture):
    global CURRENT_STAGE
    CURRENT_STAGE = name
    agent_socket = root / (name + ".sock")
    with owned_supervisor(root, agent_socket, name,
                          unlinked_control=death == "unlinked-parent-kill") as values:
        return run_loaded_case(root, name, death, paths, fixture, values)


def run_loaded_case(root, name, death, paths, fixture, values):
    ssh_agent, ssh_add, comment, parsed, proof_value, message, env = fixture
    agent_socket = root / (name + ".sock")
    process, control, child_pid, child_start, metadata, pidfd = values
    connection = None
    try:
        control.sendall(b"G")
        wait_socket(agent_socket, process, child_pid)
        expected_exe = shutil.which("ssh-agent", path=os.defpath)
        before = inspect_agent(child_pid, child_start, expected_exe, process.pid)
        code, _ = native.pty_run([ssh_add, "-k", str(paths["key"])],
                                 private_env(root, agent_socket), [native.PASSWORD])
        require(code == 0, "encrypted fixture failed native agent load")
        signature = native.sign(agent_socket, parsed["public_blob"], message)
        native.verify(paths["verifier"], root, env,
                      native.proof(proof_value, signature), name + "-proof", True)
        connection, prior_signature = open_signed_connection(agent_socket,
                                                              parsed["public_blob"], comment, message)
        native.verify(paths["verifier"], root, env,
                      native.proof(proof_value, prior_signature), name + "-open-proof", True)
        if death == "normal":
            pass
        elif death == "term":
            process.send_signal(signal.SIGTERM)
            process.wait(timeout=TIMEOUT)
        elif death == "kill":
            process.send_signal(signal.SIGKILL)
            process.wait(timeout=TIMEOUT)
        elif death == "agent-kill":
            pidfd_signal(pidfd, signal.SIGKILL)
        elif death == "unlinked-parent-kill":
            process.send_signal(signal.SIGKILL)
            process.wait(timeout=TIMEOUT)
            return observe_unlinked_agent(root, name, pidfd, connection, agent_socket,
                                          parsed["public_blob"], comment, message, paths, proof_value, env)
        else:
            raise ProbeError("unknown lifecycle case")
        if death == "normal":
            control.sendall(b"X")
            process.wait(timeout=TIMEOUT)
        dead = pidfd_ready(pidfd)
        require(dead, "agent process did not exit within the bounded deadline")
        if connection is not None:
            open_result = request_after_death(connection, parsed["public_blob"], message)
            require(open_result != "signature", "already-open agent connection signed after death")
        else:
            open_result = "unavailable"
        new_result = new_connection_after_death(agent_socket, parsed["public_blob"], message)
        require(new_result != "signature", "new agent connection signed after death")
        after = None
        try:
            after = agent_credentials(child_pid)
        except (OSError, ProbeError):
            pass
        if death == "agent-kill":
            status = parent_agent_status(control, child_pid)
            control.sendall(b"X")
            process.wait(timeout=TIMEOUT)
            status_source = "parent"
        else:
            status = reap_owned(child_pid)
            status_source = "subreaper"
        require(status is not None and os.WIFSIGNALED(status)
                and os.WTERMSIG(status) == signal.SIGKILL,
                "agent did not report kernel SIGKILL through wait status")
        return {"case": name, "death_trigger": death, "agent_pidfd_ready_within_3s": True,
                "agent_dead": dead, "open_connection_result_after_death": open_result,
                "new_connection_result_after_death": new_result,
                "socket_path_remained": agent_socket.exists(), "credentials_before": before,
                "credentials_after": after, "supervisor_returncode": process.returncode,
                "agent_wait_signal": "SIGKILL", "agent_wait_status_source": status_source}
    finally:
        if connection is not None:
            connection.close()


def run_race_case(root, name, *, before_arm):
    global CURRENT_STAGE
    CURRENT_STAGE = name
    agent_socket = root / (name + ".sock")
    with owned_supervisor(root, agent_socket, name, pause_before_arm=before_arm) as values:
        process, control, child_pid, child_start, metadata, pidfd = values
        process.send_signal(signal.SIGKILL)
        process.wait(timeout=TIMEOUT)
        require(pidfd_ready(pidfd), "executor survived supervisor death at startup barrier")
        require(not agent_socket.exists(), "startup race created a native agent socket")
        status = reap_owned(child_pid)
        require(status is not None, "startup executor was not reaped by the subreaper")
        if before_arm:
            require(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 78,
                    "pre-arm executor did not reject the parent mismatch after arming")
            result = "parent_mismatch_refused_after_arm"
        else:
            require(os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL,
                    "armed pre-exec executor did not receive kernel SIGKILL")
            result = "sigkill_before_exec"
        return {"case": name, "executor_pid": child_pid,
                "parent_death_result": result, "socket_created": False}


def run_probe(writer, verifier):
    global CURRENT_STAGE, CURRENT_REPORT
    CURRENT_STAGE = "preflight"
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "cases": [], "sentinel_survived": False, "limitations": [
                  "probe observes process lifetime and signature refusal only"]}
    CURRENT_REPORT = report
    require(sys.platform == "linux", "this probe requires Linux")
    require(os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
            "disposable-container declaration is absent")
    require_isolation()
    require_unprivileged()
    require(Path("/proc").is_dir() and hasattr(os, "pidfd_open"),
            "Linux procfs or pidfd support is unavailable")
    report["resource_limits"] = require_resources()
    prctl(PR_SET_CHILD_SUBREAPER, 1)
    require(prctl(PR_GET_NO_NEW_PRIVS) == 1, "disposable observer is missing no_new_privs")
    with tempfile.TemporaryDirectory(prefix="vivac-lifecycle-", dir="/tmp") as temporary:
        CURRENT_STAGE = "fixture"
        root = Path(temporary).resolve()
        root.chmod(0o700)
        (root / "home").mkdir(mode=0o700)
        ssh_agent, ssh_add, comment, parsed, proof_value, message, env = create_fixture(root, writer, verifier)
        key_path = root / "fixture-key"
        paths = {"verifier": verifier, "key": key_path}
        sentinel_socket = root / "sentinel.sock"
        with owned_sentinel(ssh_agent, root, sentinel_socket) as (sentinel, sentinel_pidfd):
            CURRENT_STAGE = "sentinel-baseline"
            wait_socket(sentinel_socket, sentinel, sentinel.pid)
            code, _ = native.pty_run([ssh_add, "-k", str(key_path)],
                                     private_env(root, sentinel_socket), [native.PASSWORD])
            require(code == 0, "sentinel agent fixture load failed")
            sentinel_signature = native.sign(sentinel_socket, parsed["public_blob"], message)
            native.verify(verifier, root, env, native.proof(proof_value, sentinel_signature),
                          "sentinel-before", True)
            fixture = (ssh_agent, ssh_add, comment, parsed, proof_value, message, env)
            for name, trigger in (("normal-return", "normal"), ("term", "term"),
                                  ("kill", "kill"), ("direct-agent-kill", "agent-kill")):
                report["cases"].append(run_case(root, name, trigger, paths, fixture))
                require(sentinel.poll() is None and not pidfd_ready(sentinel_pidfd, 0),
                        "sentinel process was affected by an owned-agent case")
                signature = native.sign(sentinel_socket, parsed["public_blob"], message)
                native.verify(verifier, root, env, native.proof(proof_value, signature),
                              "sentinel-after-" + name, True)
            for name, before_arm in (("pre-arm-parent-death", True),
                                     ("post-arm-pre-exec-parent-death", False)):
                report["cases"].append(run_race_case(root, name, before_arm=before_arm))
                require(sentinel.poll() is None and not pidfd_ready(sentinel_pidfd, 0),
                        "sentinel process was affected by a startup race")
                signature = native.sign(sentinel_socket, parsed["public_blob"], message)
                native.verify(verifier, root, env, native.proof(proof_value, signature),
                              "sentinel-after-" + name, True)
            name = "native-unlinked-parent-kill"
            report["cases"].append(run_case(root, name, "unlinked-parent-kill", paths, fixture))
            require(sentinel.poll() is None and not pidfd_ready(sentinel_pidfd, 0),
                    "sentinel process was affected by the unlinked control")
            signature = native.sign(sentinel_socket, parsed["public_blob"], message)
            native.verify(verifier, root, env, native.proof(proof_value, signature),
                          "sentinel-after-" + name, True)
            report["sentinel_survived"] = True
    report["status"] = "observed"
    return report


@contextmanager
def owned_sentinel(ssh_agent, root, sentinel_socket):
    with ExitStack() as cleanup:
        cleanup.callback(unlink_owned_socket, sentinel_socket)
        sentinel = subprocess.Popen([ssh_agent, "-D", "-a", str(sentinel_socket)],
                                    env=private_env(root), stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                    preexec_fn=native.child_setup, close_fds=True)
        cleanup.callback(native.stop, sentinel)
        sentinel_pidfd = os.pidfd_open(sentinel.pid)
        cleanup.callback(os.close, sentinel_pidfd)
        cleanup.callback(stop_agent, sentinel_pidfd)
        yield sentinel, sentinel_pidfd


def private_write(path, data):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as target:
        target.write(data)
        target.flush()
        os.fsync(target.fileno())


def main():
    global CURRENT_REPORT
    CURRENT_REPORT = None
    parser = argparse.ArgumentParser()
    parser.add_argument("--writer", type=Path)
    parser.add_argument("--verifier", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--internal-supervisor", action="store_true")
    parser.add_argument("--internal-agent-exec", action="store_true")
    parser.add_argument("--unlinked-control", action="store_true")
    parser.add_argument("--expected-parent", type=int)
    parser.add_argument("--agent-socket")
    parser.add_argument("--home")
    parser.add_argument("--control-fd", type=int)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux", "this probe requires Linux")
        require(os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "disposable-container declaration is absent")
        require_isolation()
        require_unprivileged()
        require(prctl(PR_GET_NO_NEW_PRIVS) == 1,
                "disposable observer is missing no_new_privs")
        require(not args.unlinked_control or args.internal_agent_exec or args.internal_supervisor,
                "unlinked control is restricted to the experimental internal launcher")
        if args.internal_agent_exec:
            return agent_exec(args)
        if args.internal_supervisor:
            return supervisor(args)
        require(args.writer is not None and args.verifier is not None and args.report is not None,
                "writer, verifier, and report are required")
        writer = args.writer.resolve(strict=True)
        verifier = args.verifier.resolve(strict=True)
        require(writer.is_file() and os.access(writer, os.X_OK), "writer executable unavailable")
        require(verifier.is_file() and os.access(verifier, os.X_OK), "verifier executable unavailable")
        require(args.report.is_absolute(), "report path must be absolute")
        report_path = args.report.parent.resolve(strict=True) / args.report.name
        require(not report_path.exists() and not report_path.is_symlink(), "report already exists")
        require(os.access(report_path.parent, os.W_OK | os.X_OK),
                "report directory is not writable before fixture creation")
        result = run_probe(writer, verifier)
        private_write(report_path, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Lifecycle observations recorded; custody_accredited=false")
        return 0
    except ParentChanged:
        return 78
    except (ProbeError, OSError, ValueError, subprocess.SubprocessError, native.ProbeError) as error:
        if isinstance(error, (ProbeError, native.ProbeError)):
            failure = str(error)
        else:
            failure = {"error_type": type(error).__name__,
                       "errno": getattr(error, "errno", None)}
        if args.report is not None and args.report.is_absolute():
            try:
                report_path = args.report.parent.resolve(strict=True) / args.report.name
                if not report_path.exists() and not report_path.is_symlink():
                    payload = dict(CURRENT_REPORT or {})
                    payload.update({"custody_accredited": False, "failure": failure,
                                    "stage": CURRENT_STAGE, "status": "incomplete", "version": 1})
                    private_write(report_path, json.dumps(payload, sort_keys=True).encode("ascii") + b"\n")
            except OSError:
                pass
        print("Lifecycle probe failed at stage " + CURRENT_STAGE + "; no result is accredited.",
              file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
