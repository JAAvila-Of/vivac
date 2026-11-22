#!/usr/bin/env python3
"""Exercise synthetic one-use opening permits in a disposable Linux store."""
import argparse
import ctypes
import errno
import json
import os
from pathlib import Path
import select
import signal
import stat
import subprocess
import sys
import time
import unittest

PERMIT = "open-permit.json"
INSTALLATION = "00000000000000000000000001"
CREDENTIAL = "00000000000000000000000002"
GENERATION = "a" * 64
STATES = ("first-open", "synthetic-clean-close")
DEADLINE_SECONDS = 5
MAX_PACKET = 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def canonical_permit(state):
    return json.dumps({
        "version": 1,
        "installation_id": INSTALLATION,
        "credential_id": CREDENTIAL,
        "generation": GENERATION,
        "state": state,
    }, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"


def parse_permit(payload):
    require(type(payload) is bytes and 0 < len(payload) <= MAX_PACKET,
            "permit size differs")
    value = json.loads(payload)
    require(type(value) is dict
            and set(value) == {"version", "installation_id", "credential_id", "generation", "state"}
            and type(value["version"]) is int and value["version"] == 1
            and value["installation_id"] == INSTALLATION
            and value["credential_id"] == CREDENTIAL
            and value["generation"] == GENERATION
            and value["state"] in STATES
            and canonical_permit(value["state"]) == payload, "permit binding or encoding differs")
    return value


def check_directory(path):
    require(path.is_absolute() and os.getuid() == os.geteuid() != 0
            and os.getgid() == os.getegid(), "unprivileged absolute fixture required")
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        metadata = os.fstat(descriptor)
        require(metadata.st_uid == os.geteuid() and metadata.st_gid == os.getegid()
                and stat.S_IMODE(metadata.st_mode) == 0o700
                and not os.listxattr(descriptor), "fixture directory policy differs")
        filesystem = (ctypes.c_long * 32)()
        libc = ctypes.CDLL(None, use_errno=True)
        require(libc.fstatfs(descriptor, ctypes.byref(filesystem)) == 0
                and filesystem[0] == 0xEF53, "fixture requires an ext-family filesystem")
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def identity(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_mode,
            metadata.st_uid, metadata.st_gid, metadata.st_nlink, metadata.st_size)


def read_permit(directory):
    descriptor = os.open(PERMIT, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC,
                         dir_fd=directory)
    try:
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode)
                and stat.S_IMODE(metadata.st_mode) == 0o600
                and metadata.st_uid == os.geteuid() and metadata.st_gid == os.getegid()
                and metadata.st_nlink == 1 and 0 < metadata.st_size <= MAX_PACKET
                and not os.listxattr(descriptor), "permit file policy differs")
        payload = os.read(descriptor, MAX_PACKET + 1)
        value = parse_permit(payload)
        require(identity(os.fstat(descriptor)) == identity(metadata)
                and identity(os.stat(PERMIT, dir_fd=directory, follow_symlinks=False))
                == identity(metadata), "permit object changed")
        return value
    finally:
        os.close(descriptor)


def emit(value):
    packet = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    require(len(packet) <= MAX_PACKET, "role output exceeds bound")
    sys.stdout.buffer.write(packet)
    sys.stdout.buffer.flush()


def wait_release():
    require(bool(select.select([sys.stdin.buffer], [], [], DEADLINE_SECONDS)[0])
            and os.read(sys.stdin.fileno(), 1) == b"G", "fixture release timed out or differs")


def outcome(name, locked, prompt=False, **extra):
    return {"outcome": name, "lock_acquired": locked,
            "synthetic_prompt_boundary": prompt, **extra}


def attempt(path, cut):
    import fcntl

    directory = check_directory(path)
    locked = False
    try:
        try:
            fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
            locked = True
        except BlockingIOError:
            emit(outcome("refused", False, reason="contended"))
            return
        try:
            permit = read_permit(directory)
        except FileNotFoundError:
            emit(outcome("refused", True, reason="missing-permit"))
            return
        except (OSError, ValueError):
            emit(outcome("refused", True, reason="invalid-permit"))
            return
        os.unlink(PERMIT, dir_fd=directory)
        if cut == "before-directory-sync":
            emit(outcome("unlinked-before-directory-sync", True))
            wait_release()
            raise ValueError("early cut must be terminated by the observer")
        try:
            if cut == "directory-sync-error":
                raise OSError(errno.EIO, "synthetic directory sync failure")
            os.fsync(directory)
        except OSError:
            emit(outcome("refused", True, reason="directory-sync-error"))
            return
        emit(outcome("synthetic-prompt-boundary", True, True, permit_state=permit["state"]))
        if cut == "hold":
            wait_release()
    finally:
        if locked:
            fcntl.flock(directory, fcntl.LOCK_UN)
        os.close(directory)


