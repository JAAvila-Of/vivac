"""Experimental Linux-only offline client release provenance probe."""

import argparse
from dataclasses import dataclass
from datetime import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time


MIB = 1024 * 1024
ARCHIVE_LIMIT = 128 * MIB
BUNDLE_LIMIT = 4 * MIB
ROOTS_LIMIT = MIB
VERIFIER_LIMIT = 128 * MIB
STDOUT_LIMIT = 2 * MIB
STDERR_LIMIT = 64 * 1024
TARGETS = (
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)
REPOSITORY = "JAAvila-Of/vivac"
REPOSITORY_ID = "1352788684"
OWNER_ID = "69764058"
SOURCE_REF = "refs/heads/main"
WORKFLOW = REPOSITORY + "/.github/workflows/release.yml@" + SOURCE_REF
WORKFLOW_IDENTITY = "https://github.com/" + WORKFLOW
ISSUER = "https://token.actions.githubusercontent.com"
PREDICATE_TYPE = "https://slsa.dev/provenance/v1"
ERROR_MESSAGE = "client provenance verification failed\n"


class ProbeError(Exception):
    """An input or verification result failed the probe's policy."""


@dataclass(frozen=True)
class Snapshot:
    path: Path
    sha256: str
    limit: int
    identity: tuple


def _identity(info):
    return (
        info.st_dev, info.st_ino, info.st_mode, info.st_nlink,
        info.st_size, info.st_mtime_ns, info.st_ctime_ns,
    )


def _regular(info, limit):
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ProbeError()
    if info.st_size < 0 or info.st_size > limit:
        raise ProbeError()


def _stream_regular(source, limit, output=None):
    """Read a bounded regular file, rejecting replacement or concurrent writes."""
    before = source.lstat()
    _regular(before, limit)
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    descriptor = os.open(source, flags)
    try:
        opened = os.fstat(descriptor)
        _regular(opened, limit)
        if _identity(before) != _identity(opened):
            raise ProbeError()
        digest = hashlib.sha256()
        count = 0
        while True:
            block = os.read(descriptor, min(64 * 1024, limit - count + 1))
            if not block:
                break
            count += len(block)
            if count > limit:
                raise ProbeError()
            digest.update(block)
            if output is not None:
                view = memoryview(block)
                while view:
                    written = os.write(output, view)
                    if written <= 0:
                        raise ProbeError()
                    view = view[written:]
        after = os.fstat(descriptor)
        named = source.lstat()
        if (count != opened.st_size
                or _identity(opened) != _identity(after)
                or _identity(opened) != _identity(named)):
            raise ProbeError()
        return digest.hexdigest(), _identity(after)
    finally:
        os.close(descriptor)


def snapshot_file(source, destination, limit, executable=False):
    """Copy input bytes to a new private file before invoking any verifier."""
    descriptor = os.open(
        destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC,
        0o600,
    )
    try:
        digest, _ = _stream_regular(Path(source), limit, descriptor)
        os.fchmod(descriptor, 0o500 if executable else 0o400)
        identity = _identity(os.fstat(descriptor))
    finally:
        os.close(descriptor)
    snapshot = Snapshot(Path(destination), digest, limit, identity)
    check_snapshot(snapshot)
    return snapshot


def check_snapshot(snapshot):
    digest, identity = _stream_regular(snapshot.path, snapshot.limit)
    if digest != snapshot.sha256 or identity != snapshot.identity:
        raise ProbeError()


