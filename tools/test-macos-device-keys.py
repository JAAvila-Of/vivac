#!/usr/bin/env python3
"""Exercise the production backend on explicitly authorized hosted macOS runners."""

import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time


PASSWORD = b"TEST-ONLY-public-fixture-device-key-2026"
WRONG = b"TEST-ONLY-public-fixture-wrong-key-2026"
NEW = b"New device key passphrase: "
CONFIRM = b"Confirm device key passphrase: "
UNLOCK = b"Device key passphrase: "
PROTECTION = "macos_classic_keychain_passphrase"


class Refused(Exception):
    """A fixed diagnostic safe to show in CI."""


def require(condition):
    if not condition:
        raise Refused("native assertion failed")


def inventory():
    result = []
    for verb in ("list-keychains", "default-keychain"):
        process = subprocess.run(
            ["/usr/bin/security", verb, "-d", "user"],
            capture_output=True, timeout=20, check=False,
        )
        require(process.returncode == 0)
        result.append(hashlib.sha256(process.stdout + process.stderr).digest())
    return result


def snapshot(directory):
    values = []
    for path in sorted(directory.rglob("*")):
        metadata = path.lstat()
        digest = None
        if stat.S_ISREG(metadata.st_mode):
            digest = hashlib.sha256(path.read_bytes()).digest()
        elif stat.S_ISLNK(metadata.st_mode):
            digest = os.readlink(path)
        values.append((str(path.relative_to(directory)), metadata.st_mode,
                       metadata.st_nlink, metadata.st_size, digest))
    return values