def publish(path, state):
    import fcntl

    directory = check_directory(path)
    try:
        fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
        descriptor = os.open(PERMIT, os.O_WRONLY | os.O_CREAT | os.O_EXCL
                             | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=directory)
        try:
            payload = canonical_permit(state)
            require(os.write(descriptor, payload) == len(payload), "permit write was short")
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.fsync(directory)
        require(read_permit(directory)["state"] == state, "published fixture permit differs")
        emit({"outcome": "synthetic-permit-published", "permit_state": state})
    finally:
        os.close(directory)


def role_process(path, role, option):
    return subprocess.Popen(
        [sys.executable, "-I", "-S", "-B", str(Path(__file__).resolve()),
         "--role", role, "--option", option, str(path)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        close_fds=True, cwd="/", env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})


def read_packet(process):
    deadline = time.monotonic() + DEADLINE_SECONDS
    packet = bytearray()
    while not packet.endswith(b"\n"):
        remaining = deadline - time.monotonic()
        require(remaining > 0 and bool(select.select([process.stdout], [], [], remaining)[0]),
                "role packet timed out")
        item = os.read(process.stdout.fileno(), 1)
        require(bool(item), "role ended before its packet")
        packet.extend(item)
        require(len(packet) <= MAX_PACKET, "role packet exceeds bound")
    value = json.loads(packet)
    require(type(value) is dict, "role packet is not an object")
    return value


def finish(process, expected, *, release=False):
    if release:
        process.stdin.write(b"G")
        process.stdin.flush()
    output, errors = process.communicate(timeout=DEADLINE_SECONDS)
    require(process.returncode == expected and not output and not errors,
            "role exit or trailing output differs")


def run_role(path, role, option, expected):
    process = role_process(path, role, option)
    try:
        packet = read_packet(process)
        require(packet == expected, "role result differs")
        finish(process, 0)
        return {"exit_code": process.returncode, "packet": packet}
    finally:
        dispose(process)


def dispose(process):
    if process.poll() is None:
        process.kill()
        process.wait(timeout=DEADLINE_SECONDS)
    for stream in (process.stdin, process.stdout, process.stderr):
        stream.close()


def publish_fixture(path, state):
    return run_role(path, "publish", state,
                    {"outcome": "synthetic-permit-published", "permit_state": state})


def denied(path, reason):
    return run_role(path, "attempt", "normal", outcome("refused", True, reason=reason))


def synthetic_prompt(path, state):
    return run_role(path, "attempt", "normal",
                    outcome("synthetic-prompt-boundary", True, True, permit_state=state))


def terminate_owned(process):
    reference = os.pidfd_open(process.pid)
    try:
        poller = select.poll()
        poller.register(reference, select.POLLIN)
        require(process.poll() is None and not poller.poll(0), "cut process already exited")
        signal.pidfd_send_signal(reference, signal.SIGKILL)
        require(bool(poller.poll(DEADLINE_SECONDS * 1000)), "cut process exit was not observed")
        finish(process, -signal.SIGKILL)
    finally:
        os.close(reference)


def new_store(root, name):
    path = root / name
    path.mkdir(mode=0o700)
    return path


def write_fixture(path, payload):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
    try:
        require(os.write(descriptor, payload) == len(payload), "fixture write was short")
    finally:
        os.close(descriptor)


