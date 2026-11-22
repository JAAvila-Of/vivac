"""Observe disposable attended unlock input without retaining its contents."""
import argparse
import copy
import errno
import hashlib
import json
import os
from pathlib import Path
import pty
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import termios
import time

import lifecycle_probe as life
import terminal_probe as terminal


PROMPT = b"Device credential unlock secret: "
SECRET = b"0123456789abcdef" * 4
REFUSAL = b"Experimental OpenSSH writer refused or failed."
SUCCESS = dict(version=1, attended_input_accepted=True, terminal_restored=True,
               credential_unlocked=False, session_activated=False)
POSITIVE = {"valid", "valid-stdin-decoy"}
HEADLESS = {"no-tty", "unset-disposable"}
CASES = ("valid", "valid-stdin-decoy", "uppercase", "short", "long", "nonhex", "nul", "cr",
         "invalid-utf8", "eof", "no-tty", "term", "ignored-term", "background", "terminal-drift",
         "timeout", "extra-argument", "terminal-hangup", "unset-disposable")
FIELDS = {"accepted", "exit_code", "prompts_observed", "terminal_restored", "echo_disabled",
          "foreground_owned", "input_echo_detected", "process_exit_observed", "pidfd_exit_observed",
          "helper_unchanged", "state_unchanged", "elapsed_ms", "credential_unlocked", "session_activated"}
ECHO_BITS = sum(getattr(termios, name, 0) for name in
                ("ECHO", "ECHONL", "ECHOE", "ECHOK", "ECHOKE", "ECHOCTL", "ECHOPRT"))
OUTPUT_LIMIT = 4096
CASE_SECONDS = 22
BACKGROUND = r'''
import os, sys
writer, announce, gate = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
child = os.fork()
if child == 0:
    os.close(announce)
    os.setpgid(0, 0)
    if os.read(gate, 1) != b"G":
        os._exit(97)
    os.close(gate)
    os.execve(writer, [writer, "--attended-unlock-input-probe"], os.environ)
os.setpgid(child, child)
os.close(gate)
os.write(announce, str(child).encode("ascii") + b"\n")
os.close(announce)
_, status = os.waitpid(child, 0)
code = os.waitstatus_to_exitcode(status)
os._exit(code if code >= 0 else 128 - code)
'''


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def validate_success(payload):
    def unique(items):
        value = {}
        for name, item in items:
            require(name not in value, "unlock input success contains duplicate fields")
            value[name] = item
        return value
    value = json.loads(payload, object_pairs_hook=unique)
    require(type(value) is dict and set(value) == set(SUCCESS)
            and type(value["version"]) is int and value["version"] == 1
            and all(type(value[field]) is bool for field in SUCCESS if field != "version")
            and value == SUCCESS, "unlock input success metadata differs")
    return value


def validate_case(name, value):
    require(name in CASES and type(value) is dict and set(value) == FIELDS,
            "unlock input case fields differ")
    require(all(type(value[field]) is bool for field in
                ("accepted", "input_echo_detected", "process_exit_observed", "pidfd_exit_observed",
                 "helper_unchanged", "state_unchanged", "credential_unlocked", "session_activated")),
            "unlock input boolean evidence differs")
    require(value["accepted"] is (name in POSITIVE) and type(value["exit_code"]) is int
            and value["exit_code"] == (0 if name in POSITIVE else 1), "unlock input outcome differs")
    require(not value["input_echo_detected"] and not value["credential_unlocked"]
            and not value["session_activated"], "unlock input disclosed content or claimed activation")
    require(all(value[field] is True for field in
                ("process_exit_observed", "pidfd_exit_observed", "helper_unchanged", "state_unchanged")),
            "unlock input retained resources or changed state")
    expected_prompts = 0 if name in HEADLESS | {"background", "extra-argument"} else 1
    require(type(value["prompts_observed"]) is int and value["prompts_observed"] == expected_prompts,
            "unlock input prompt count differs")
    expected_terminal = None if name in HEADLESS | {"terminal-hangup"} else True
    require(value["terminal_restored"] is expected_terminal, "unlock input terminal restoration differs")
    expected_echo = True if expected_prompts else None
    expected_foreground = False if name == "background" else expected_echo
    require(value["echo_disabled"] is expected_echo and value["foreground_owned"] is expected_foreground,
            "unlock input foreground or echo boundary differs")
    require(type(value["elapsed_ms"]) is int and 0 <= value["elapsed_ms"] <= CASE_SECONDS * 1000,
            "unlock input duration exceeded its bound")
    if name == "timeout":
        require(14000 <= value["elapsed_ms"] <= 21000, "unlock input deadline was not observed")
    else:
        require(value["elapsed_ms"] <= 5000, "unlock input response fell back to its terminal deadline")
    return value


