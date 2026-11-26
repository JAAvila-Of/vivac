#!/usr/bin/env python3
"""Run the keeper opening-permit experiment over a disposable SSH terminal."""

import json
import os
from pathlib import Path
import re
import socket
import sys


def worker(arguments):
    if len(arguments) != 5:
        raise ValueError("SSH worker argument count differs")
    reader, mode, credential, home, socket_path = arguments
    if not (reader == "/launcher-build/launcher189/debug/rust-launcher-probe" and Path(home).is_absolute() and Path(socket_path).is_absolute()
            and mode in ("permit-close", "permit-keeper-lost", "permit-unconfirmed", "permitted")
            and re.fullmatch(r"[0-7][0123456789abcdefghjkmnpqrstvwxyz]{25}", credential)
            and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1"
            and os.getuid() == os.geteuid() == os.getgid() == os.getegid() == 65534
            and os.isatty(0) and os.getpgrp() == os.tcgetpgrp(0)):
        raise ValueError("SSH worker identity differs")
    terminal = os.ttyname(0)
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(5)
    connection.connect(socket_path)
    data = Path("/proc/self/stat").read_text(encoding="ascii")
    start = int(data.rsplit(")", 1)[1].split()[19])
    packet = dict(kind="worker", pid=os.getpid(), start=start,
                  parent=os.getppid(), stdin_is_tty=True, terminal=terminal)
    connection.sendall(json.dumps(packet, separators=(",", ":")).encode("ascii") + b"\n")
    if connection.recv(1) != b"B":
        raise ValueError("SSH worker was not claimed")
    connection.setblocking(True)
    for descriptor in (0, 1, 2):
        os.dup2(connection.fileno(), descriptor)
    connection.close()
    command = ([reader, "--endpoint-permitted-custodian", credential, home] if mode == "permitted"
               else [reader, "--endpoint-active-custodian", credential, home, mode])
    os.execve(reader, command, {"PATH": os.defpath, "HOME": str(Path(home) / "home"),
                               "LANG": "C", "LC_ALL": "C", "SSH_ASKPASS_REQUIRE": "never",
                               "VIVAC_OPENSSH_DISPOSABLE": "1"})


if __name__ == "__main__" and sys.argv[1:2] == ["--worker"]:
    worker(sys.argv[2:])
    sys.exit("SSH worker did not exec")


from contextlib import contextmanager
import hashlib
import selectors
import shlex
import signal
import stat
import struct
import subprocess
import tempfile
import time

import creation_draft_probe as draft
import creation_native_probe as native
import creation_reader_probe as joint
import generated_probe as generated
import lifecycle_probe as life
import rust_launcher_probe as launcher
import ssh_lifecycle_probe as ssh

require = generated.require
PUBLISHED = b'{"clean_close_permit_published":true}\n'
UNCONFIRMED = b'{"clean_close_permit_refused":true}\n'
REFUSED = b'{"endpoint_failure":"opening-permit","kind":"NotFound"}\n'
NO_TERMINAL = b'{"endpoint_failure":"opening-permit","kind":"PermissionDenied"}\n'
SUPERVISOR_START = b"VVKSTART\x01" + bytes(7)
REGISTRATION_REFUSED = b"Rust process reference transport experiment failed.\n"


def read_until(stream, deadline, limit, *, line=False):
    received = bytearray()
    with selectors.DefaultSelector() as selector:
        selector.register(stream, selectors.EVENT_READ)
        while len(received) < limit:
            remaining = deadline - time.monotonic()
            require(remaining > 0 and selector.select(remaining), "SSH observation timed out")
            chunk = os.read(stream.fileno(), 1 if line else limit - len(received))
            if not chunk:
                break
            received.extend(chunk)
            if line and chunk == b"\n":
                return bytes(received)
    require(not line, "SSH control line incomplete")
    return bytes(received)


