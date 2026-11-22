#!/usr/bin/env python3
"""Reproduce format boundaries in a disposable copy of a fixed upstream tree."""

import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import shutil
import subprocess
import tarfile
import tempfile


COMMIT = "4251a43abce2a0633fa35cdf3cd360af1fa88bc0"
OBSERVATIONS = {
    "empty_container_accepts_two_unrelated_passwords",
    "removing_all_items_preserves_password_success_without_expected_item",
    "deleting_external_attribute_hashes_preserves_authenticated_item",
    "encrypted_blobs_shorter_than_mac_panic_instead_of_returning_error",
    "maximum_iteration_count_passes_strength_check_without_derivation",
}


def extract_sources(archive, destination):
    total = 0
    with tarfile.open(archive) as contents:
        for count, member in enumerate(contents, 1):
            parts = PurePosixPath(member.name)
            source_member = member
            if member.issym():
                # Upstream links package notices to the root license. Copy only
                # that known regular member; never create filesystem links.
                linked = posixpath.normpath(str(parts.parent / member.linkname))
                if parts.name != "LICENSE" or linked != "LICENSE":
                    raise RuntimeError("unsupported source archive link")
                source_member = contents.getmember("LICENSE")
                if not source_member.isfile() or source_member.size > 65536:
                    raise RuntimeError("unsupported source archive license")
            if (count > 10000 or parts.is_absolute() or ".." in parts.parts
                    or "\\" in member.name or ":" in member.name
                    or not (member.isdir() or source_member.isfile())):
                raise RuntimeError("unsupported source archive member")
            total += source_member.size
            if source_member.size < 0 or source_member.size > 16 * 1024 * 1024 or total > 128 * 1024 * 1024:
                raise RuntimeError("source archive exceeds review bounds")
            output = destination.joinpath(*parts.parts)
            if member.isdir():
                output.mkdir(parents=True, exist_ok=True)
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                with contents.extractfile(source_member) as incoming, output.open("xb") as outgoing:
                    shutil.copyfileobj(incoming, outgoing, 65536)


def run(source, target_dir):
    revision = subprocess.check_output(
        ["git", "-C", str(source), "rev-parse", "HEAD"], text=True,
    ).strip()
    if revision != COMMIT:
        raise RuntimeError("the source checkout is not the reviewed commit")
    fragment = Path(__file__).with_name("oo7_format_tests.rs").read_bytes()
    with tempfile.TemporaryDirectory(prefix="vivac-oo7-format-") as directory:
        root = Path(directory)
        archive = root / "source.tar"
        with archive.open("wb") as output:
            subprocess.run(
                ["git", "-C", str(source), "archive", "--format=tar", COMMIT],
                stdout=output, check=True,
            )
        print("upstream_commit=" + COMMIT, flush=True)
        with archive.open("rb") as archived:
            archive_digest = hashlib.file_digest(archived, "sha256").hexdigest()
        print("source_archive_sha256=" + archive_digest, flush=True)
        print("review_fragment_sha256=" + hashlib.sha256(fragment).hexdigest(), flush=True)
        copied = root / "source"
        copied.mkdir()
        extract_sources(archive, copied)
        module = copied / "client" / "src" / "file" / "api" / "mod.rs"
        with module.open("ab") as output:
            output.write(b"\n")
            output.write(fragment)
        environment = {
            name: os.environ[name]
            for name in ("PATH", "CARGO_HOME", "RUSTUP_HOME", "SYSTEMROOT",
                         "WINDIR", "TEMP", "TMP", "LANG", "LC_ALL")
            if name in os.environ
        }
        environment["CARGO_TARGET_DIR"] = str(target_dir)
        result = subprocess.run(
            ["cargo", "test", "--locked", "-p", "oo7", "--lib",
             "--features", "unstable", "vivac_format_review", "--", "--nocapture"],
            cwd=copied, env=environment, check=False, stdout=subprocess.PIPE,
            text=True, encoding="utf-8", errors="replace",
        )
        print(result.stdout, end="", flush=True)
        if result.returncode == 0:
            passed = set(re.findall(
                r"^test file::api::vivac_format_review::([a-z_]+) \.\.\. ok$",
                result.stdout, flags=re.MULTILINE,
            ))
            if passed != OBSERVATIONS or not re.search(
                r"test result: ok\. 5 passed; 0 failed; 0 ignored;", result.stdout,
            ):
                raise RuntimeError("the five required observations did not all run")
            print("Format observations reproduced; provider acceptance is not established.", flush=True)
        return result.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True,
                        help="Read-only checkout of oo7 at the reviewed commit")
    parser.add_argument("--target-dir", type=Path, required=True,
                        help="New disposable Cargo build directory")
    arguments = parser.parse_args()
    source = arguments.source.resolve(strict=True)
    target = arguments.target_dir.absolute()
    # A fresh build destination prevents stale binaries from crediting new tests.
    target.mkdir(parents=False, exist_ok=False)
    try:
        return run(source, target)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print("Format review failed: " + str(error), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