def run_experiment(root):
    directory = check_directory(root)
    try:
        require(not os.listdir(directory), "scratch directory must be empty")
    finally:
        os.close(directory)
    results = []
    path = new_store(root, "initial")
    published = publish_fixture(path, "first-open")
    opened = synthetic_prompt(path, "first-open")
    refused = denied(path, "missing-permit")
    results.append({"case": "initial-single-use", "attempts": [published, opened, refused]})

    path = new_store(root, "clean-close")
    publish_fixture(path, "first-open")
    synthetic_prompt(path, "first-open")
    published = publish_fixture(path, "synthetic-clean-close")
    reopened = synthetic_prompt(path, "synthetic-clean-close")
    refused = denied(path, "missing-permit")
    results.append({"case": "synthetic-clean-close-single-use",
                    "attempts": [published, reopened, refused]})

    path = new_store(root, "missing")
    results.append({"case": "missing", "attempts": [denied(path, "missing-permit")]})
    mutations = {
        "malformed": b"{",
        "wrong-binding": canonical_permit("first-open").replace(
            INSTALLATION.encode(), b"00000000000000000000000003"),
        "unknown-format": canonical_permit("first-open").replace(b'"version":1', b'"version":2'),
        "wrong-mode": canonical_permit("first-open"),
    }
    for name, payload in mutations.items():
        path = new_store(root, name)
        write_fixture(path / PERMIT, payload)
        if name == "wrong-mode":
            (path / PERMIT).chmod(0o644)
        results.append({"case": name, "attempts": [denied(path, "invalid-permit")]})
        require((path / PERMIT).read_bytes() == payload, "refusal changed fixture contents")

    for name in ("symlink", "hardlink"):
        path = new_store(root, name)
        target = root / (name + "-target")
        write_fixture(target, canonical_permit("first-open"))
        if name == "symlink":
            (path / PERMIT).symlink_to(target)
        else:
            os.link(target, path / PERMIT)
        results.append({"case": name, "attempts": [denied(path, "invalid-permit")]})
        require(target.read_bytes() == canonical_permit("first-open"),
                "refusal modified the linked target")

    path = new_store(root, "sync-error")
    publish_fixture(path, "first-open")
    refusal = run_role(path, "attempt", "directory-sync-error",
                       outcome("refused", True, reason="directory-sync-error"))
    results.append({"case": "directory-sync-error",
                    "attempts": [refusal, denied(path, "missing-permit")]})

    path = new_store(root, "concurrent")
    publish_fixture(path, "first-open")
    process = role_process(path, "attempt", "hold")
    try:
        packet = read_packet(process)
        require(packet == outcome("synthetic-prompt-boundary", True, True,
                                  permit_state="first-open"), "held boundary differs")
        contender = run_role(path, "attempt", "normal",
                             outcome("refused", False, reason="contended"))
        finish(process, 0, release=True)
        results.append({"case": "concurrent",
                        "attempts": [{"exit_code": 0, "packet": packet}, contender,
                                     denied(path, "missing-permit")]})
    finally:
        dispose(process)

    for cut, name in (("hold", "death-after-directory-sync"),
                      ("before-directory-sync", "death-before-directory-sync")):
        path = new_store(root, name)
        publish_fixture(path, "first-open")
        process = role_process(path, "attempt", cut)
        try:
            packet = read_packet(process)
            expected = (outcome("synthetic-prompt-boundary", True, True, permit_state="first-open")
                        if cut == "hold" else outcome("unlinked-before-directory-sync", True))
            require(packet == expected and not (path / PERMIT).exists(), "death cut differs")
            terminate_owned(process)
            results.append({"case": name,
                            "attempts": [{"exit_code": -signal.SIGKILL, "packet": packet},
                                         denied(path, "missing-permit")],
                            "owned_pidfd_exit_observed": True})
        finally:
            dispose(process)
    value = {"version": 1, "experiment": "synthetic-cold-open-permit",
             "status": "observed", "filesystem": "ext-family", "cases": results,
             "synthetic_fixtures_only": True, "secret_entry_performed": False,
             "closure_authority": False, "operational_reopening": False,
             "physical_power_loss_simulated": False}
    require(len(json.dumps(value)) <= 16384, "experiment report exceeds bound")
    return value


class PermitCodecTests(unittest.TestCase):
    def test_only_bound_canonical_fixture_permits_are_accepted(self):
        for state in ("first-open", "synthetic-clean-close"):
            self.assertEqual(parse_permit(canonical_permit(state))["state"], state)
        valid = json.loads(canonical_permit("first-open"))
        invalid = [
            canonical_permit("unknown"),
            json.dumps(valid, indent=2).encode("ascii"),
            canonical_permit("first-open").replace(b'"version":1', b'"version":2'),
            canonical_permit("first-open").replace(b'"version":1', b'"version":true'),
            canonical_permit("first-open").replace(b'00000000000000000000000001', b'00000000000000000000000003'),
            canonical_permit("first-open").replace(b'"generation":"' + b"a" * 64, b'"generation":"' + b"0" * 64),
            canonical_permit("first-open") + b" ",
            b"{}\n",
        ]
        for payload in invalid:
            with self.subTest(payload=payload), self.assertRaises(ValueError):
                parse_permit(payload)


def main():
    if sys.argv[1:] == ["--self-test"]:
        unittest.main(argv=[sys.argv[0]])
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--role", choices=("attempt", "publish"))
    parser.add_argument("--option")
    parser.add_argument("scratch", type=Path)
    arguments = parser.parse_args()
    require(sys.platform == "linux", "this fixture requires Linux")
    os.umask(0o077)
    if arguments.role == "attempt":
        require(arguments.option in ("normal", "hold", "before-directory-sync", "directory-sync-error"),
                "attempt cut differs")
        attempt(arguments.scratch, arguments.option)
    elif arguments.role == "publish":
        require(arguments.option in STATES, "synthetic permit state differs")
        publish(arguments.scratch, arguments.option)
    else:
        require(arguments.option is None, "observer does not take a role option")
        value = run_experiment(arguments.scratch)
        sys.stdout.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        report = {"experiment": "synthetic-cold-open-permit", "status": "incomplete",
                  "failure": {"kind": type(error).__name__,
                              "errno": error.errno if isinstance(error, OSError) else None}}
        sys.stdout.write(json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n")
        sys.exit(1)