class Runner:
    def __init__(self, executable, root):
        self.executable = executable
        self.root = root
        self.work = root / "work"
        self.work.mkdir(mode=0o700)
        self.environment = os.environ.copy()
        for name in ("HOME", "USERPROFILE", "CODEX_HOME", "VIVAC_HOME"):
            destination = root / name.lower()
            destination.mkdir(mode=0o700)
            self.environment[name] = str(destination)

    def command(self, arguments, answers=(), interrupt=False, stdin=None):
        import pty
        import select
        import termios

        command = [str(self.executable), "device-key", *map(str, arguments), "--json"]
        if stdin is not None:
            process = subprocess.Popen(
                command, cwd=self.work, env=self.environment,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                start_new_session=True,
            )
            try:
                process.stdin.write(stdin)
                process.stdin.close()
                streams = {process.stdout.fileno(): bytearray(), process.stderr.fileno(): bytearray()}
                active = list(streams)
                deadline = time.monotonic() + 20
                while active:
                    require(time.monotonic() < deadline)
                    ready, _, _ = select.select(active, [], [], 0.02)
                    for descriptor in ready:
                        chunk = os.read(descriptor, 4096)
                        if not chunk:
                            active.remove(descriptor)
                        else:
                            streams[descriptor].extend(chunk)
                            limit = 65536 if descriptor == process.stdout.fileno() else 16384
                            require(len(streams[descriptor]) <= limit)
                code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
                output, error = (bytes(streams[pipe.fileno()]) for pipe in (process.stdout, process.stderr))
                self.no_echo((output, error), (PASSWORD, WRONG))
                return code, output, error, b""
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                if not process.stdin.closed:
                    process.stdin.close()
                process.stdout.close()
                process.stderr.close()

        output_read, output_write = os.pipe()
        error_read, error_write = os.pipe()
        pid, terminal = pty.fork()
        if pid == 0:
            try:
                os.close(output_read)
                os.close(error_read)
                os.dup2(output_write, 1)
                os.dup2(error_write, 2)
                os.close(output_write)
                os.close(error_write)
                os.chdir(self.work)
                os.execve(self.executable, command, self.environment)
            except BaseException:
                os._exit(127)
        os.close(output_write)
        os.close(error_write)
        descriptors = {output_read: 65536, error_read: 16384, terminal: 16384}
        buffers = {descriptor: bytearray() for descriptor in descriptors}
        pending = list(answers)
        sent = []
        offset = 0
        status = None
        interrupted = False
        deadline = time.monotonic() + 20
        try:
            while descriptors or status is None:
                require(time.monotonic() < deadline)
                if status is None:
                    waited, value = os.waitpid(pid, os.WNOHANG)
                    if waited:
                        status = value
                ready, _, _ = select.select(list(descriptors), [], [], 0.02)
                for descriptor in ready:
                    try:
                        chunk = os.read(descriptor, 4096)
                    except OSError as error:
                        if descriptor == terminal and error.errno == errno.EIO:
                            chunk = b""
                        else:
                            raise
                    if not chunk:
                        descriptors.pop(descriptor)
                    else:
                        buffers[descriptor].extend(chunk)
                        require(len(buffers[descriptor]) <= descriptors[descriptor])
                transcript = buffers[terminal]
                prompt = pending[0][0] if pending else (NEW if interrupt else None)
                if prompt is not None:
                    position = transcript.find(prompt, offset)
                    if position >= 0:
                        echo_deadline = min(deadline, time.monotonic() + 1)
                        while termios.tcgetattr(terminal)[3] & termios.ECHO:
                            require(time.monotonic() < echo_deadline)
                            select.select([], [], [], 0.005)
                        offset = position + len(prompt)
                        if interrupt:
                            os.kill(pid, signal.SIGINT)
                            interrupted = True
                            interrupt = False
                        else:
                            _, answer = pending.pop(0)
                            sent.append(answer)
                            payload = answer + b"\n"
                            while payload:
                                written = os.write(terminal, payload)
                                payload = payload[written:]
            require(not pending)
            if interrupted:
                require(termios.tcgetattr(terminal)[3] & termios.ECHO)
            streams = tuple(bytes(buffers[fd]) for fd in (output_read, error_read, terminal))
            self.no_echo(streams, sent + [PASSWORD, WRONG])
            return os.waitstatus_to_exitcode(status), *streams
        finally:
            if status is None:
                try:
                    os.killpg(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                os.waitpid(pid, 0)
            for descriptor in (output_read, error_read, terminal):
                os.close(descriptor)

    @staticmethod
    def no_echo(streams, answers):
        for answer in answers:
            if answer:
                require(all(answer not in stream for stream in streams))

    def success(self, arguments, answers=(), stdin=None):
        code, output, error, terminal = self.command(arguments, answers, stdin=stdin)
        if code != 0 or error:
            verb = arguments[0] if arguments[0] in {
                "create", "show", "prove", "verify-proof", "lock", "unlock"
            } else "unknown"
            print(f"failed: {verb} command (exit {code})", file=sys.stderr)
            diagnostics = {
                "Device key refused: the macOS protected key operation failed.": "protected operation",
                "Device key refused: the device key passphrase is invalid or unavailable.": "terminal passphrase",
                "Private storage permissions are unavailable.": "private filesystem profile",
                "Device key refused: the protected key package changed or has an invalid size.": "package identity or size",
                "Device key refused: the new keychain accepted an empty passphrase.": "empty passphrase accepted",
                "Device key refused: the protected key package has unexpected members.": "unexpected package members",
                "Device key refused: the device key store is locked; unlock it explicitly.": "locked store",
                "Device key refused: the protected key item is missing or ambiguous.": "ambiguous item",
                "Device key refused: the protected key item changed during the operation.": "item identity changed",
                "Device key refused: the protected key does not match its generated identity.": "generated identity mismatch",
                "Device key refused: the protected key payload has an unsupported size or version.": "payload format",
                "Device key refused: the device key manifest identity is invalid.": "manifest identity",
                "Device key refused: the device key manifest is invalid.": "manifest format",
                "Device key refused: the protected key database does not match its manifest digest.": "database digest",
                "Private bundle preparation permissions are unavailable.": "private ownership or mode",
                "Artifacts must be regular files.": "regular file requirement",
                "SQLite source files must have exactly one hard link.": "single link requirement",
                "Artifacts must not cross symbolic links or reparse points.": "symbolic link requirement",
            }
            for line in error[:4096].decode("ascii", errors="ignore").splitlines():
                if line.strip() in diagnostics:
                    print(f"failure category: {diagnostics[line.strip()]}", file=sys.stderr)
                match = re.fullmatch(
                    r"\s*Device key refused: the macOS keychain "
                    r"(creation|unlock|open|item creation|lookup|status query|lock|"
                    r"UI policy|empty passphrase check|initial unlock) failed \(OSStatus (-?[0-9]+)\)\.\s*",
                    line,
                )
                if match:
                    print(f"native failure: {match[1]} (OSStatus {match[2]})", file=sys.stderr)
        require(code == 0 and not error)
        if not answers:
            require(not terminal)
        return json.loads(output)

    def reject(self, arguments, answers=(), stdin=None, interrupt=False):
        before = snapshot(self.root)
        code, output, _, terminal = self.command(arguments, answers, interrupt, stdin)
        require(code != 0 and not output)
        if not answers and not interrupt:
            require(not terminal)
        require(snapshot(self.root) == before)

    def create(self, name):
        path = self.root / name
        receipt = self.success(["create", path], [(NEW, PASSWORD), (CONFIRM, PASSWORD)])
        require(set(receipt) == {"device_id", "credential_id", "public_key", "protection", "status"})
        require(receipt["protection"] == PROTECTION and receipt["status"] == "local_key_only")
        require(stat.S_IMODE(path.stat().st_mode) == 0o700)
        names = {item.name for item in path.iterdir()}
        require("device-key.json" in names)
        require(len(names & {"device.keychain", "device.keychain-db"}) == 1)
        database = next(name for name in names if name.startswith("device.keychain"))
        marker = ".fl" + hashlib.sha1(database.encode("ascii"), usedforsecurity=False).hexdigest()[:8].upper()
        require(names == {database, marker, "device-key.json"})
        metadata = (path / marker).lstat()
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1)
        require(metadata.st_uid == os.geteuid() and metadata.st_size == 0)
        require(stat.S_IMODE(metadata.st_mode) == 0o400)
        return path, receipt

    def prove(self, path, receipt):
        now = int(time.time())
        challenge = {
            "version": 1, "organization_id": "00000000000000000000000001",
            "principal_id": "00000000000000000000000002",
            "operation_id": "00000000000000000000000003", "base_revision": 0,
            "nonce": secrets.token_hex(32), "issued_at": now, "expires_at": now + 240,
            **{key: receipt[key] for key in ("device_id", "credential_id", "public_key")},
        }
        request = self.work / "challenge.json"
        request.write_text(json.dumps(challenge), encoding="utf-8")
        proof = self.success(["prove", path, "--challenge", request], stdin=b"")
        require(proof["challenge"] == challenge and proof["status"] == "possession_only")
        require(proof["authority"] == "not_checked" and proof["challenge_use"] == "not_checked")
        document = self.work / "proof.json"
        document.write_text(json.dumps(proof), encoding="utf-8")
        require(self.success(["verify-proof", document], stdin=b"") == proof)
        return request

    def copied(self, source, name):
        destination = self.root / name
        shutil.copytree(source, destination)
        return destination

    def corrupt_loaded_database(self, path, receipt, request):
        database = next(item for item in path.iterdir() if item.name.startswith("device.keychain"))
        metadata = database.lstat()
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1)
        identity = metadata.st_dev, metadata.st_ino, metadata.st_mode
        with database.open("r+b") as stream:
            opened = os.fstat(stream.fileno())
            require((opened.st_dev, opened.st_ino, opened.st_mode) == identity)
            original = stream.read(1024 * 1024 + 1)
            require(0 < len(original) <= 1024 * 1024)
            try:
                stream.seek(0)
                stream.write(bytes([original[0] ^ 1]))
                stream.flush()
                os.fsync(stream.fileno())
                changed = database.lstat()
                require((changed.st_dev, changed.st_ino, changed.st_mode) == identity)
                self.reject(["show", path])
                self.reject(["prove", path, "--challenge", request])
            finally:
                stream.seek(0)
                stream.write(original)
                stream.truncate(len(original))
                stream.flush()
                os.fsync(stream.fileno())
                restored = database.lstat()
                require((restored.st_dev, restored.st_ino, restored.st_mode) == identity)
                require(database.read_bytes() == original)
        require(self.success(["show", path]) == receipt)

    def exercise(self):
        print("stage: creation and possession", flush=True)
        path, receipt = self.create("primary")
        for _ in range(2):
            require(self.success(["show", path], stdin=b"") == receipt)
        request = self.prove(path, receipt)
        self.reject(["create", path])

        print("stage: explicit locking", flush=True)
        require(self.success(["lock", path]) == {"status": "locked", "protection": PROTECTION})
        self.reject(["show", path])
        self.reject(["prove", path, "--challenge", request])
        self.reject(["unlock", path], [(UNLOCK, WRONG)])
        require(self.success(["unlock", path], [(UNLOCK, PASSWORD)]) ==
                {"status": "unlocked", "protection": PROTECTION})
        require(self.success(["show", path]) == receipt)
        request = self.prove(path, receipt)

        print("stage: loaded database integrity", flush=True)
        self.corrupt_loaded_database(path, receipt, request)

        print("stage: unsafe inputs", flush=True)
        for name, answers in (
            ("mismatch", [(NEW, PASSWORD), (CONFIRM, WRONG)]),
            ("oversize", [(NEW, b"x" * 513)]),
        ):
            self.reject(["create", self.root / name], answers)
        self.reject(["create", self.root / "stdin"], stdin=PASSWORD + b"\n" + PASSWORD + b"\n")
        self.reject(["create", self.root / "interrupted"], interrupt=True)

        print("stage: damaged profiles", flush=True)
        for kind in ("malformed", "unknown", "identity", "database", "missing"):
            damaged = self.copied(path, "damage-" + kind)
            manifest = damaged / "device-key.json"
            if kind == "malformed":
                manifest.write_bytes(b"{")
            elif kind == "missing":
                manifest.unlink()
            elif kind == "database":
                database = next(item for item in damaged.iterdir() if item.name.startswith("device.keychain"))
                database.write_bytes(b"truncated-test-only")
            else:
                value = json.loads(manifest.read_bytes())
                if kind == "unknown":
                    value["unexpected"] = True
                else:
                    value["device_id"] = "00000000000000000000000007"
                manifest.write_text(json.dumps(value), encoding="utf-8")
            self.reject(["show", damaged])
            self.reject(["unlock", damaged])

        print("stage: filesystem boundaries", flush=True)
        login_name = self.root / "login.keychain-private"
        login_name.mkdir(mode=0o700)
        self.reject(["create", login_name / "credential"])
        alias = self.root / "profile-symlink"
        alias.symlink_to(path, target_is_directory=True)
        self.reject(["show", alias])
        self.reject(["create", alias / "child"])
        database = next(item.name for item in path.iterdir() if item.name.startswith("device.keychain"))
        marker = next(item.name for item in path.iterdir() if item.name.startswith(".fl"))
        for kind in ("missing", "nonempty", "wrong-name"):
            damaged = self.copied(path, "marker-" + kind)
            target = damaged / marker
            if kind == "missing":
                target.unlink()
            elif kind == "nonempty":
                target.chmod(0o600)
                target.write_bytes(b"synthetic-lock-data")
                target.chmod(0o400)
            else:
                target.rename(damaged / ".fl00000000")
            self.reject(["show", damaged])
            self.reject(["unlock", damaged])
        for filename in ("device-key.json", database, marker):
            for kind in ("symlink", "fifo", "hardlink", "mode", "acl"):
                damaged = self.copied(path, "boundary-" + filename + "-" + kind)
                target = damaged / filename
                if kind in ("symlink", "fifo", "hardlink"):
                    target.unlink()
                if kind == "symlink":
                    target.symlink_to(path / filename)
                elif kind == "fifo":
                    os.mkfifo(target, 0o600)
                elif kind == "hardlink":
                    os.link(path / filename, target)
                elif kind == "mode":
                    target.chmod(0o644)
                else:
                    result = subprocess.run(
                        ["/bin/chmod", "+a", "everyone allow read", str(target)],
                        capture_output=True, timeout=20, check=False,
                    )
                    require(result.returncode == 0)
                self.reject(["show", damaged])
                self.reject(["unlock", damaged])
                if kind == "hardlink":
                    target.unlink()
        directory_mode = self.copied(path, "boundary-directory-mode")
        directory_mode.chmod(0o755)
        self.reject(["show", directory_mode])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--allow-hosted-keychain-test", action="store_true")
    arguments = parser.parse_args()
    # This gate precedes temporary paths and all native operations.
    if (sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
            or not arguments.allow_hosted_keychain_test):
        print("refused: explicitly authorized hosted macOS runner required", file=sys.stderr)
        return 2
    root = None
    identity = None
    marker = secrets.token_bytes(32)
    before = None
    failed = False
    try:
        executable = arguments.executable.resolve(strict=True)
        require(executable.is_file() and os.access(executable, os.X_OK))
        before = inventory()
        root = Path(tempfile.mkdtemp(prefix="vivac-device-key-ci-")).resolve(strict=True)
        root.chmod(0o700)
        metadata = root.lstat()
        identity = metadata.st_dev, metadata.st_ino
        (root / ".owned-ci-root").write_bytes(marker)
        (root / ".owned-ci-root").chmod(0o600)
        Runner(executable, root).exercise()
    except BaseException:
        failed = True
        print("failed: native device key validation", file=sys.stderr)
    finally:
        if before is not None:
            try:
                require(inventory() == before)
            except BaseException:
                failed = True
                print("failed: keychain inventory preservation", file=sys.stderr)
        if root is not None:
            try:
                metadata = root.lstat()
                require(not root.is_symlink() and root.resolve(strict=True) == root)
                require((metadata.st_dev, metadata.st_ino) == identity)
                ownership = root / ".owned-ci-root"
                require(not ownership.is_symlink() and ownership.read_bytes() == marker)
                # `shutil.rmtree` does not traverse directory symlinks.
                shutil.rmtree(root)
            except BaseException:
                failed = True
                print("failed: private fixture cleanup", file=sys.stderr)
    if failed:
        return 1
    print("passed: native device key validation")
    return 0


if __name__ == "__main__":
    sys.exit(main())
