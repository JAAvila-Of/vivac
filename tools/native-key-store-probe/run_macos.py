#!/usr/bin/env python3
"""Exercise only temporary fixture keychains on disposable hosted macOS runners."""

import argparse
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile


MARKER = b"vivac.native-key-store-probe.v1\n"
SAFE_DIAGNOSTIC = re.compile(
    rb"(?:probe step: (?:create-keychain|unlock|add|save-public|read|verify|open-missing|unlock-missing|get-status|lock)-(?:before|after)"
    rb"|probe step: (?:validating isolated root|root validated)"
    rb"|macOS status: -?[0-9]{1,11}|probe I/O kind: [A-Za-z]+)"
)


def require_hosted_macos(explicit):
    if (
        not explicit
        or sys.platform != "darwin"
        or os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
    ):
        raise RuntimeError("explicit disposable GitHub-hosted macOS runner required")


def validate_cleanup(root, temporary_parent):
    if root.is_symlink() or root.resolve(strict=True).parent != temporary_parent:
        raise RuntimeError("refusing cleanup outside the owned temporary directory")
    if stat.S_IMODE(root.stat().st_mode) != 0o700:
        raise RuntimeError("temporary directory permissions changed")
    marker = root / ".probe-owned"
    if marker.is_symlink() or marker.read_bytes() != MARKER:
        raise RuntimeError("temporary directory ownership marker changed")
    for child in root.rglob("*"):
        if child.is_symlink() or not child.resolve(strict=True).is_relative_to(root):
            raise RuntimeError("refusing cleanup of an unowned path")


def run_stage(binary, root, stage, expected_success=True):
    # No password, seed or item data is passed through arguments or printed.
    print(f"macos {stage}: starting", flush=True)
    result = subprocess.run(
        [str(binary), str(root), stage],
        check=False,
        capture_output=True,
        timeout=20,
    )
    expected_code = 0 if expected_success else 1
    if result.returncode != expected_code:
        remaining = 4096
        for line in (result.stderr or b"").splitlines():
            if SAFE_DIAGNOSTIC.fullmatch(line) and len(line) + 1 <= remaining:
                print(line.decode("ascii"), file=sys.stderr, flush=True)
                remaining -= len(line) + 1
        raise RuntimeError(f"unexpected result for macOS probe stage: {stage}")
    print(f"macos {stage}: passed", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-hosted-keychain-probe", action="store_true")
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    require_hosted_macos(args.allow_hosted_keychain_probe)
    binary = args.binary.resolve(strict=True)
    if not binary.is_file():
        raise RuntimeError("probe binary must be a file")
    temporary_parent = Path(tempfile.gettempdir()).resolve(strict=True)
    temporary = tempfile.TemporaryDirectory(
        prefix="vivac-native-key-store-probe-", dir=temporary_parent, delete=False
    )
    root = Path(temporary.name).resolve(strict=True)
    os.chmod(root, 0o700)
    (root / ".probe-owned").write_bytes(MARKER)
    try:
        # HOME stays unchanged: the disposable OS account provides isolation.
        run_stage(binary, root, "create")
        run_stage(binary, root, "read")
        run_stage(binary, root, "missing")
        run_stage(binary, root, "existing")
        run_stage(binary, root, "read")
        for stage in ("duplicate", "wrong", "locked", "read", "corrupt"):
            run_stage(binary, root, stage)
    finally:
        validate_cleanup(root, temporary_parent)
        temporary.cleanup()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        sys.exit("macOS native fixture probe failed")