def fingerprint(path):
    require(path.is_absolute() and not path.is_symlink() and path.is_file(), "unlock input helper shape differs")
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= 64 * 1024 * 1024,
                "unlock input helper exceeded its bound")
        while True:
            block = stream.read(65536)
            if not block:
                break
            digest.update(block)
        after = os.fstat(stream.fileno())
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns),
                "unlock input helper changed while inspected")
    return digest.hexdigest()


def snapshot(root):
    require(root.is_dir() and not root.is_symlink(), "unlock input temporary root differs")
    entries = sorted(root.rglob("*"))
    require(len(entries) <= 16, "unlock input temporary state exceeded its bound")
    result = []
    for entry in entries:
        value = entry.lstat()
        require(not stat.S_ISLNK(value.st_mode), "unlock input created a link")
        result.append((entry.relative_to(root).as_posix(), value.st_mode, value.st_size,
                       value.st_ino, value.st_mtime_ns, value.st_ctime_ns))
    return result


def pidfd_ready(descriptor, deadline):
    remaining = deadline - time.monotonic()
    require(remaining > 0, "unlock input exit deadline expired")
    with selectors.DefaultSelector() as selector:
        selector.register(descriptor, selectors.EVENT_READ)
        return bool(selector.select(remaining))


def read_background_pid(descriptor, deadline):
    raw = bytearray()
    os.set_blocking(descriptor, False)
    with selectors.DefaultSelector() as selector:
        selector.register(descriptor, selectors.EVENT_READ)
        while b"\n" not in raw:
            require(time.monotonic() < deadline, "background identity deadline expired")
            if not selector.select(min(0.05, deadline - time.monotonic())):
                continue
            part = os.read(descriptor, 32)
            require(part and len(raw) + len(part) <= 24, "background identity boundary differs")
            raw.extend(part)
    require(raw.endswith(b"\n") and raw[:-1].isdigit(), "background identity was malformed")
    identity = int(raw[:-1])
    require(identity > 1, "background identity differs")
    return identity


def payload(name):
    return {"valid": SECRET, "valid-stdin-decoy": SECRET, "uppercase": SECRET.upper(),
            "short": SECRET[:-1], "long": SECRET + b"a", "nonhex": b"g" + SECRET[1:],
            "nul": SECRET[:32] + b"\0" + SECRET[33:], "cr": SECRET[:32] + b"\r" + SECRET[33:],
            "invalid-utf8": SECRET[:32] + b"\xff" + SECRET[33:]}.get(name)