def client_arguments(transport, tty, remote):
    return [transport.ssh, tty, "-F", "/dev/null", "-p", str(transport.port),
                 "-i", str(transport.client_key), "-oIdentitiesOnly=yes", "-oBatchMode=yes",
                 "-oStrictHostKeyChecking=yes", "-oUserKnownHostsFile=" + str(transport.known_hosts),
                 "-oGlobalKnownHostsFile=/dev/null", "-oForwardAgent=no", "-oClearAllForwardings=yes",
                 "-oControlMaster=no", "-oControlPath=none", "-oConnectTimeout=5",
                 "-oLogLevel=DEBUG1" if transport.diagnostic else "-oLogLevel=ERROR",
                 "nobody@127.0.0.1", "VIVAC_OPENSSH_DISPOSABLE=1 exec " + shlex.join(remote)]


def launch_client(transport, reader, mode, credential, home, control):
    remote = [sys.executable, "-S", "-B", str(Path(__file__).resolve()),
              "--worker", str(reader), mode, credential, str(home), str(control)]
    return subprocess.Popen(client_arguments(transport, "-tt", remote), env=transport.env,
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, close_fds=True)


def check_opening_without_pty(transport, reader, credential, home, permit, expected):
    before = permit.lstat()
    home.mkdir(mode=0o700)
    remote = [str(reader), "--endpoint-permitted-custodian", credential, str(home)]
    refused = subprocess.run(client_arguments(transport, "-T", remote), env=transport.env,
                             stdin=subprocess.DEVNULL, capture_output=True, timeout=20, check=False)
    require(permit.is_file() and not permit.is_symlink(), "SSH without a PTY consumed the opening permit")
    after = permit.lstat()
    require((after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
            == (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
            and permit.read_bytes() == expected, "SSH without a PTY changed the opening permit")
    require(refused.returncode == 1 and refused.stdout == NO_TERMINAL and refused.stderr == b"",
            "SSH without a PTY did not refuse at the opening preflight")
    home.rmdir()


def check_supervisor_start_refusals(reader, credential, home, permit, expected):
    before = permit.lstat()
    home.mkdir(mode=0o700)
    command = [str(reader), "--registration-active-supervisor", credential, str(home)]
    env = {"PATH": os.defpath, "LANG": "C", "VIVAC_OPENSSH_DISPOSABLE": "1"}
    direct = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True,
                            env=env, timeout=5, check=False)
    require(direct.returncode == 1 and direct.stdout == b"" and direct.stderr == REGISTRATION_REFUSED,
            "Supervisor accepted direct invocation without inherited descriptors")
    for socket_type in (socket.SOCK_STREAM, socket.SOCK_SEQPACKET):
        parent, child = socket.socketpair(socket.AF_UNIX, socket_type)
        observer, child_control = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        with parent, child, observer, child_control:
            for channel in (parent, child):
                channel.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
            parent.sendall(SUPERVISOR_START)
            descriptors = (child.fileno(), child_control.fileno())
            refused = subprocess.run(command + [str(fd) for fd in descriptors] + ["opening-permit"],
                                     stdin=subprocess.DEVNULL, capture_output=True, env=env,
                                     pass_fds=descriptors, timeout=8, check=False)
            observer.settimeout(2)
            require(refused.returncode == 1 and refused.stdout == b"" and refused.stderr == REGISTRATION_REFUSED
                    and observer.recv(2) == b"X", "Supervisor did not refuse foreign startup at the channel check")
    after = permit.lstat()
    require((after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
            == (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
            and permit.read_bytes() == expected, "Foreign supervisor startup changed the opening permit")
    home.rmdir()


@contextmanager
def remote_session(transport, reader, mode, credential, home, control, report=None):
    import termios

    home.mkdir(mode=0o700)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(control))
        control.chmod(0o600)
        listener.listen(1)
        client = launch_client(transport, reader, mode, credential, home, control)
        keeper = terminal = ssh_session = None
        try:
            started = time.monotonic()
            deadline = started + 35
            with selectors.DefaultSelector() as selector:
                selector.register(listener, selectors.EVENT_READ)
                while not selector.select(0.05):
                    if client.poll() is not None or time.monotonic() >= deadline:
                        if report is not None:
                            # The worker cannot receive a secret before this handshake.
                            for stream in (client.stdout, client.stderr):
                                os.set_blocking(stream.fileno(), False)
                            report["keeper_ssh_bootstrap_failure"] = {
                                "exit": client.returncode,
                                "stdout": (client.stdout.read(2048) or b"").decode("ascii", "replace"),
                                "stderr": (client.stderr.read(4096) or b"").decode("ascii", "replace"),
                            }
                            if transport.diagnostic:
                                report["processes_at_deadline"] = bootstrap_processes()
                        require(False, "SSH worker did not establish its local channel")
            channel, _ = listener.accept()
            if transport.diagnostic and report is not None:
                report["worker_channel_seconds"] = round(time.monotonic() - started, 3)
            with channel:
                channel.settimeout(5)
                metadata = json.loads(read_until(channel, time.monotonic() + 5, 1024, line=True))
                terminal_name = metadata.pop("terminal", None)
                require(type(terminal_name) is str and re.fullmatch(r"/dev/pts/[0-9]+", terminal_name),
                        "SSH worker terminal name differs")
                keeper = ssh.claim_worker(channel, metadata, transport.server.pid, transport.server_start, True)
                require(metadata["parent"] != transport.server.pid, "SSH worker did not have a session process")
                ssh_session = os.pidfd_open(metadata["parent"])
                require(not life.pidfd_ready(ssh_session, 0), "SSH session process was already gone")
                pid = metadata["pid"]
                terminal = os.open(terminal_name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOCTTY | os.O_NOFOLLOW)
                descriptor = os.fstat(terminal)
                inherited = Path("/proc", str(pid), "fd/0").stat()
                require(os.isatty(terminal) and descriptor.st_uid == os.geteuid()
                        and (descriptor.st_dev, descriptor.st_ino) == (inherited.st_dev, inherited.st_ino),
                        "SSH worker did not hold the observed PTY")
                original = termios.tcgetattr(terminal)
                channel.sendall(b"B")
                yield client, channel, pid, keeper, terminal, original, ssh_session
        finally:
            if client.poll() is None:
                client.kill()
                client.wait(timeout=5)
            for stream in (client.stdin, client.stdout, client.stderr):
                stream.close()
            for descriptor in (keeper, terminal, ssh_session):
                if descriptor is not None:
                    os.close(descriptor)
            control.unlink()


def record_stage(stages, field, start):
    now = time.monotonic_ns()
    if stages is not None:
        stages[field + "_ns"] = now - start
    return now


def unlock(client, channel, terminal, pid, reader, secret, permit, deadline, report, stages, role):
    import termios

    expect = native.ENDPOINT_UNLOCK_PROMPT
    start = time.monotonic_ns()
    prompt = read_until(client.stdout, deadline, len(expect))
    if prompt != expect:
        # Only capture these streams before the observer sends any secret.
        os.set_blocking(client.stderr.fileno(), False)
        os.set_blocking(channel.fileno(), False)
        report["keeper_ssh_prompt_failure"] = {
            "prefix": prompt.decode("ascii", "replace"),
            "exit": client.poll(),
            "stderr": (client.stderr.read(1024) or b"").decode("ascii", "replace"),
            "control": (channel.makefile("rb", buffering=0).read(1024) or b"").decode("ascii", "replace"),
        }
        require(False, "SSH unlock prompt differs")
    observed_prompt = record_stage(stages, role + "_channel_to_prompt", start)
    echo = sum(getattr(termios, name, 0) for name in
               ("ECHO", "ECHONL", "ECHOE", "ECHOK", "ECHOKE", "ECHOCTL", "ECHOPRT"))
    require(Path(life.executable(pid)).resolve(strict=True) == reader.resolve(strict=True)
            and not termios.tcgetattr(terminal)[3] & echo
            and not permit.exists() and not permit.is_symlink(), "SSH prompt preceded consumption or hid no echo")
    generated.proc_secret_absent(pid, secret)
    payload = bytearray(secret)
    payload.append(10)
    try:
        client.stdin.write(payload)
        client.stdin.flush()
        return record_stage(stages, role + "_prompt_to_input", observed_prompt)
    finally:
        generated.wipe(payload)


def signature(args, credential, home, client, secret, deadline, report, stages, role, input_sent):
    native.active_endpoint_socket(home / "endpoint.sock", client, deadline, report,
                                  "SSH custodian did not open the local proof socket")
    endpoint = record_stage(stages, role + "_input_to_endpoint", input_sent)
    verifier = subprocess.Popen([str(args.reader), "--endpoint-native-verifier", credential, str(home)],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                env=life.private_env(home), cwd="/", close_fds=True)
    try:
        packet = launcher.bounded_output(verifier, deadline, first_line=True)
        native.record_endpoint_failure(report, "ssh-independent-verifier", packet)
        evidence = native.endpoint_packet(packet, "verifier", secret)
        native.release_endpoint_process(verifier, deadline)
        record_stage(stages, role + "_endpoint_to_verified_signature", endpoint)
        return evidence["generation"]
    finally:
        if verifier.poll() is None:
            verifier.kill()
            verifier.wait(timeout=5)
        for stream in (verifier.stdin, verifier.stdout, verifier.stderr):
            stream.close()


def ssh_exit(client, channel, keeper, expected, deadline):
    require(client.wait(timeout=max(0.001, deadline - time.monotonic())) == expected
            and life.pidfd_ready(keeper, 5), "SSH client or remote worker exit differs")
    require(read_until(channel, deadline, 1) == b""
            and read_until(client.stdout, deadline, 1) == b""
            and read_until(client.stderr, deadline, 1) == b"", "unexpected SSH terminal output")


def disconnect(client, channel, keeper_pid, keeper, ssh_session, args, credential, home, deadline):
    import fcntl

    previous = home / "previous"
    cut = read_until(channel, deadline, 1024, line=True)
    supervisor = native.parse_active_keeper_cut(cut, keeper_pid)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as agent_connection:
        agent_connection.settimeout(2)
        agent_connection.connect(str(previous / "agent.sock"))
        agent, uid, gid = struct.unpack("3i", agent_connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    require(uid == os.geteuid() and gid == os.getegid()
            and len({keeper_pid, supervisor, agent}) == 3, "SSH native participants differ")
    socket_types = {"endpoint.sock": socket.SOCK_SEQPACKET, "agent.sock": socket.SOCK_STREAM}
    identities = {}
    for name in socket_types:
        metadata = (previous / name).lstat()
        require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid(),
                "SSH native socket identity differs")
        identities[name] = (metadata.st_dev, metadata.st_ino)
    descriptors = []
    try:
        for pid, parent, exe in ((supervisor, keeper_pid, args.reader), (agent, supervisor, "/usr/bin/ssh-agent")):
            start = life.process_start(pid)
            descriptor = os.pidfd_open(pid)
            descriptors.append(descriptor)
            life.inspect_agent(pid, start, str(exe), parent)
        descriptors.append(os.pidfd_open(client.pid))
        require(client.poll() is None and not life.pidfd_ready(keeper, 0)
                and all(not life.pidfd_ready(fd, 0) for fd in descriptors), "SSH death cut was already closed")
        life.pidfd_signal(descriptors[-1], signal.SIGKILL)
        require(client.wait(timeout=5) == -signal.SIGKILL and life.pidfd_ready(keeper, 5)
                and life.pidfd_ready(ssh_session, 5)
                and all(life.pidfd_ready(fd, 5) for fd in descriptors),
                "SSH disconnect did not end every registered process")
        require(read_until(channel, deadline, 1) == b"" and read_until(client.stdout, deadline, 1) == b""
                and read_until(client.stderr, deadline, 1) == b"", "SSH disconnect left output pending")
        for pid in (supervisor, agent):
            life.reap_owned(pid)
        package = draft.PACKAGES / credential
        directory = draft.directory(package, os.geteuid(), 0o700)
        try:
            fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(directory)
        for name, socket_type in socket_types.items():
            stale = previous / name
            with socket.socket(socket.AF_UNIX, socket_type) as connection:
                connection.settimeout(2)
                try:
                    connection.connect(str(stale))
                except ConnectionRefusedError:
                    pass
                else:
                    require(False, "SSH disconnect left a reachable native socket")
            metadata = stale.lstat()
            require((metadata.st_dev, metadata.st_ino) == identities[name],
                    "SSH native socket changed at disconnect")
            life.unlink_owned_socket(stale)
        previous.rmdir()
    finally:
        for descriptor in descriptors:
            os.close(descriptor)


def observe(args, credential, secret, runtime, report, mode):
    import termios

    root = Path(tempfile.mkdtemp(prefix="keeper-ssh-", dir=runtime))
    transport = ssh.SSHTransport(root)
    permit = draft.USER / "state/openings-v1" / credential
    record = (draft.ADMISSIONS / credential / "record.json").read_bytes()
    expected = b"VVCOPEN\x00" + bytes((1, 1)) + bytes(6) + hashlib.sha256(record).digest()
    modes = {"normal": "permit-close", "disconnect": "permit-keeper-lost",
             "unconfirmed": "permit-unconfirmed"}
    require(permit.read_bytes() == expected and mode in modes, "SSH opening selection differs")
    deadline = time.monotonic() + 95
    old_home, new_home, rejected_home = (root / name for name in ("old", "new", "rejected"))
    stages = report.get("operational_measurement", {}).get("ssh")
    completed = False
    try:
        check_supervisor_start_refusals(args.reader, credential, root / "foreign-start", permit, expected)
        start = time.monotonic_ns()
        transport.start()
        preflight = time.monotonic_ns()
        check_opening_without_pty(transport, args.reader, credential, root / "no-pty", permit, expected)
        start += time.monotonic_ns() - preflight
        with remote_session(transport, args.reader, modes[mode], credential, old_home, root / "old.sock",
                            report) as session:
            client, channel, pid, keeper, terminal, original, ssh_session = session
            record_stage(stages, "transport_to_predecessor_channel", start)
            sent = unlock(client, channel, terminal, pid, args.reader, secret, permit, deadline,
                          report, stages, "predecessor")
            old_generation = signature(args, credential, old_home / "previous", client, secret,
                                       deadline, report, stages, "predecessor", sent)
            require(termios.tcgetattr(terminal) == original, "SSH predecessor terminal was not restored")
            release = time.monotonic_ns()
            channel.sendall(b"G")
            if mode == "normal":
                require(read_until(channel, deadline, 1024, line=True) == PUBLISHED, "SSH clean close publication differs")
                native.inspect_clean_close_permit(args, credential, runtime, report)
                permit_observed = record_stage(stages, "predecessor_release_to_permit", release)
                channel.sendall(b"G")
                ssh_exit(client, channel, keeper, 0, deadline)
                record_stage(stages, "permit_observed_to_predecessor_exit", permit_observed)
            elif mode == "unconfirmed":
                require(read_until(channel, deadline, 1024, line=True) == UNCONFIRMED,
                        "SSH produced a permit without authenticated confirmation")
                ssh_exit(client, channel, keeper, 0, deadline)
            else:
                disconnect(client, channel, pid, keeper, ssh_session, args, credential, old_home, deadline)
        old_home.rmdir()
        if mode == "normal":
            start = time.monotonic_ns()
            with remote_session(transport, args.reader, "permitted", credential, new_home, root / "new.sock",
                                report) as session:
                client, channel, pid, keeper, terminal, original, _ = session
                record_stage(stages, "successor_transport_to_channel", start)
                sent = unlock(client, channel, terminal, pid, args.reader, secret, permit, deadline,
                              report, stages, "successor")
                new_generation = signature(args, credential, new_home, client, secret, deadline,
                                           report, stages, "successor", sent)
                packet = read_until(channel, deadline, 1024, line=True)
                native.record_endpoint_failure(report, "ssh-successor-custodian", packet)
                reply = native.endpoint_packet(packet, "custodian", secret)
                require(new_generation == reply["generation"] != old_generation
                        and termios.tcgetattr(terminal) == original, "SSH successor identity or terminal differs")
                release = time.monotonic_ns()
                channel.sendall(b"G")
                ssh_exit(client, channel, keeper, 0, deadline)
                record_stage(stages, "successor_release_to_exit", release)
            new_home.rmdir()
        require(not permit.exists() and not permit.is_symlink(), "SSH refusal had an available permit")
        start = time.monotonic_ns()
        with remote_session(transport, args.reader, "permitted", credential, rejected_home,
                            root / "rejected.sock", report) as session:
            client, channel, _, keeper, _, _, _ = session
            claimed = record_stage(stages, "refusal_transport_to_channel", start)
            require(read_until(channel, deadline, 1024, line=True) == REFUSED, "SSH reconnect refusal differs")
            ssh_exit(client, channel, keeper, 1, deadline)
            record_stage(stages, "refusal_channel_to_exit", claimed)
        rejected_home.rmdir()
        require(transport.server.poll() is None and not permit.exists()
                and (draft.ADMISSIONS / credential / "record.json").read_bytes() == record,
                "SSH reconnect changed the resident admission or server")
        completed = True
        return dict(mode=mode, ssh_pty_verified=True, no_pty_permit_preserved=True,
                    supervisor_start_refused_foreign_inputs=True,
                    predecessor_signed=True, initial_permit_consumed=True,
                    reconnect_refused_before_prompt=True, descendant_exits_observed=True,
                    clean_close_published=mode == "normal", successor_signature_verified=mode == "normal",
                    disconnect_observed=mode == "disconnect",
                    unconfirmed_closure_refused=mode == "unconfirmed", authority=False)
    finally:
        transport.close()
        if completed:
            for name in ("transport-client", "transport-client.pub", "transport-host", "transport-host.pub",
                         "authorized_keys", "known_hosts", "sshd_config", "sshd.pid"):
                (root / name).unlink(missing_ok=True)
            root.rmdir()


def smoke(report_path):
    root = Path(tempfile.mkdtemp(prefix="keeper-ssh-smoke-", dir="/tmp"))
    transport = ssh.SSHTransport(root, diagnostic=True)
    result = {"status": "incomplete"}
    try:
        transport.start()
        with remote_session(transport, joint.READER, "permitted", "00000000000000000000000001",
                            root / "home", root / "control.sock", result) as session:
            client, channel, _, keeper, _, _, _ = session
            deadline = time.monotonic() + 8
            require(read_until(channel, deadline, 1024, line=True)
                    == b'{"endpoint_failure":"other","kind":"NotFound"}\n', "Unprovisioned probe differs")
            require(client.wait(timeout=5) == 1 and life.pidfd_ready(keeper, 5),
                    "Unprovisioned process did not exit")
        result["status"] = "observed"
    except (OSError, ValueError, generated.ProbeError, subprocess.SubprocessError) as error:
        result["failure"] = type(error).__name__
    finally:
        transport.close()
        log = root / "sshd-diagnostic"
        if log.is_file():
            with log.open("rb") as stream:
                result["sshd"] = stream.read(8192).decode("ascii", "replace")
        report_path.write_text(json.dumps(result, sort_keys=True) + "\n", encoding="ascii")
    return 0 if result["status"] == "observed" else 1


def bootstrap_processes():
    result = []
    for process in Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        try:
            name = (process / "comm").read_text().strip()
            status = (process / "stat").read_text().rsplit(")", 1)[1].split()
            wait = (process / "wchan").read_text().strip()
            files = {}
            for descriptor in ("0", "1", "2"):
                try:
                    files[descriptor] = os.readlink(process / "fd" / descriptor)
                except OSError:
                    pass
            result.append((process.name, name, status[:6], status[11:13], wait, files))
        except OSError:
            continue
    return result[:32]


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--transport-smoke":
        sys.exit(smoke(Path(sys.argv[2])))
    sys.exit("SSH probe selection differs")