def run_bounded(command, environment, cwd, timeout):
    """Drain bounded pipes within a deadline and reap this process group only."""
    deadline = time.monotonic() + timeout
    process = subprocess.Popen(
        command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, env=environment, cwd=cwd,
        start_new_session=True, close_fds=True, bufsize=0,
    )
    try:
        with selectors.DefaultSelector() as selector:
            captured = {"stdout": bytearray(), "stderr": bytearray()}
            limits = {"stdout": STDOUT_LIMIT, "stderr": STDERR_LIMIT}
            for stream, name in ((process.stdout, "stdout"),
                                 (process.stderr, "stderr")):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, name)
            exit_info = None
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ProbeError()
                if exit_info is None:
                    observed = os.waitid(
                        os.P_PID, process.pid,
                        os.WEXITED | os.WNOHANG | os.WNOWAIT,
                    )
                    if observed is not None and observed.si_pid == process.pid:
                        exit_info = observed
                if not selector.get_map() and exit_info is not None:
                    break
                for key, _ in selector.select(min(0.1, remaining)):
                    name = key.data
                    room = limits[name] - len(captured[name])
                    try:
                        block = os.read(key.fd, min(64 * 1024, room + 1))
                    except BlockingIOError:
                        continue
                    if not block:
                        selector.unregister(key.fileobj)
                        continue
                    if len(block) > room:
                        raise ProbeError()
                    captured[name].extend(block)
            if exit_info.si_code != os.CLD_EXITED or exit_info.si_status != 0:
                raise ProbeError()
            return bytes(captured["stdout"])
    finally:
        # Keep the leader unreaped until its group is killed, preventing PID reuse.
        # A successful parent can still have descendants holding input files.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        finally:
            process.wait()
            process.stdout.close()
            process.stderr.close()


def _check_version(output):
    first_line = output.split(b"\n", 1)[0]
    match = re.fullmatch(
        rb"gh version 2\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        rb"(?: \([0-9]{4}-[0-9]{2}-[0-9]{2}\))?", first_line,
    )
    if match is None or int(match.group(1)) < 102:
        raise ProbeError()


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ProbeError()
        value[key] = item
    return value


def _reject_constant(_value):
    raise ProbeError()


def _finite_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise ProbeError()
    return number


def parse_verified_output(output):
    """Parse only the bounded output of a successfully executed verifier."""
    if len(output) > STDOUT_LIMIT:
        raise ProbeError()
    value = json.loads(
        output.decode("utf-8"), object_pairs_hook=_unique_object,
        parse_constant=_reject_constant, parse_float=_finite_float,
    )
    if not isinstance(value, list) or not 1 <= len(value) <= 16:
        raise ProbeError()
    return value


def _valid_timestamp(item):
    if not isinstance(item, dict):
        return False
    kind = item.get("type")
    timestamp = item.get("timestamp")
    if not isinstance(kind, str) or not kind or not isinstance(timestamp, str):
        return False
    if re.fullmatch(
            r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
            r"(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})", timestamp) is None:
        return False
    try:
        return datetime.fromisoformat(timestamp).utcoffset() is not None
    except ValueError:
        return False


def policy_matches(value, archive_name, archive_sha256, source_commit):
    """Require every policy claim within one complete verified result."""
    if not isinstance(value, list) or not 1 <= len(value) <= 16:
        return False
    expected_certificate = {
        "sourceRepositoryIdentifier": REPOSITORY_ID,
        "sourceRepositoryOwnerIdentifier": OWNER_ID,
        "sourceRepositoryURI": "https://github.com/" + REPOSITORY,
        "sourceRepositoryDigest": source_commit,
        "sourceRepositoryRef": SOURCE_REF,
        "subjectAlternativeName": WORKFLOW_IDENTITY,
        "issuer": ISSUER,
        "runnerEnvironment": "github-hosted",
    }
    for entry in value:
        if not isinstance(entry, dict):
            continue
        result = entry.get("verificationResult")
        if not isinstance(result, dict):
            continue
        signature = result.get("signature")
        statement = result.get("statement")
        timestamps = result.get("verifiedTimestamps")
        if not isinstance(signature, dict) or not isinstance(statement, dict):
            continue
        certificate = signature.get("certificate")
        if not isinstance(certificate, dict) or not all(
                isinstance(certificate.get(key), str)
                and certificate[key] == expected
                for key, expected in expected_certificate.items()):
            continue
        if (not isinstance(timestamps, list) or not timestamps
                or not all(_valid_timestamp(item) for item in timestamps)):
            continue
        if statement.get("predicateType") != PREDICATE_TYPE:
            continue
        subjects = statement.get("subject")
        if not isinstance(subjects, list):
            continue
        if any(isinstance(subject, dict)
               and subject.get("name") == archive_name
               and subject.get("digest") == {"sha256": archive_sha256}
               for subject in subjects):
            return True
    return False