def observe_case(writer, root, name, helper_hash):
    require(name in CASES, "unknown unlock input case")
    environment = life.private_env(root)
    if name == "unset-disposable":
        environment.pop("VIVAC_OPENSSH_DISPOSABLE")
    if name == "no-tty":
        environment["VIVAC_UNLOCK_SECRET"] = SECRET.decode("ascii")
    before = snapshot(root)
    args = [str(writer), "--attended-unlock-input-probe"]
    if name == "extra-argument":
        args.append("--unexpected")
    master = slave = process = child_pidfd = wrapper_pidfd = None
    announce_read = announce_write = gate_read = gate_write = None
    transcript = bytearray()
    prompts = 0
    echo_disabled = foreground_owned = None
    started = time.monotonic()
    deadline = started + CASE_SECONDS
    sent = False
    actual_pid = None
    try:
        if name in HEADLESS:
            process = subprocess.Popen(args, env=environment, stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
            child_pidfd = os.pidfd_open(process.pid)
            out, _ = process.communicate(SECRET + b"\n", timeout=4)
            require(len(out) <= OUTPUT_LIMIT, "headless unlock input output exceeded its bound")
            transcript.extend(out)
            restored = None
        else:
            master, slave = pty.openpty()
            original = copy.deepcopy(termios.tcgetattr(slave))
            if name == "background":
                announce_read, announce_write = os.pipe2(os.O_CLOEXEC)
                gate_read, gate_write = os.pipe2(os.O_CLOEXEC)
                process = subprocess.Popen([sys.executable, "-B", "-c", BACKGROUND, str(writer),
                                            str(announce_write), str(gate_read)],
                                           env=environment, stdin=slave, stdout=slave, stderr=slave,
                                           pass_fds=(announce_write, gate_read),
                                           preexec_fn=lambda: terminal.child_setup(slave))
                os.close(announce_write); announce_write = None
                os.close(gate_read); gate_read = None
                actual_pid = read_background_pid(announce_read, deadline)
                child_pidfd = os.pidfd_open(actual_pid)
                wrapper_pidfd = os.pidfd_open(process.pid)
                foreground_owned = os.tcgetpgrp(master) == os.getpgid(actual_pid)
                require(foreground_owned is False, "background process remained foreground")
                os.write(gate_write, b"G")
                os.close(gate_write); gate_write = None
            else:
                ignored = (signal.SIGTERM,) if name == "ignored-term" else ()
                process = subprocess.Popen(args, env=environment,
                                           stdin=subprocess.PIPE if name == "valid-stdin-decoy" else slave,
                                           stdout=slave, stderr=slave,
                                           preexec_fn=lambda: terminal.child_setup(slave, ignored))
                child_pidfd = os.pidfd_open(process.pid)
                actual_pid = process.pid
                if name == "valid-stdin-decoy":
                    process.stdin.write(b"not-the-unlock-input\n")
                    process.stdin.close()
            os.set_blocking(master, False)
            with selectors.DefaultSelector() as selector:
                selector.register(master, selectors.EVENT_READ)
                while process.poll() is None:
                    require(time.monotonic() < deadline, "unlock input PTY deadline expired")
                    for key, _ in selector.select(0.025):
                        try:
                            block = os.read(key.fd, 4096)
                        except OSError as error:
                            if error.errno == errno.EIO:
                                continue
                            raise
                        if block:
                            transcript.extend(block)
                            require(len(transcript) <= OUTPUT_LIMIT, "unlock input output exceeded its bound")
                    if not sent and PROMPT in transcript:
                        prompts = transcript.count(PROMPT)
                        require(prompts == 1, "unlock input prompted more than once")
                        hidden = termios.tcgetattr(slave)
                        echo_disabled = not bool(hidden[3] & ECHO_BITS)
                        foreground_owned = os.tcgetpgrp(master) == os.getpgid(actual_pid)
                        require(echo_disabled and foreground_owned, "unlock input prompted outside its terminal boundary")
                        sent = True
                        value = payload(name)
                        if value is not None:
                            terminal.write_all(master, value + b"\n", deadline)
                        elif name == "eof":
                            terminal.write_all(master, b"\x04", deadline)
                        elif name in ("term", "ignored-term"):
                            signal.pidfd_send_signal(child_pidfd, signal.SIGTERM)
                        elif name == "terminal-drift":
                            changed = copy.deepcopy(hidden)
                            changed[0] ^= termios.IXON
                            termios.tcsetattr(slave, termios.TCSANOW, changed)
                            require(termios.tcgetattr(slave) != hidden, "terminal drift was not observed")
                            terminal.write_all(master, SECRET + b"\n", deadline)
                        elif name == "terminal-hangup":
                            selector.unregister(master)
                            os.close(master); master = None
                        elif name != "timeout":
                            raise ProbeError("unexpected unlock input prompt")
                process.wait(timeout=1)
                if master is not None:
                    while True:
                        try:
                            block = os.read(master, 4096)
                        except BlockingIOError:
                            break
                        except OSError as error:
                            if error.errno == errno.EIO:
                                break
                            raise
                        if not block:
                            break
                        transcript.extend(block)
                        require(len(transcript) <= OUTPUT_LIMIT, "unlock input output exceeded its bound")
            prompts = transcript.count(PROMPT)
            restored = None if name == "terminal-hangup" else termios.tcgetattr(slave) == original
        require(pidfd_ready(child_pidfd, deadline), "unlock input child exit was not observed")
        if wrapper_pidfd is not None:
            require(pidfd_ready(wrapper_pidfd, deadline), "background wrapper exit was not observed")
        expected_prompt = 0 if name in HEADLESS | {"background", "extra-argument"} else 1
        require(prompts == expected_prompt, "unlock input prompt boundary differs")
        standard = bytes(transcript).replace(b"\r\n", b"\n")
        if prompts:
            require(standard.startswith(PROMPT), "unlock input emitted unexpected leading output")
            standard = standard[len(PROMPT):]
        if name in POSITIVE:
            validate_success(standard)
            require(process.returncode == 0, "unlock input success exit differs")
        elif name != "terminal-hangup":
            require(standard == REFUSAL + b"\n", "unlock input rejection diagnostics differ")
        value = payload(name)
        echoed = SECRET in transcript or (value is not None and value in transcript)
        result = dict(accepted=process.returncode == 0, exit_code=process.returncode,
                      prompts_observed=prompts, terminal_restored=restored,
                      echo_disabled=echo_disabled, foreground_owned=foreground_owned,
                      input_echo_detected=echoed, process_exit_observed=True, pidfd_exit_observed=True,
                      helper_unchanged=fingerprint(writer) == helper_hash, state_unchanged=snapshot(root) == before,
                      elapsed_ms=round((time.monotonic() - started) * 1000),
                      credential_unlocked=False, session_activated=False)
        validate_case(name, result)
        return result
    finally:
        if process is not None and process.poll() is None:
            if actual_pid is not None and actual_pid != process.pid:
                try:
                    signal.pidfd_send_signal(child_pidfd, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.kill()
            process.wait(timeout=2)
        for descriptor in (master, slave, child_pidfd, wrapper_pidfd, announce_read, announce_write, gate_read, gate_write):
            if descriptor is not None:
                os.close(descriptor)
        if process is not None:
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream is not None and not stream.closed:
                    stream.close()


def validate_report(value):
    require(type(value) is dict and set(value) == {"version", "status", "cases", "helper_sha256",
                                                 "source_sha256", "resource_limits", "uid",
                                                 "credential_unlocked", "session_activated"},
            "unlock input report fields differ")
    require(type(value["version"]) is int and value["version"] == 1 and value["status"] == "observed"
            and type(value["uid"]) is int and value["uid"] == 65534,
            "unlock input report identity differs")
    require(value["credential_unlocked"] is False and value["session_activated"] is False,
            "unlock input report claims activation")
    for field in ("helper_sha256", "source_sha256"):
        require(type(value[field]) is str and len(value[field]) == 64
                and all(character in "0123456789abcdef" for character in value[field]),
                "unlock input report hash differs")
    limits = value["resource_limits"]
    require(type(limits) is dict and set(limits) == {"memory_bytes", "swap_bytes", "pids", "cpu_quota",
                                                 "cpu_period", "temporary_bytes"}
            and all(type(item) is int for item in limits.values())
            and 0 < limits["memory_bytes"] <= 512 * 1024 * 1024 and limits["swap_bytes"] == 0
            and 0 < limits["pids"] <= 128 and 0 < limits["cpu_quota"] <= limits["cpu_period"]
            and 0 < limits["temporary_bytes"] <= 128 * 1024 * 1024,
            "unlock input report limits differ")
    require(type(value["cases"]) is dict and set(value["cases"]) == set(CASES),
            "unlock input report cases differ")
    for name, result in value["cases"].items():
        validate_case(name, result)
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--writer", type=Path, required=True)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1"
                and os.getuid() == os.geteuid() == os.getgid() == os.getegid() == 65534,
                "unlock input experiment requires disposable credentials")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "unlock input experiment requires no-new-privileges")
        limits = life.require_resources(expected_script="unlock_input_probe.py")
        helper_hash, source_hash = fingerprint(args.writer), fingerprint(Path(__file__).resolve())
        with tempfile.TemporaryDirectory(prefix="unlock-input-", dir="/tmp") as temporary:
            root = Path(temporary)
            cases = {name: observe_case(args.writer, root, name, helper_hash) for name in CASES}
        report = dict(version=1, status="observed", cases=cases, helper_sha256=helper_hash,
                      source_sha256=source_hash, resource_limits=limits, uid=65534,
                      credential_unlocked=False, session_activated=False)
        require(fingerprint(args.writer) == helper_hash and fingerprint(Path(__file__).resolve()) == source_hash,
                "unlock input helper or observer changed")
        validate_report(report)
        print(json.dumps(report, sort_keys=True))
        return 0
    except (ProbeError, life.ProbeError, terminal.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        frames = []
        frame = error.__traceback__
        while frame is not None:
            frames.append({"file": Path(frame.tb_frame.f_code.co_filename).name, "line": frame.tb_lineno})
            frame = frame.tb_next
        print(json.dumps({"status": "incomplete", "failure": {"kind": type(error).__name__, "frames": frames[-8:]}}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
