#!/usr/bin/env python3
import argparse
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time


PASSWORD = b"vivac-native-probe-fixture-only-2026"
TIMEOUT = 15


def isolated_environment(root):
    env = {
        "PATH": os.defpath,
        "LANG": "C.UTF-8",
        "HOME": str(root / "home"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(root / "bus"),
        "GNOME_KEYRING_CONTROL": str(root / "control"),
    }
    return env


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def restart_keyring(daemon, executable, binary, root, env):
    if daemon.poll() is not None:
        raise RuntimeError("isolated keyring daemon exited unexpectedly")
    stop(daemon)
    return start_keyring(executable, binary, root, env)


def stage(binary, root, env, name, timeout=TIMEOUT):
    if name not in ("missing", "ready", "create", "read", "wrong", "locked", "corrupt", "ambiguous"):
        raise RuntimeError("unknown probe stage")
    print(name, flush=True)
    result = subprocess.run(
        [str(binary), str(root), name], env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=timeout,
        check=False,
    )
    if result.returncode != 0:
        for line in result.stderr[:4096].splitlines():
            if re.fullmatch(rb"(?:probe step: [a-z][a-z ]{0,79}|probe I/O kind: [A-Za-z]{1,40})", line):
                print(line.decode("ascii"), file=sys.stderr, flush=True)
        raise RuntimeError("probe stage failed: " + name)


def validate_persistent_keyring(root):
    print("checking encrypted persistent file", flush=True)
    path = root / "data" / "keyrings" / "login.keyring"
    expected = path.lstat()

    def validate_metadata(info):
        if not stat.S_ISREG(info.st_mode):
            raise RuntimeError("persistent keyring is not a regular file")
        if info.st_nlink != 1:
            raise RuntimeError("persistent keyring link count was refused")
        if info.st_uid != os.geteuid():
            raise RuntimeError("persistent keyring owner was refused")
        if info.st_mode & 0o077:
            raise RuntimeError("persistent keyring permissions were refused")
        if not 16 <= info.st_size <= 1024 * 1024:
            raise RuntimeError("persistent keyring size was refused")

    validate_metadata(expected)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stored:
        actual = os.fstat(stored.fileno())
        validate_metadata(actual)
        if (actual.st_dev, actual.st_ino) != (expected.st_dev, expected.st_ino):
            raise RuntimeError("persistent keyring changed during validation")
        if stored.read(16) != b"GnomeKeyring\n\r\x00\n":
            raise RuntimeError("persistent keyring has an unexpected binary header")


def wait_ready(binary, root, env, daemon):
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            raise RuntimeError("isolated keyring daemon exited before readiness")
        try:
            stage(binary, root, env, "ready", timeout=min(2, max(0.1, deadline - time.monotonic())))
            return
        except RuntimeError:
            time.sleep(0.1)
    raise RuntimeError("isolated keyring readiness deadline exceeded")


def start_keyring(executable, binary, root, env):
    daemon = subprocess.Popen(
        [executable, "--foreground", "--unlock", "--components=secrets",
         "--control-directory=" + str(root / "control")],
        env=env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        daemon.stdin.write(PASSWORD)
        daemon.stdin.close()
        wait_ready(binary, root, env, daemon)
        return daemon
    except BaseException:
        stop(daemon)
        raise


def probe(binary):
    dbus = shutil.which("dbus-daemon", path=os.defpath)
    keyring = shutil.which("gnome-keyring-daemon", path=os.defpath)
    if not dbus or not keyring:
        raise RuntimeError("dbus-daemon and gnome-keyring-daemon are required")
    with tempfile.TemporaryDirectory(prefix="vivac-native-probe-", dir="/tmp") as directory:
        root = Path(directory).resolve()
        root.chmod(0o700)
        for name in ("home", "data", "config", "cache", "runtime", "control"):
            (root / name).mkdir(mode=0o700)
        (root / ".probe-owned").write_bytes(b"vivac.native-key-store-probe.v1\n")
        env = isolated_environment(root)
        # No service directories or external configuration can activate a provider.
        config = root / "bus.conf"
        from xml.sax.saxutils import escape
        config.write_text(
            '<busconfig><type>session</type><listen>unix:path='
            + escape(str(root / "bus"))
            + '</listen><auth>EXTERNAL</auth><policy context="default">'
            '<allow send_destination="*"/><allow receive_sender="*"/>'
            '<allow own="*"/></policy></busconfig>', encoding="utf-8",
        )
        bus = None
        daemon = None
        try:
            bus = subprocess.Popen(
                [dbus, "--nofork", "--config-file=" + str(config)], env=env,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            deadline = time.monotonic() + TIMEOUT
            while not (root / "bus").exists():
                if bus.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("isolated bus failed to start")
                time.sleep(0.05)
            stage(binary, root, env, "missing")
            daemon = start_keyring(keyring, binary, root, env)
            stage(binary, root, env, "create")
            validate_persistent_keyring(root)
            stage(binary, root, env, "read")
            daemon = restart_keyring(daemon, keyring, binary, root, env)
            stage(binary, root, env, "read")
            validate_persistent_keyring(root)
            stage(binary, root, env, "wrong")
            stage(binary, root, env, "locked")
            daemon = restart_keyring(daemon, keyring, binary, root, env)
            stage(binary, root, env, "read")
            validate_persistent_keyring(root)
            stage(binary, root, env, "corrupt")
            stage(binary, root, env, "ambiguous")
            if daemon.poll() is not None or bus.poll() is not None:
                raise RuntimeError("isolated provider or bus exited unexpectedly")
            print("Linux native key store probe passed")
        finally:
            stop(daemon)
            stop(bus)


def main():
    if sys.platform != "linux":
        print("This probe requires native Linux", file=sys.stderr)
        return 2
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    try:
        probe(binary)
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        messages = {
            "persistent keyring is not a regular file",
            "persistent keyring link count was refused",
            "persistent keyring owner was refused",
            "persistent keyring permissions were refused",
            "persistent keyring size was refused",
            "persistent keyring changed during validation",
            "persistent keyring has an unexpected binary header",
        }
        if isinstance(error, FileNotFoundError):
            message = "required file is absent"
        elif isinstance(error, PermissionError):
            message = "required file access was denied"
        elif isinstance(error, subprocess.TimeoutExpired):
            message = "child process deadline exceeded"
        elif type(error) is RuntimeError and error.args and error.args[0] in messages:
            message = error.args[0]
        else:
            message = "probe operation failed"
        print("Linux native key store probe failed: " + message, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