def verify(args):
    if sys.platform != "linux":
        raise ProbeError()
    for digest in (args.gh_sha256, args.roots_sha256):
        if re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ProbeError()
    if re.fullmatch(r"[0-9a-f]{40}", args.source_commit) is None:
        raise ProbeError()
    if args.target not in TARGETS:
        raise ProbeError()
    suffix = ".zip" if args.target == "x86_64-pc-windows-msvc" else ".tar.gz"
    archive_name = "vivac-" + args.target + suffix
    with tempfile.TemporaryDirectory(prefix="vivac-provenance-") as temporary:
        private = Path(temporary)
        os.chmod(private, 0o700)
        archive = snapshot_file(args.archive, private / archive_name, ARCHIVE_LIMIT)
        bundle = snapshot_file(args.bundle, private / "bundle.jsonl", BUNDLE_LIMIT)
        roots = snapshot_file(args.roots, private / "roots.jsonl", ROOTS_LIMIT)
        verifier = snapshot_file(args.gh, private / "gh", VERIFIER_LIMIT, True)
        snapshots = (archive, bundle, roots, verifier)
        if verifier.sha256 != args.gh_sha256 or roots.sha256 != args.roots_sha256:
            raise ProbeError()
        home = private / "home"
        home.mkdir(mode=0o700)
        config = home / "config"
        config.mkdir(mode=0o700)
        environment = {
            "PATH": "/usr/bin:/bin", "HOME": str(home),
            "GH_CONFIG_DIR": str(config), "GH_NO_UPDATE_NOTIFIER": "1",
            "GH_PROMPT_DISABLED": "1",
            "GH_TELEMETRY": "false", "DO_NOT_TRACK": "true",
        }
        version = run_bounded([str(verifier.path), "--version"],
                              environment, private, 5)
        for snapshot in snapshots:
            check_snapshot(snapshot)
        _check_version(version)
        command = [
            str(verifier.path), "attestation", "verify", str(archive.path),
            "--bundle", str(bundle.path), "--custom-trusted-root", str(roots.path),
            "--repo", REPOSITORY, "--signer-workflow", WORKFLOW,
            "--cert-oidc-issuer", ISSUER, "--source-digest", args.source_commit,
            "--source-ref", SOURCE_REF, "--deny-self-hosted-runners",
            "--digest-alg", "sha256", "--format", "json", "--hostname", "github.com",
        ]
        output = run_bounded(command, environment, private, 30)
        for snapshot in snapshots:
            check_snapshot(snapshot)
        value = parse_verified_output(output)
        if not policy_matches(value, archive_name, archive.sha256, args.source_commit):
            raise ProbeError()
        return {
            "type": "client-release-provenance-probe-v1",
            "archive_name": archive_name, "sha256": archive.sha256,
            "source_commit": args.source_commit,
            "repository_id": REPOSITORY_ID, "owner_id": OWNER_ID,
            "workflow": WORKFLOW_IDENTITY,
            "verifier_sha256": verifier.sha256, "roots_sha256": roots.sha256,
        }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("archive", "bundle", "gh", "gh-sha256", "roots",
                 "roots-sha256", "source-commit"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--target", required=True, choices=TARGETS)
    args = parser.parse_args(argv)
    try:
        result = verify(args)
        encoded = json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n"
    except (ProbeError, OSError, ValueError, OverflowError, RecursionError):
        sys.stderr.write(ERROR_MESSAGE)
        return 1
    sys.stdout.write(encoded)
    return 0


if __name__ == "__main__":
    sys.exit(main())
