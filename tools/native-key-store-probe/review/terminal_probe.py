#!/usr/bin/env python3
"""Disposable PTY and loopback-SSH checks for the experimental Linux reader."""

import argparse
import copy
import errno
import json
import os
from pathlib import Path
import pty
import selectors
import shlex
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import termios
import time

import openssh_probe as native


PROMPTS = (
    b"New device key passphrase: ",
    b"Confirm device key passphrase: ",
)
ECHO_BITS = termios.ECHO | getattr(termios, "ECHONL", 0)
OUTPUT_LIMIT = 32 * 1024
CASE_DEADLINE = 20
REMOTE_DEADLINE = 20
TIMEOUT = 35
PASSWORD = native.PASSWORD
WRONG_PASSWORD = native.WRONG_PASSWORD
SSH_PRIVATE_FIXTURE = b"ssh-transport-only-public-fixture-2026"


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def environment(root):
    result = native.environment(root)
    result["VIVAC_OPENSSH_DISPOSABLE"] = "1"
    return result


def child_setup(slave, ignored_signals=()):
    import fcntl

    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for signal_number in ignored_signals:
        signal.signal(signal_number, signal.SIG_IGN)


def write_all(descriptor, payload, deadline):
    position = 0
    while position < len(payload):
        try:
            count = os.write(descriptor, payload[position:])
        except BlockingIOError:
            remaining = deadline - time.monotonic()
            require(remaining > 0, "PTY input deadline expired")
            with selectors.DefaultSelector() as selector:
                selector.register(descriptor, selectors.EVENT_WRITE)
                selector.select(min(remaining, 0.1))
            continue
        except OSError as error:
            raise ProbeError("PTY input could not be delivered completely") from error
        require(count > 0, "PTY input write made no progress")
        position += count
    return position


def process_exit(process, timeout=2):
    if process.poll() is not None:
        return process.returncode
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        return process.wait(timeout=2)


def terminal_case(writer, root, env, comment, name, actions, *, no_tty=False,
                  wait_for_timeout=False, ignored_signals=()):
    import fcntl

    destination = root / (name + "-key")
    args = [str(writer)]
    if no_tty:
        args.extend(["--terminal", str(destination), comment])
        process = subprocess.Popen(
            args, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            output, _ = process.communicate(timeout=CASE_DEADLINE)
        except subprocess.TimeoutExpired as error:
            process.kill()
            process.wait(timeout=2)
            raise ProbeError("headless writer did not terminate") from error
        require(len(output) <= OUTPUT_LIMIT, "headless writer output exceeded its bound")
        return {
            "refused_without_file": process.returncode != 0 and not destination.exists(),
            "terminal_restored": None,
        }

    args.extend(["--terminal", str(destination), comment])
    master, slave = pty.openpty()
    original = copy.deepcopy(termios.tcgetattr(slave))
    process = None
    output = bytearray()
    prompt_count = 0
    next_search = 0
    sent_actions = 0
    deadline = time.monotonic() + (CASE_DEADLINE if wait_for_timeout else CASE_DEADLINE)
    started = time.monotonic()
    timeout_started = None
    last_action_time = None
    echo_off_at_prompts = True
    foreground_at_prompts = True
    try:
        process = subprocess.Popen(
            args, env=env, stdin=slave, stdout=slave, stderr=slave,
            preexec_fn=lambda: child_setup(slave, ignored_signals),
        )
        os.set_blocking(master, False)
        with selectors.DefaultSelector() as selector:
            selector.register(master, selectors.EVENT_READ)
            while process.poll() is None:
                require(time.monotonic() < deadline, "terminal reader case timed out")
                for key, _ in selector.select(0.025):
                    try:
                        chunk = os.read(key.fd, 4096)
                    except OSError as error:
                        if error.errno == errno.EIO:
                            continue
                        raise
                    if not chunk:
                        continue
                    output.extend(chunk)
                    require(len(output) <= OUTPUT_LIMIT, "terminal reader output exceeded its bound")
                    if wait_for_timeout:
                        timeout_started = timeout_started or time.monotonic()
                        continue
                    while prompt_count < len(PROMPTS):
                        at = output.find(PROMPTS[prompt_count], next_search)
                        if at < 0:
                            break
                        next_search = at + len(PROMPTS[prompt_count])
                        state = termios.tcgetattr(slave)
                        echo_off_at_prompts &= not bool(state[3] & ECHO_BITS)
                        foreground_at_prompts &= os.tcgetpgrp(master) == os.getpgid(process.pid)
                        require(echo_off_at_prompts, "terminal echo remained enabled at a password prompt")
                        require(foreground_at_prompts, "writer did not own the terminal foreground process group")
                        require(prompt_count < len(actions), "unexpected extra password prompt")
                        kind, value = actions[prompt_count]
                        if kind == "bytes":
                            write_all(master, value + b"\n", deadline)
                        elif kind == "bytes_eof":
                            write_all(master, value + b"\x04", deadline)
                        elif kind == "control":
                            write_all(master, value, deadline)
                        elif kind == "signal":
                            os.kill(process.pid, value)
                        elif kind == "wait":
                            pass
                        else:
                            raise ProbeError("unknown terminal fixture action")
                        sent_actions += 1
                        last_action_time = time.monotonic()
                        prompt_count += 1
                if wait_for_timeout and time.monotonic() - deadline < 0.1:
                    pass
        exit_code = process.wait(timeout=1)
        restored = termios.tcgetattr(slave) == original
        require(restored, "terminal attributes were not restored exactly")
        require(not destination.exists() or destination.is_file(), "writer created a non-file destination")
        if wait_for_timeout:
            require(
                timeout_started is not None and exit_code != 0
                and time.monotonic() - timeout_started >= 14,
                "no-input reader did not refuse at its terminal deadline",
            )
            require(not destination.exists(), "timed-out reader created a key")
        elif exit_code != 0 and actions and last_action_time is not None:
            require(time.monotonic() - last_action_time < 5, "terminal refusal exceeded its response deadline")
        if exit_code == 0:
            profile = native.parse_key(destination)
            require(
                profile["cipher"] == "aes256-gcm@openssh.com" and profile["rounds"] == 64,
                "terminal writer produced an unexpected protection profile",
            )
        return {
            "exit_refused": exit_code != 0,
            "encrypted_file_created": exit_code == 0 and destination.exists(),
            "no_file_on_refusal": exit_code == 0 or not destination.exists(),
            "terminal_restored": restored,
            "echo_off_at_password_prompts": echo_off_at_prompts,
            "foreground_at_password_prompts": foreground_at_prompts,
            "prompts_observed": prompt_count,
            "actions_sent": sent_actions,
            "fixture_echo_detected": PASSWORD in output or WRONG_PASSWORD in output,
            "exit_seconds": round(time.monotonic() - started, 3),
            "reader_deadline_observed": wait_for_timeout,
        }
    except (ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        number = error.errno if isinstance(error, OSError) else None
        raise ProbeError(
            f"PTY case {name} failed ({type(error).__name__}, errno={number})"
        ) from error
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        os.close(master)
        os.close(slave)


def verify_case(name, result, positive=False):
    require(result["terminal_restored"] is True, name + " did not restore terminal state")
    require(result["fixture_echo_detected"] is False, name + " echoed a fixture passphrase")
    require(result["no_file_on_refusal"], name + " created a file after refusal")
    if positive:
        require(result["encrypted_file_created"], name + " did not create an encrypted key")
        require(result["prompts_observed"] == 2, name + " did not observe both prompts")
        require(result["echo_off_at_password_prompts"], name + " did not disable terminal echo")
        require(result["foreground_at_password_prompts"], name + " was not foreground")
    else:
        require(result["exit_refused"], name + " was not refused")


def local_cases(writer, root, env, comment):
    cases = {}
    valid = [
        ("normal", PASSWORD),
        ("minimum", b"a" * 16),
        ("maximum", b"b" * 512),
        ("utf8", ("é" * 8).encode()),
    ]
    for name, password in valid:
        result = terminal_case(writer, root, env, comment, "local-" + name,
                               [("bytes", password), ("bytes", password)])
        verify_case("local-" + name, result, positive=True)
        cases["valid_" + name] = {key: result[key] for key in ("encrypted_file_created", "terminal_restored", "echo_off_at_password_prompts")}

    invalid = [
        ("empty", [("bytes", b""), ("bytes", b"")]),
        ("short", [("bytes", b"short"), ("bytes", b"short")]),
        ("long", [("bytes", b"a" * 513), ("bytes", b"a" * 513)]),
        ("kernel_line_limit", [("bytes", b"z" * 5000), ("bytes", b"z" * 5000)]),
        ("nul", [("bytes", b"a" * 15 + b"\0"), ("bytes", b"a" * 15 + b"\0")]),
        ("invalid_utf8", [("bytes", b"a" * 15 + b"\xff"), ("bytes", b"a" * 15 + b"\xff")]),
        ("mismatch", [("bytes", PASSWORD), ("bytes", WRONG_PASSWORD)]),
        ("eof_empty_first", [("control", b"\x04")]),
        ("eof_partial_first", [("bytes_eof", PASSWORD)]),
        ("eof_empty_confirmation", [("bytes", PASSWORD), ("control", b"\x04")]),
        ("eof_partial_confirmation", [("bytes", PASSWORD), ("bytes_eof", b"a" * 16)]),
        ("interrupt_first", [("control", b"\x03")]),
        ("suspend_first", [("control", b"\x1a")]),
        ("term_first", [("signal", signal.SIGTERM)]),
        ("hangup_first", [("signal", signal.SIGHUP)]),
        ("quit_first", [("signal", signal.SIGQUIT)]),
        ("interrupt_confirmation", [("bytes", PASSWORD), ("control", b"\x03")]),
        ("term_confirmation", [("bytes", PASSWORD), ("signal", signal.SIGTERM)]),
        ("hangup_confirmation", [("bytes", PASSWORD), ("signal", signal.SIGHUP)]),
        ("quit_confirmation", [("bytes", PASSWORD), ("signal", signal.SIGQUIT)]),
        ("suspend_confirmation", [("bytes", PASSWORD), ("control", b"\x1a")]),
    ]
    for name, actions in invalid:
        result = terminal_case(writer, root, env, comment, "local-" + name, actions)
        verify_case("local-" + name, result)
        cases[name] = {key: result[key] for key in ("exit_refused", "no_file_on_refusal", "terminal_restored")}

    for signal_number, label in ((signal.SIGHUP, "hangup"), (signal.SIGTERM, "terminate")):
        ignored = terminal_case(
            writer, root, env, comment, "local-ignored-" + label,
            [("signal", signal_number)], ignored_signals=(signal_number,),
        )
        verify_case("local-ignored-" + label, ignored)
        require(ignored["prompts_observed"] == 1, "ignored signal was not handled through the signal descriptor")
        cases["ignored_" + label + "_signal_delivered"] = {
            key: ignored[key] for key in ("exit_refused", "no_file_on_refusal", "terminal_restored")
        }

    result = terminal_case(writer, root, env, comment, "local-timeout", [], wait_for_timeout=True)
    verify_case("local-timeout", result)
    cases["no_input_timeout"] = {key: result[key] for key in ("exit_refused", "no_file_on_refusal", "terminal_restored")}
    result = terminal_case(writer, root, env, comment, "local-headless", [], no_tty=True)
    require(result["refused_without_file"], "headless --terminal invocation was not refused")
    cases["without_tty"] = result
    return cases


def file_sha256(path):
    import hashlib

    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def process_start_time(pid):
    with open("/proc/" + str(pid) + "/stat", "rb") as source:
        record = source.read(4096)
    fields = record[record.rfind(b")") + 2:].split()
    require(len(fields) > 19, "remote process receipt is malformed")
    return fields[19].decode("ascii")


def remote_foreground_matches(receipt):
    pid = receipt["pid"]
    with open("/proc/" + str(pid) + "/stat", "rb") as source:
        record = source.read(4096)
    fields = record[record.rfind(b")") + 2:].split()
    require(len(fields) > 19, "remote process stat was malformed")
    require(fields[19].decode("ascii") == receipt["start_time"], "remote process identity changed")
    process_group = int(receipt["process_group"])
    return int(fields[2]) == process_group and int(fields[5]) == process_group


def write_state(path, value):
    temporary = path.with_name(path.name + ".new")
    native.private_write(temporary, json.dumps(value, sort_keys=True).encode() + b"\n")
    os.replace(temporary, path)


def remote_wrapper(args):
    tty_fd = None
    original = None
    receipt = {"restoration_observable": False}
    try:
        tty_path = os.ttyname(0)
        tty_fd = os.open(tty_path, os.O_RDWR | os.O_NOCTTY | os.O_CLOEXEC | os.O_NOFOLLOW)
        original = termios.tcgetattr(tty_fd)
        reader_terminal = copy.deepcopy(original)
        reader_terminal[3] |= termios.ECHO | termios.ICANON
        termios.tcsetattr(tty_fd, termios.TCSANOW, reader_terminal)
        observed = termios.tcgetattr(tty_fd)
        receipt = {
            "process_group": os.getpgrp(),
            "tty_path": tty_path,
            "reader_echo_baseline_on": bool(observed[3] & termios.ECHO),
            "reader_canonical_baseline_on": bool(observed[3] & termios.ICANON),
            "restoration_observable": False,
        }
        require(receipt["reader_echo_baseline_on"] and receipt["reader_canonical_baseline_on"],
                "SSH terminal baseline was not echoing and canonical")
        write_state(Path(args.state), receipt)
        child = subprocess.Popen(
            [str(args.writer), "--terminal", str(args.destination), args.comment],
            env=environment(Path(args.state).parent), stdin=subprocess.DEVNULL,
        )
        receipt = {
            **receipt,
            "pid": child.pid,
            "process_group": os.getpgid(child.pid),
            "start_time": process_start_time(child.pid),
        }
        write_state(Path(args.state), receipt)
        try:
            code = child.wait(timeout=REMOTE_DEADLINE)
        except subprocess.TimeoutExpired:
            child.kill()
            code = child.wait(timeout=2)
            receipt["wrapper_timeout"] = True
        try:
            current = termios.tcgetattr(tty_fd)
            receipt["writer_terminal_restored"] = current == reader_terminal
        except OSError:
            receipt["writer_terminal_restored"] = False
        receipt["child_exit_refused"] = code != 0
        return 0 if code == 0 else 1
    except (OSError, ValueError, ProbeError, subprocess.SubprocessError) as error:
        number = error.errno if isinstance(error, OSError) else None
        print(
            f"Remote wrapper failed ({type(error).__name__}, errno={number}).",
            file=sys.stderr,
        )
        return 1
    finally:
        if tty_fd is not None and original is not None:
            try:
                termios.tcsetattr(tty_fd, termios.TCSANOW, original)
                current = termios.tcgetattr(tty_fd)
                receipt["terminal_restored"] = current == original
                receipt["restoration_observable"] = True
            except OSError:
                receipt["terminal_restored"] = None
                receipt["restoration_observable"] = False
            try:
                write_state(Path(args.state), receipt)
            except (OSError, ValueError):
                pass
            os.close(tty_fd)


def remote_tty_state(receipt):
    try:
        fd = os.open(receipt["tty_path"], os.O_RDWR | os.O_NOCTTY | os.O_CLOEXEC | os.O_NOFOLLOW)
        try:
            attributes = termios.tcgetattr(fd)
            foreground = remote_foreground_matches(receipt)
            return not bool(attributes[3] & ECHO_BITS), foreground
        finally:
            os.close(fd)
    except (OSError, ProbeError, ValueError, KeyError):
        return False, False


def complete_remote_receipt(state_path, receipt):
    deadline = time.monotonic() + 2
    while "pid" not in receipt and time.monotonic() < deadline:
        if state_path.exists():
            try:
                receipt = json.loads(state_path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                pass
        if "pid" not in receipt:
            time.sleep(0.01)
    require("pid" in receipt and "start_time" in receipt, "remote writer process receipt was incomplete")
    return receipt


def verify_owned_remote_process(receipt, writer):
    pid = receipt["pid"]
    proc = Path("/proc") / str(pid)
    require(proc.exists(), "remote writer exited before the requested signal")
    require(process_start_time(pid) == receipt["start_time"], "remote writer PID was reused")
    require((proc / "exe").resolve() == writer.resolve(), "remote PID does not name the expected writer")


def remote_process_is_reaped_or_dead(pid):
    proc = Path("/proc") / str(pid)
    if not proc.exists():
        return True
    try:
        with open(proc / "stat", "rb") as source:
            record = source.read(4096)
        fields = record[record.rfind(b")") + 2:].split()
        return not fields or fields[0] in (b"Z", b"X")
    except OSError:
        return True


def wait_remote_process_exit(receipt, writer, deadline):
    pid = receipt["pid"]
    proc = Path("/proc") / str(pid)
    while time.monotonic() < deadline:
        if remote_process_is_reaped_or_dead(pid):
            return True
        try:
            if process_start_time(pid) != receipt["start_time"]:
                return True
        except OSError:
            return True
        require((proc / "exe").resolve() == writer.resolve(), "remote PID identity changed")
        time.sleep(0.025)
    return False


def remote_case(writer, root, env, comment, port, client_key, ssh_binary,
                state_path, destination, actions):
    remote_arguments = [
        "python3", str(Path(__file__).resolve()), "--remote-wrapper",
        "--writer", str(writer), "--state", str(state_path),
        "--destination", str(destination), "--comment", comment,
    ]
    command = shlex.join(["/usr/bin/env", "VIVAC_OPENSSH_DISPOSABLE=1", *remote_arguments])
    args = [
        ssh_binary, "-tt", "-F", "/dev/null", "-p", str(port),
        "-oBatchMode=yes", "-oIdentitiesOnly=yes", "-oIdentityAgent=none",
        "-oForwardAgent=no", "-oClearAllForwardings=yes",
        "-oStrictHostKeyChecking=yes", "-oUserKnownHostsFile=" + str(root / "known_hosts"),
        "-oGlobalKnownHostsFile=/dev/null", "-oControlMaster=no", "-oControlPath=none",
        "-oConnectionAttempts=1", "-oConnectTimeout=3", "-i", str(client_key),
        "root@127.0.0.1", command,
    ]
    master, slave = pty.openpty()
    client_tty = termios.tcgetattr(slave)
    client_tty[3] &= ~ECHO_BITS
    termios.tcsetattr(slave, termios.TCSANOW, client_tty)
    process = None
    output = bytearray()
    prompt_count = 0
    sent_actions = 0
    next_search = 0
    receipt = None
    remote_echo_ok = True
    remote_foreground_ok = True
    deadline = time.monotonic() + CASE_DEADLINE
    started = time.monotonic()
    last_action_time = None
    disconnect = any(action[0] == "disconnect" for action in actions)
    try:
        process = subprocess.Popen(
            args, env=env, stdin=slave, stdout=slave, stderr=slave,
            preexec_fn=lambda: child_setup(slave),
        )
        os.set_blocking(master, False)
        with selectors.DefaultSelector() as selector:
            selector.register(master, selectors.EVENT_READ)
            while process.poll() is None:
                require(time.monotonic() < deadline, "loopback SSH case timed out")
                if state_path.exists():
                    try:
                        receipt = json.loads(state_path.read_text(encoding="utf-8"))
                    except (OSError, ValueError):
                        receipt = None
                for key, _ in selector.select(0.025):
                    try:
                        chunk = os.read(key.fd, 4096)
                    except OSError as error:
                        if error.errno == errno.EIO:
                            continue
                        raise
                    if not chunk:
                        continue
                    output.extend(chunk)
                    require(len(output) <= OUTPUT_LIMIT, "SSH terminal output exceeded its bound")
                    while prompt_count < len(PROMPTS):
                        at = output.find(PROMPTS[prompt_count], next_search)
                        if at < 0:
                            break
                        next_search = at + len(PROMPTS[prompt_count])
                        if receipt is None and state_path.exists():
                            receipt = json.loads(state_path.read_text(encoding="utf-8"))
                        require(receipt is not None, "remote process receipt was absent at password prompt")
                        receipt = complete_remote_receipt(state_path, receipt)
                        require(receipt.get("reader_echo_baseline_on") is True
                                and receipt.get("reader_canonical_baseline_on") is True,
                                "remote reader terminal baseline was not echoing and canonical")
                        echo_off, foreground = remote_tty_state(receipt)
                        remote_echo_ok &= echo_off
                        remote_foreground_ok &= foreground
                        require(echo_off and foreground, "remote TTY was not hidden and foreground at prompt")
                        require(prompt_count < len(actions), "unexpected extra remote password prompt")
                        kind, value = actions[prompt_count]
                        if kind == "bytes":
                            write_all(master, value + b"\n", deadline)
                        elif kind == "bytes_eof":
                            write_all(master, value + b"\x04", deadline)
                        elif kind == "control":
                            write_all(master, value, deadline)
                        elif kind == "signal":
                            receipt = complete_remote_receipt(state_path, receipt)
                            verify_owned_remote_process(receipt, writer)
                            os.kill(receipt["pid"], value)
                        elif kind == "disconnect":
                            process.terminate()
                        else:
                            raise ProbeError("unknown SSH terminal fixture action")
                        sent_actions += 1
                        last_action_time = time.monotonic()
                        prompt_count += 1
                if disconnect and prompt_count:
                    break
        if disconnect and process.poll() is None:
            process.terminate()
        code = process.wait(timeout=3)
        if state_path.exists():
            receipt = json.loads(state_path.read_text(encoding="utf-8"))
        restored = receipt.get("terminal_restored") if receipt is not None else None
        child_refused = receipt.get("child_exit_refused", False) if receipt is not None else False
        exited = False
        if receipt is not None:
            receipt = complete_remote_receipt(state_path, receipt)
            exited = wait_remote_process_exit(receipt, writer, deadline=time.monotonic() + 3)
        if code == 0:
            profile = native.parse_key(destination)
            require(profile["cipher"] == "aes256-gcm@openssh.com" and profile["rounds"] == 64,
                    "SSH-created key has unexpected protection")
        return {
            "ssh_exit_refused": code != 0,
            "encrypted_file_created": code == 0 and destination.exists(),
            "no_file_on_refusal": code == 0 or not destination.exists(),
            "remote_echo_off_at_prompts": remote_echo_ok if prompt_count else None,
            "remote_foreground_at_prompts": remote_foreground_ok if prompt_count else None,
            "remote_terminal_restored": restored,
            "remote_restoration_observable": receipt.get("restoration_observable") if receipt else False,
            "writer_terminal_restored": receipt.get("writer_terminal_restored") if receipt else None,
            "reader_echo_baseline_on": receipt.get("reader_echo_baseline_on") if receipt else None,
            "remote_child_exited": exited,
            "remote_child_refused": child_refused,
            "fixture_echo_detected": PASSWORD in output or WRONG_PASSWORD in output,
            "prompts_observed": prompt_count,
            "actions_sent": sent_actions,
            "exit_seconds": round(time.monotonic() - started, 3),
            "refusal_seconds": round(time.monotonic() - last_action_time, 3) if last_action_time else None,
        }
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            process.wait(timeout=3)
        os.close(master)
        os.close(slave)


def spawn_transport_key(keygen, path, env, comment):
    # These unencrypted credentials authenticate only the isolated loopback
    # SSH fixture; they are synthetic transport keys, never device keys.
    code, _ = native.pty_run(
        [keygen, "-q", "-t", "ed25519", "-f", str(path), "-C", comment],
        env, [b"", b""],
    )
    require(code == 0, "synthetic SSH transport key generation failed")
    require(path.is_file() and path.stat().st_mode & 0o777 == 0o600,
            "synthetic SSH private fixture has unexpected permissions")
    return path.with_name(path.name + ".pub")


def ssh_cases(writer, root, env, comment):
    ssh_binary = shutil.which("ssh", path=os.defpath)
    ssh_keygen = shutil.which("ssh-keygen", path=os.defpath)
    sshd = shutil.which("sshd", path=os.defpath + os.pathsep + "/usr/sbin:/sbin")
    require(ssh_binary and ssh_keygen and sshd, "SSH client or server executable is absent")
    client_private = root / "ssh-transport-client"
    client_public = spawn_transport_key(ssh_keygen, client_private, env, "vivac transport fixture")
    host_private = root / "ssh-transport-host"
    host_public = spawn_transport_key(ssh_keygen, host_private, env, "vivac host fixture")
    auth_dir = root / "authorized"
    auth_dir.mkdir(mode=0o700)
    native.private_write(auth_dir / "keys", client_public.read_bytes())
    host_line = host_public.read_text(encoding="ascii").split()
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    known_hosts_path = root / "known_hosts"
    known_hosts = "[127.0.0.1]:" + str(port) + " " + " ".join(host_line[:2]) + "\n"
    native.private_write(known_hosts_path, known_hosts.encode("ascii"))
    config = root / "sshd_config"
    lines = [
        "Port " + str(port), "ListenAddress 127.0.0.1", "HostKey " + str(host_private),
        "PidFile " + str(root / "sshd.pid"), "AuthorizedKeysFile " + str(auth_dir / "keys"),
        "StrictModes yes", "PasswordAuthentication no", "KbdInteractiveAuthentication no",
        "PermitRootLogin prohibit-password", "PubkeyAuthentication yes", "UsePAM no",
        "AllowAgentForwarding no", "AllowTcpForwarding no", "X11Forwarding no",
        "PermitTunnel no", "PermitTTY yes", "MaxAuthTries 2", "LogLevel ERROR",
    ]
    native.private_write(config, ("\n".join(lines) + "\n").encode("ascii"))
    server = subprocess.Popen(
        [sshd, "-D", "-e", "-f", str(config)], env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    try:
        ready_by = time.monotonic() + 5
        while True:
            require(server.poll() is None, "loopback SSH server exited before readiness")
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                    break
            except OSError:
                require(time.monotonic() < ready_by, "loopback SSH server readiness timed out")
                time.sleep(0.025)
        cases = {}
        scenarios = [
            ("ssh_valid", [("bytes", PASSWORD), ("bytes", PASSWORD)], True),
            ("ssh_partial_eof", [("bytes_eof", PASSWORD)], False),
            ("ssh_term", [("signal", signal.SIGTERM)], False),
            ("ssh_disconnect", [("disconnect", None)], False),
        ]
        for name, actions, positive in scenarios:
            state = root / (name + ".json")
            destination = root / (name + "-key")
            try:
                result = remote_case(
                    writer, root, env, comment, port, client_private, ssh_binary,
                    state, destination, actions,
                )
                require(result["fixture_echo_detected"] is False, name + " echoed a fixture passphrase")
                if positive:
                    require(result["encrypted_file_created"], name + " did not create an encrypted key")
                    require(result["remote_terminal_restored"] is True, name + " did not restore remote terminal")
                    require(result["writer_terminal_restored"] is True,
                            name + " reader did not restore its echoing terminal baseline")
                    require(result["reader_echo_baseline_on"] is True,
                            name + " did not begin with terminal echo enabled")
                    require(result["remote_child_exited"], name + " remote child did not exit")
                else:
                    require(result["ssh_exit_refused"] and result["no_file_on_refusal"], name + " was not refused")
                    require(result["remote_child_exited"], name + " remote child did not exit")
                    if name != "ssh_disconnect":
                        require(result["refusal_seconds"] is not None and result["refusal_seconds"] < 5,
                                name + " remote reader exceeded the refusal deadline")
                        require(result["remote_terminal_restored"] is True, name + " did not restore remote terminal")
                        require(result["writer_terminal_restored"] is True,
                                name + " reader did not restore its echoing terminal baseline")
            except (ProbeError, native.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
                number = error.errno if isinstance(error, OSError) else None
                detail = str(error) if isinstance(error, (ProbeError, native.ProbeError)) else f"{type(error).__name__}, errno={number}"
                raise ProbeError(f"SSH case {name} failed: {detail}") from None
            cases[name] = {key: result[key] for key in (
                "ssh_exit_refused", "encrypted_file_created", "no_file_on_refusal",
                "remote_echo_off_at_prompts", "remote_foreground_at_prompts",
                "remote_terminal_restored", "remote_child_exited", "fixture_echo_detected",
                "remote_restoration_observable", "writer_terminal_restored", "reader_echo_baseline_on",
            )}
        state = root / "ssh-reconnect.json"
        destination = root / "ssh-reconnect-key"
        try:
            result = remote_case(
                writer, root, env, comment, port, client_private, ssh_binary,
                state, destination, [("bytes", PASSWORD), ("bytes", PASSWORD)],
            )
            require(result["encrypted_file_created"] and result["remote_terminal_restored"] is True,
                    "fresh SSH session did not recover after the prior session")
            require(result["writer_terminal_restored"] is True and result["reader_echo_baseline_on"] is True,
                    "fresh SSH reader did not restore its echoing terminal baseline")
        except (ProbeError, native.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
            number = error.errno if isinstance(error, OSError) else None
            detail = str(error) if isinstance(error, (ProbeError, native.ProbeError)) else f"{type(error).__name__}, errno={number}"
            raise ProbeError(f"SSH case fresh-session failed: {detail}") from None
        cases["fresh_session_after_disconnect"] = True
        return {
            "cases": cases,
            "passed": True,
            "ssh_sha256": file_sha256(ssh_binary),
            "ssh_keygen_sha256": file_sha256(ssh_keygen),
            "sshd_sha256": file_sha256(sshd),
            "writer_sha256": file_sha256(writer),
            "script_sha256": file_sha256(Path(__file__).resolve()),
            "server_loopback_only": True,
            "host_key_pinned": True,
            "agent_forwarding_disabled": True,
            "external_network_disabled_by_container": True,
        }
    finally:
        server.terminate()
        try:
            server.wait(timeout=2)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=2)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--writer", required=True, type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--remote-wrapper", action="store_true")
    parser.add_argument("--state", type=Path)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--comment")
    args = parser.parse_args()
    report_path = None
    phase = "preflight"
    try:
        require(sys.platform == "linux", "terminal probe requires Linux")
        require(os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable declaration is absent")
        require({name for _, name in socket.if_nameindex()} == {"lo"},
                "terminal probe requires a loopback-only network namespace")
        writer = args.writer.resolve(strict=True)
        require(writer.is_file() and os.access(writer, os.X_OK), "writer is unavailable")
        if args.remote_wrapper:
            require(args.state is not None and args.destination is not None and args.comment is not None,
                    "remote wrapper arguments are incomplete")
            return remote_wrapper(args)
        require(args.report is not None and args.report.is_absolute(), "report destination must be absolute")
        report_path = args.report.parent.resolve(strict=True) / args.report.name
        require(not report_path.exists() and not report_path.is_symlink(), "report must be new")
        with tempfile.TemporaryDirectory(prefix="vivac-terminal-probe-", dir="/tmp") as folder:
            root = Path(folder).resolve()
            root.chmod(0o700)
            (root / "home").mkdir(mode=0o700)
            env = environment(root)
            device_id, credential_id = native.identifier(), native.identifier()
            comment = "vivac.device-key.v1:" + device_id + ":" + credential_id
            phase = "local PTY matrix"
            cases = local_cases(writer, root, env, comment)
            phase = "loopback SSH matrix"
            ssh = ssh_cases(writer, root, env, comment)
            ssh_binary = shutil.which("ssh", path=os.defpath)
            code, version = native.bounded_run([ssh_binary, "-V"], env)
            require(
                code == 0 and 8 <= len(version) <= 256
                and version.startswith(b"OpenSSH_")
                and all(32 <= byte <= 126 for byte in version.strip()),
                "OpenSSH version receipt was refused",
            )
            report = {
                "version": 1,
                "openssh_version": version.decode("ascii", errors="strict"),
                "ssh_sha256": file_sha256(ssh_binary),
                "python_sha256": file_sha256(sys.executable),
                "openssh_probe_sha256": file_sha256(Path(native.__file__).resolve()),
                "local_terminal_cases": cases,
                "ssh_cases": ssh,
                "terminal_cases_passed": True,
                "ssh_cases_passed": ssh["passed"],
                "custody_accredited": False,
            }
            phase = "metadata report"
            native.private_write(report_path, json.dumps(report, indent=2, sort_keys=True).encode() + b"\n")
        print("Terminal observations: local=true ssh=true custody=false", flush=True)
        return 0
    except (ProbeError, native.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        number = error.errno if isinstance(error, OSError) else None
        if isinstance(error, (ProbeError, native.ProbeError)):
            message = f"{phase} failed: {error}"
        else:
            message = f"{phase} failed ({type(error).__name__}, errno={number})"
        print("Terminal probe failed: " + message + "; no result is accredited.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
