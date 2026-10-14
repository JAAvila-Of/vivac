"""Probe Unix exclusive directory publication with synthetic fixtures only."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import threading
import uuid
from pathlib import Path
from typing import Any


SCRATCH_NAME = ".scratch"
FIXTURE_ROOT_NAME = "directory-publication-probe"
RUN_PREFIX = "run-"
TOKEN_NAME = ".child-token"
LINUX_RENAME_NOREPLACE = 1
MACOS_RENAME_EXCL = 4
EXIT_OK = 0
EXIT_FAILED = 1
EXIT_ERROR = 2
EXIT_INCOMPLETE = 3


class ProbeUnavailable(Exception):
    pass


def _emit(payload: dict[str, Any], code: int) -> int:
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return code


def _directory_flags() -> int:
    required = ("O_DIRECTORY", "O_NOFOLLOW")
    if any(not hasattr(os, name) for name in required):
        raise ProbeUnavailable("directory no-follow open flags are unavailable")
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)


def _open_absolute_directory(path: Path) -> int:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("workspace must be absolute and contain no parent traversal")
    flags = _directory_flags()
    fd = os.open(path.anchor, flags)
    try:
        for part in path.parts[1:]:
            if part in {"", ".", ".."}:
                raise ValueError("workspace contains an unsafe path component")
            next_fd = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return fd
    except BaseException:
        os.close(fd)
        raise


def _open_child_directory(parent_fd: int, leaf: str) -> int:
    if not leaf or leaf in {".", ".."} or "/" in leaf or "\\" in leaf:
        raise ValueError("expected a single directory entry name")
    return os.open(leaf, _directory_flags(), dir_fd=parent_fd)


def _lstat_at(parent_fd: int, leaf: str) -> os.stat_result:
    return os.stat(leaf, dir_fd=parent_fd, follow_symlinks=False)


def _open_or_create_directory(parent_fd: int, leaf: str, mode: int = 0o700) -> int:
    try:
        os.mkdir(leaf, mode, dir_fd=parent_fd)
    except FileExistsError:
        info = _lstat_at(parent_fd, leaf)
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise OSError(errno.ELOOP, "fixture path is not a real directory")
    directory_fd = _open_child_directory(parent_fd, leaf)
    named = _lstat_at(parent_fd, leaf)
    held = os.fstat(directory_fd)
    if stat.S_ISLNK(named.st_mode) or not stat.S_ISDIR(named.st_mode):
        os.close(directory_fd)
        raise OSError(errno.ELOOP, "fixture path is not a real directory")
    if (named.st_dev, named.st_ino) != (held.st_dev, held.st_ino):
        os.close(directory_fd)
        raise OSError(errno.ESTALE, "fixture directory identity changed while opening")
    return directory_fd


def _exclusive_api() -> tuple[Any, int, str]:
    if sys.platform.startswith("linux"):
        symbol, flag = "renameat2", LINUX_RENAME_NOREPLACE
    elif sys.platform == "darwin":
        symbol, flag = "renameatx_np", MACOS_RENAME_EXCL
    else:
        raise ProbeUnavailable("unsupported platform; no rename fallback is permitted")
    try:
        function = getattr(ctypes.CDLL(None, use_errno=True), symbol)
    except (AttributeError, OSError) as exc:
        raise ProbeUnavailable(f"exclusive rename symbol unavailable: {symbol}") from exc
    function.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    function.restype = ctypes.c_int
    return function, flag, symbol


def _rename_exclusive(function: Any, flag: int, source_fd: int, source: str, target_fd: int, target: str) -> None:
    result = function(source_fd, os.fsencode(source), target_fd, os.fsencode(target), flag)
    if result == 0:
        return
    error = ctypes.get_errno()
    unsupported = {
        errno.EINVAL,
        errno.ENOSYS,
        getattr(errno, "EOPNOTSUPP", errno.ENOTSUP),
        errno.ENOTSUP,
    }
    if error in unsupported:
        raise ProbeUnavailable(f"exclusive rename is unsupported (errno {error})")
    raise OSError(error, "exclusive rename failed")


def _read_file_at(directory_fd: int, leaf: str, limit: int = 1_048_576) -> bytes:
    fd = os.open(leaf, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0), dir_fd=directory_fd)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise OSError(errno.EINVAL, "fixture member is not a bounded regular file")
        chunks: list[bytes] = []
        remaining = limit + 1
        while remaining:
            chunk = os.read(fd, min(65_536, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        content = b"".join(chunks)
        if len(content) > limit:
            raise OSError(errno.EFBIG, "fixture member exceeds the probe limit")
        return content
    finally:
        os.close(fd)


def _write_file_at(directory_fd: int, leaf: str, content: bytes) -> None:
    fd = os.open(
        leaf,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
        0o600,
        dir_fd=directory_fd,
    )
    try:
        view = memoryview(content)
        while view:
            written = os.write(fd, view)
            if written <= 0:
                raise OSError(errno.EIO, "fixture write made no progress")
            view = view[written:]
        os.fsync(fd)
    finally:
        os.close(fd)


def _bundle_members(tag: str) -> dict[str, bytes]:
    return {
        "index.html": f"<!doctype html><title>{tag}</title>\n".encode("utf-8"),
        "style.css": f"body {{ --fixture: '{tag}'; }}\n".encode("utf-8"),
        "app.js": f"document.title = '{tag}';\n".encode("utf-8"),
    }


def _manifest(members: dict[str, bytes]) -> tuple[bytes, dict[str, str]]:
    hashes = {name: hashlib.sha256(content).hexdigest() for name, content in members.items()}
    payload = {
        "schema": 1,
        "members": {
            name: {"bytes": len(members[name]), "sha256": hashes[name]}
            for name in sorted(members)
        },
    }
    return (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode(), hashes


def _create_bundle(parent_fd: int, leaf: str, tag: str) -> dict[str, str]:
    os.mkdir(leaf, 0o700, dir_fd=parent_fd)
    bundle_fd = _open_child_directory(parent_fd, leaf)
    try:
        members = _bundle_members(tag)
        manifest, hashes = _manifest(members)
        for name, content in members.items():
            _write_file_at(bundle_fd, name, content)
        _write_file_at(bundle_fd, "manifest.json", manifest)
        os.fsync(bundle_fd)
        return hashes
    finally:
        os.close(bundle_fd)


def _copy_verified(source_fd: int, parent_fd: int, stage_leaf: str) -> tuple[int, dict[str, str]]:
    manifest_bytes = _read_file_at(source_fd, "manifest.json")
    manifest = json.loads(manifest_bytes)
    if not isinstance(manifest, dict):
        raise OSError(errno.EINVAL, "synthetic manifest is not an object")
    descriptors = manifest.get("members")
    expected_names = {"index.html", "style.css", "app.js"}
    if manifest.get("schema") != 1 or not isinstance(descriptors, dict) or set(descriptors) != expected_names:
        raise OSError(errno.EINVAL, "synthetic manifest shape is invalid")
    os.mkdir(stage_leaf, 0o700, dir_fd=parent_fd)
    stage_fd = _open_child_directory(parent_fd, stage_leaf)
    try:
        hashes: dict[str, str] = {}
        for name in sorted(expected_names):
            content = _read_file_at(source_fd, name)
            digest = hashlib.sha256(content).hexdigest()
            descriptor = descriptors[name]
            if len(content) != descriptor.get("bytes") or digest != descriptor.get("sha256"):
                raise OSError(errno.EILSEQ, "synthetic source member failed verification")
            _write_file_at(stage_fd, name, content)
            copied = _read_file_at(stage_fd, name)
            if copied != content or hashlib.sha256(copied).hexdigest() != digest:
                raise OSError(errno.EILSEQ, "staged member failed verification")
            hashes[name] = digest
        _write_file_at(stage_fd, "manifest.json", manifest_bytes)
        if set(os.listdir(stage_fd)) != expected_names | {"manifest.json"}:
            raise OSError(errno.EINVAL, "staging directory contains unexpected entries")
        os.fsync(stage_fd)
        return stage_fd, hashes
    except BaseException:
        os.close(stage_fd)
        raise


def _stage_and_publish(function: Any, flag: int, source_fd: int, parent_fd: int, target: str) -> dict[str, Any]:
    stage = f".stage-{uuid.uuid4().hex}"
    stage_fd, hashes = _copy_verified(source_fd, parent_fd, stage)
    try:
        _rename_exclusive(function, flag, parent_fd, stage, parent_fd, target)
    finally:
        os.close(stage_fd)
    return {"published": True, "hashes": hashes, "stage": stage}


def _complete_at(parent_fd: int, leaf: str) -> bool:
    try:
        directory_fd = _open_child_directory(parent_fd, leaf)
    except OSError:
        return False
    try:
        manifest_bytes = _read_file_at(directory_fd, "manifest.json")
        manifest = json.loads(manifest_bytes)
        if not isinstance(manifest, dict):
            return False
        descriptors = manifest.get("members")
        names = {"index.html", "style.css", "app.js"}
        if manifest.get("schema") != 1 or not isinstance(descriptors, dict) or set(descriptors) != names:
            return False
        if set(os.listdir(directory_fd)) != names | {"manifest.json"}:
            return False
        for name in names:
            content = _read_file_at(directory_fd, name)
            descriptor = descriptors[name]
            if len(content) != descriptor["bytes"] or hashlib.sha256(content).hexdigest() != descriptor["sha256"]:
                return False
        return True
    except (OSError, ValueError, TypeError, KeyError):
        return False
    finally:
        os.close(directory_fd)


def _occupied_case(function: Any, flag: int, source_fd: int, parent_fd: int, leaf: str, kind: str) -> bool:
    if kind in {"empty-directory", "nonempty-directory"}:
        os.mkdir(leaf, 0o700, dir_fd=parent_fd)
        if kind == "nonempty-directory":
            target_fd = _open_child_directory(parent_fd, leaf)
            try:
                _write_file_at(target_fd, "sentinel", b"occupied-directory")
            finally:
                os.close(target_fd)
    elif kind == "file":
        _write_file_at(parent_fd, leaf, b"occupied-file")
    elif kind == "symlink":
        os.symlink("source", leaf, dir_fd=parent_fd)
    before = _lstat_at(parent_fd, leaf)
    before_identity = (before.st_dev, before.st_ino, before.st_mode)
    try:
        _stage_and_publish(function, flag, source_fd, parent_fd, leaf)
        return False
    except OSError as exc:
        if isinstance(exc, ProbeUnavailable):
            raise
        if exc.errno != errno.EEXIST:
            raise
    after = _lstat_at(parent_fd, leaf)
    unchanged = (after.st_dev, after.st_ino, after.st_mode) == before_identity
    if kind == "file":
        unchanged = unchanged and _read_file_at(parent_fd, leaf) == b"occupied-file"
    elif kind == "nonempty-directory":
        directory_fd = _open_child_directory(parent_fd, leaf)
        try:
            unchanged = unchanged and _read_file_at(directory_fd, "sentinel") == b"occupied-directory"
        finally:
            os.close(directory_fd)
    elif kind == "empty-directory":
        directory_fd = _open_child_directory(parent_fd, leaf)
        try:
            unchanged = unchanged and os.listdir(directory_fd) == []
        finally:
            os.close(directory_fd)
    else:
        unchanged = unchanged and stat.S_ISLNK(after.st_mode) and os.readlink(leaf, dir_fd=parent_fd) == "source"
    return unchanged


def _validate_run(workspace: Path, run_name: str, token: str) -> tuple[int, int, int]:
    if not re.fullmatch(r"run-[0-9a-f]{32}", run_name) or not re.fullmatch(r"[0-9a-f]{32}", token):
        raise ValueError("child fixture token or name is invalid")
    workspace_fd = _open_absolute_directory(workspace)
    try:
        scratch_fd = _open_child_directory(workspace_fd, SCRATCH_NAME)
        try:
            root_fd = _open_child_directory(scratch_fd, FIXTURE_ROOT_NAME)
            try:
                named = _lstat_at(root_fd, run_name)
                if stat.S_ISLNK(named.st_mode) or not stat.S_ISDIR(named.st_mode):
                    raise OSError(errno.ELOOP, "child run fixture is not a real directory")
                run_fd = _open_child_directory(root_fd, run_name)
                run_info = os.fstat(run_fd)
                if (run_info.st_dev, run_info.st_ino) != (named.st_dev, named.st_ino):
                    os.close(run_fd)
                    raise OSError(errno.ESTALE, "child run identity changed")
                token_bytes = _read_file_at(run_fd, TOKEN_NAME, 256)
                if token_bytes != f"{token} {run_info.st_dev} {run_info.st_ino}\n".encode("ascii"):
                    os.close(run_fd)
                    raise PermissionError("child fixture token does not match")
                return root_fd, run_fd, workspace_fd
            except BaseException:
                os.close(root_fd)
                raise
        finally:
            os.close(scratch_fd)
    except BaseException:
        os.close(workspace_fd)
        raise


def _child_main(workspace: Path, run_name: str, token: str, mode: str) -> int:
    root_fd, run_fd, workspace_fd = _validate_run(workspace, run_name, token)
    try:
        parent_name = f"child-{mode}-parent"
        parent_fd = _open_or_create_directory(run_fd, parent_name)
        try:
            source_name = f"child-{mode}-source"
            _create_bundle(run_fd, source_name, f"child-{mode}")
            source_fd = _open_child_directory(run_fd, source_name)
            try:
                function, flag, _ = _exclusive_api()
                stage = f".stage-{uuid.uuid4().hex}"
                stage_fd, _ = _copy_verified(source_fd, parent_fd, stage)
                os.close(source_fd)
                if mode == "before":
                    os._exit(73)
                try:
                    _rename_exclusive(function, flag, parent_fd, stage, parent_fd, "final")
                    os.fsync(parent_fd)
                finally:
                    os.close(stage_fd)
                os._exit(74)
            finally:
                try:
                    os.close(source_fd)
                except OSError:
                    pass
        finally:
            os.close(parent_fd)
    finally:
        os.close(run_fd)
        os.close(root_fd)
        os.close(workspace_fd)
    return EXIT_ERROR


def _child_crash_case(workspace: Path, run_name: str, run_fd: int, mode: str, expected_code: int) -> dict[str, Any]:
    parent_name = f"child-{mode}-parent"
    parent_fd = _open_or_create_directory(run_fd, parent_name)
    os.close(parent_fd)
    token = uuid.uuid4().hex
    info = os.fstat(run_fd)
    token_fd = os.open(
        TOKEN_NAME,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
        0o600,
        dir_fd=run_fd,
    )
    try:
        os.write(token_fd, f"{token} {info.st_dev} {info.st_ino}\n".encode("ascii"))
        os.fsync(token_fd)
    finally:
        os.close(token_fd)
    child = subprocess.run(
        [sys.executable, str(Path(__file__).absolute()), "--workspace", str(workspace), "--child", mode, "--run-name", run_name, "--token", token],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=30,
        check=False,
    )
    os.unlink(TOKEN_NAME, dir_fd=run_fd)
    parent_fd = _open_child_directory(run_fd, parent_name)
    try:
        entries = set(os.listdir(parent_fd))
        final_complete = _complete_at(parent_fd, "final")
        stages = {entry for entry in entries if re.fullmatch(r"\.stage-[0-9a-f]{32}", entry)}
        if mode == "before":
            state_matches = child.returncode == expected_code and len(stages) == 1 and not final_complete
            temporary_complete = len(stages) == 1 and _complete_at(parent_fd, next(iter(stages)))
        else:
            state_matches = child.returncode == expected_code and not stages and final_complete
            temporary_complete = False
        return {
            "status": "passed" if state_matches and (mode != "before" or temporary_complete) else "failed",
            "exit_code": child.returncode,
            "expected_exit_code": expected_code,
            "temporary_count": len(stages),
            "temporary_complete": temporary_complete,
            "final_complete": final_complete,
        }
    finally:
        os.close(parent_fd)


def _linux_filesystem_name(workspace: Path) -> str | None:
    if not sys.platform.startswith("linux"):
        return None
    try:
        candidate = os.path.normpath(str(workspace))
        selected: tuple[int, str] | None = None
        with open("/proc/self/mountinfo", "r", encoding="ascii") as mountinfo:
            for line in mountinfo:
                fields = line.rstrip("\n").split(" - ", 1)
                if len(fields) != 2:
                    continue
                left, right = fields[0].split(), fields[1].split()
                if len(left) < 5 or not right:
                    continue
                mountpoint = re.sub(r"\\([0-7]{3})", lambda match: chr(int(match.group(1), 8)), left[4])
                if candidate == mountpoint or candidate.startswith(mountpoint.rstrip("/") + "/"):
                    if selected is None or len(mountpoint) > selected[0]:
                        selected = (len(mountpoint), right[0])
        return selected[1] if selected else None
    except (OSError, UnicodeError, ValueError):
        return None


def _run_suite(workspace: Path, workspace_fd: int, root_fd: int, run_name: str, run_fd: int) -> dict[str, Any]:
    function, flag, api = _exclusive_api()
    results: dict[str, Any] = {
        "schema": 1,
        "platform": sys.platform,
        "os_release": platform.release(),
        "python": platform.python_version(),
        "api_mode": api,
        "filesystem": _linux_filesystem_name(workspace),
        "tests": {},
        "failed": [],
        "unavailable": [],
        "incomplete": [],
        "source_identity_bound": False,
        "production_ready": False,
        "limits": [
            "synthetic fixtures only; no project files or Vivac store",
            "exclusive rename binds destination absence but names the source by leaf",
            "same-UID hostile mutation of the disposable fixture tree is outside this probe's cleanup model",
            "no power-loss durability claim",
        ],
    }
    source_name = "source-main"
    expected_hashes = _create_bundle(run_fd, source_name, "primary")
    source_fd = _open_child_directory(run_fd, source_name)
    try:
        parent_name = "publish-parent"
        parent_fd = _open_or_create_directory(run_fd, parent_name)
        try:
            published = _stage_and_publish(function, flag, source_fd, parent_fd, "published")
            results["tests"]["three_members_and_manifest"] = {
                "status": "passed" if published["hashes"] == expected_hashes and _complete_at(parent_fd, "published") else "failed"
            }
        finally:
            os.close(parent_fd)

        occupied_fd = _open_or_create_directory(run_fd, "occupied-parent")
        try:
            occupied_results = {
                kind: _occupied_case(function, flag, source_fd, occupied_fd, leaf, kind)
                for kind, leaf in (
                    ("empty-directory", "empty"),
                    ("nonempty-directory", "nonempty"),
                    ("file", "file"),
                    ("symlink", "symlink"),
                )
            }
            results["tests"]["occupied_destinations_unchanged"] = {
                "status": "passed" if all(occupied_results.values()) else "failed",
                "cases": occupied_results,
            }
        finally:
            os.close(occupied_fd)

        contention_fd = _open_or_create_directory(run_fd, "contention-parent")
        outcomes: list[dict[str, Any]] = []
        thread_errors: list[str] = []
        barrier = threading.Barrier(2)
        outcomes_lock = threading.Lock()
        def contend(tag: str) -> None:
            local_source_fd = local_parent_fd = -1
            try:
                local_source_name = f"source-{tag}"
                _create_bundle(run_fd, local_source_name, tag)
                local_source_fd = _open_child_directory(run_fd, local_source_name)
                local_parent_fd = os.dup(contention_fd)
                barrier.wait(timeout=15)
                try:
                    _stage_and_publish(function, flag, local_source_fd, local_parent_fd, "winner")
                    outcome = {"published": True}
                except OSError as exc:
                    outcome = {"published": False, "errno": exc.errno}
                with outcomes_lock:
                    outcomes.append(outcome)
            except ProbeUnavailable:
                with outcomes_lock:
                    thread_errors.append("exclusive rename unsupported")
            except BaseException as exc:
                with outcomes_lock:
                    thread_errors.append(type(exc).__name__)
            finally:
                for fd in (local_source_fd, local_parent_fd):
                    if fd >= 0:
                        os.close(fd)
        threads = [threading.Thread(target=contend, args=(tag,)) for tag in ("left", "right")]
        try:
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(timeout=30)
            if any(thread.is_alive() for thread in threads):
                raise TimeoutError("publisher competition did not finish")
            if thread_errors:
                if "exclusive rename unsupported" in thread_errors:
                    raise ProbeUnavailable("exclusive rename is unsupported during publisher competition")
                raise RuntimeError("publisher competition worker failed")
        finally:
            os.close(contention_fd)
        exactly_one = len(outcomes) == 2 and sum(bool(item.get("published")) for item in outcomes) == 1
        loser_is_exclusive_collision = (
            exactly_one
            and next(item for item in outcomes if not item.get("published")).get("errno") == errno.EEXIST
        )
        winner_fd = _open_child_directory(run_fd, "contention-parent")
        try:
            winner_complete = _complete_at(winner_fd, "winner")
        finally:
            os.close(winner_fd)
        results["tests"]["two_publishers_one_complete_winner"] = {
            "status": "passed" if exactly_one and loser_is_exclusive_collision and winner_complete else "failed",
            "published_count": sum(bool(item.get("published")) for item in outcomes),
            "loser_received_eexist": loser_is_exclusive_collision,
            "winner_complete": winner_complete,
        }

        old_parent_name = "parent-before-move"
        moved_parent_name = "parent-after-move"
        old_parent_fd = _open_or_create_directory(run_fd, old_parent_name)
        try:
            _write_file_at(old_parent_fd, "sentinel", b"original-parent")
            os.rename(old_parent_name, moved_parent_name, src_dir_fd=run_fd, dst_dir_fd=run_fd)
            recreated_fd = _open_or_create_directory(run_fd, old_parent_name)
            try:
                _write_file_at(recreated_fd, "sentinel", b"recreated-parent")
            finally:
                os.close(recreated_fd)
            _stage_and_publish(function, flag, source_fd, old_parent_fd, "via-held-parent")
            recreated_fd = _open_child_directory(run_fd, old_parent_name)
            try:
                sentinel_unchanged = _read_file_at(recreated_fd, "sentinel") == b"recreated-parent"
                try:
                    _lstat_at(recreated_fd, "via-held-parent")
                    recreated_has_no_publication = False
                except FileNotFoundError:
                    recreated_has_no_publication = True
            finally:
                os.close(recreated_fd)
            moved_fd = _open_child_directory(run_fd, moved_parent_name)
            try:
                published_in_moved = _complete_at(moved_fd, "via-held-parent")
            finally:
                os.close(moved_fd)
            results["tests"]["moved_parent_handle_and_recreated_old_name"] = {
                "status": "passed" if published_in_moved and sentinel_unchanged and recreated_has_no_publication else "failed",
                "published_in_original_directory": published_in_moved,
                "recreated_parent_sentinel_unchanged": sentinel_unchanged,
                "recreated_parent_has_no_publication": recreated_has_no_publication,
            }
        finally:
            os.close(old_parent_fd)

        os.symlink(source_name, "source-symlink", dir_fd=run_fd)
        try:
            linked_fd = _open_child_directory(run_fd, "source-symlink")
        except OSError as exc:
            rejected = exc.errno in {errno.ELOOP, errno.ENOTDIR}
        else:
            rejected = False
            os.close(linked_fd)
        results["tests"]["symlink_source_refused"] = {"status": "passed" if rejected else "failed"}

        race_parent_fd = _open_or_create_directory(run_fd, "stage-name-race-parent")
        stage_name = f".stage-{uuid.uuid4().hex}"
        pinned_fd, pinned_hashes = _copy_verified(source_fd, race_parent_fd, stage_name)
        try:
            pinned_info = os.fstat(pinned_fd)
            verified_name = stage_name + "-verified"
            os.rename(stage_name, verified_name, src_dir_fd=race_parent_fd, dst_dir_fd=race_parent_fd)
            replacement_hashes = _create_bundle(race_parent_fd, "replacement", "replacement")
            os.rename("replacement", stage_name, src_dir_fd=race_parent_fd, dst_dir_fd=race_parent_fd)
            replacement_info = _lstat_at(race_parent_fd, stage_name)
            identity_differs = (pinned_info.st_dev, pinned_info.st_ino) != (replacement_info.st_dev, replacement_info.st_ino)
            _rename_exclusive(function, flag, race_parent_fd, stage_name, race_parent_fd, "published-replacement")
            published_fd = _open_child_directory(race_parent_fd, "published-replacement")
            try:
                published_manifest = json.loads(_read_file_at(published_fd, "manifest.json"))
            finally:
                os.close(published_fd)
            published_hashes = {name: member["sha256"] for name, member in published_manifest["members"].items()}
            replacement_published = (
                _complete_at(race_parent_fd, "published-replacement")
                and published_hashes == replacement_hashes
                and published_hashes != pinned_hashes
            )
            expected_observation = identity_differs and replacement_published
            results["tests"]["staging_name_swap_counterexample"] = {
                "status": "passed" if expected_observation else "failed",
                "observed_expected_counterexample": expected_observation,
                "pinned_inode_differs_from_named_stage": identity_differs,
                "replacement_published": replacement_published,
            }
        finally:
            os.close(pinned_fd)
            os.close(race_parent_fd)

        child_results = {}
        for mode, code in (("before", 73), ("after", 74)):
            child_results[mode] = _child_crash_case(workspace, run_name, run_fd, mode, code)
        results["tests"]["child_exit_before_after_publication"] = {
            "status": "passed" if all(value["status"] == "passed" for value in child_results.values()) else "failed",
            "cases": child_results,
        }
    finally:
        os.close(source_fd)

    results["failed"] = [name for name, value in results["tests"].items() if value.get("status") == "failed"]
    return results


def _cleanup_run(root_fd: int, run_name: str, run_fd: int, identity: tuple[int, int]) -> None:
    named = _lstat_at(root_fd, run_name)
    held = os.fstat(run_fd)
    if stat.S_ISLNK(named.st_mode) or not stat.S_ISDIR(named.st_mode):
        raise OSError(errno.ELOOP, "refusing cleanup: run entry is not a real directory")
    if (named.st_dev, named.st_ino) != identity or (held.st_dev, held.st_ino) != identity:
        raise OSError(errno.ESTALE, "refusing cleanup: run directory identity changed")
    if not shutil.rmtree.avoids_symlink_attacks or "dir_fd" not in __import__("inspect").signature(shutil.rmtree).parameters:
        raise ProbeUnavailable("safe descriptor-relative recursive cleanup is unavailable")
    shutil.rmtree(run_name, dir_fd=root_fd)


def _error_payload(exc: BaseException) -> dict[str, Any]:
    item: dict[str, Any] = {"type": type(exc).__name__}
    if isinstance(exc, OSError):
        item["errno"] = exc.errno
        item["errno_name"] = errno.errorcode.get(exc.errno, "UNKNOWN") if exc.errno is not None else None
    return {"error": item}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--child", choices=("before", "after"))
    parser.add_argument("--run-name")
    parser.add_argument("--token")
    args = parser.parse_args()
    if not (sys.platform.startswith("linux") or sys.platform == "darwin"):
        return _emit(
            {
                "schema": 1,
                "platform": sys.platform,
                "os_release": platform.release(),
                "python": platform.python_version(),
                "api_mode": None,
                "filesystem": None,
                "failed": [],
                "unavailable": ["platform is unsupported"],
                "incomplete": [],
                "source_identity_bound": False,
                "production_ready": False,
            },
            EXIT_INCOMPLETE,
        )
    if args.child:
        if args.run_name is None or args.token is None:
            return _emit(
                {
                    "schema": 1,
                    "platform": sys.platform,
                    "os_release": platform.release(),
                    "python": platform.python_version(),
                    "api_mode": None,
                    "filesystem": None,
                    "failed": [],
                    "unavailable": [],
                    "incomplete": ["child token and run name are required"],
                    "source_identity_bound": False,
                    "production_ready": False,
                },
                EXIT_INCOMPLETE,
            )
        try:
            return _child_main(args.workspace, args.run_name, args.token, args.child)
        except ProbeUnavailable:
            return EXIT_INCOMPLETE
        except BaseException:
            return EXIT_ERROR

    workspace_fd = root_fd = run_fd = scratch_fd = -1
    run_name = f"{RUN_PREFIX}{uuid.uuid4().hex}"
    identity: tuple[int, int] | None = None
    result: dict[str, Any] | None = None
    exit_code = EXIT_ERROR
    try:
        workspace_fd = _open_absolute_directory(args.workspace)
        scratch_fd = _open_or_create_directory(workspace_fd, SCRATCH_NAME)
        root_fd = _open_or_create_directory(scratch_fd, FIXTURE_ROOT_NAME)
        os.mkdir(run_name, 0o700, dir_fd=root_fd)
        run_fd = _open_child_directory(root_fd, run_name)
        held = os.fstat(run_fd)
        named = _lstat_at(root_fd, run_name)
        identity = (held.st_dev, held.st_ino)
        if stat.S_ISLNK(named.st_mode) or not stat.S_ISDIR(named.st_mode) or identity != (named.st_dev, named.st_ino):
            raise OSError(errno.ESTALE, "new run fixture identity changed")
        result = _run_suite(args.workspace, workspace_fd, root_fd, run_name, run_fd)
        exit_code = EXIT_FAILED if result["failed"] else EXIT_OK
    except ProbeUnavailable as exc:
        result = {
            "schema": 1,
            "platform": sys.platform,
            "os_release": platform.release(),
            "python": platform.python_version(),
            "api_mode": None,
            "filesystem": _linux_filesystem_name(args.workspace),
            "failed": [],
            "unavailable": [str(exc).split(":")[0]],
            "incomplete": [],
            "source_identity_bound": False,
            "production_ready": False,
        }
        exit_code = EXIT_INCOMPLETE
    except BaseException as exc:
        result = {
            "schema": 1,
            "platform": sys.platform,
            "os_release": platform.release(),
            "python": platform.python_version(),
            "api_mode": None,
            "filesystem": _linux_filesystem_name(args.workspace),
            "source_identity_bound": False,
            "production_ready": False,
            **_error_payload(exc),
        }
        exit_code = EXIT_ERROR
    finally:
        if run_fd >= 0:
            try:
                if root_fd < 0 or identity is None:
                    raise OSError(errno.ESTALE, "run fixture identity was not established")
                _cleanup_run(root_fd, run_name, run_fd, identity)
                if result is not None:
                    result["cleanup"] = "removed-owned-fixture"
            except ProbeUnavailable as exc:
                exit_code = EXIT_INCOMPLETE
                if result is None:
                    result = {"failed": [], "unavailable": [], "incomplete": []}
                result.setdefault("incomplete", []).append(str(exc).split(":")[0])
            except BaseException as exc:
                exit_code = EXIT_ERROR
                if result is None:
                    result = {
                        "schema": 1,
                        "platform": sys.platform,
                        "os_release": platform.release(),
                        "python": platform.python_version(),
                        "api_mode": None,
                        "filesystem": _linux_filesystem_name(args.workspace),
                        "source_identity_bound": False,
                        "production_ready": False,
                        **_error_payload(exc),
                    }
                else:
                    result["cleanup"] = "refused"
                    result["error"] = _error_payload(exc)["error"]
                    result.setdefault("incomplete", []).append("fixture cleanup refused after identity check")
            os.close(run_fd)
        for fd in (root_fd, scratch_fd, workspace_fd):
            if fd >= 0:
                os.close(fd)
    if result is None:
        result = {
            "schema": 1,
            "platform": sys.platform,
            "os_release": platform.release(),
            "python": platform.python_version(),
            "api_mode": None,
            "filesystem": None,
            "source_identity_bound": False,
            "production_ready": False,
            "failed": [],
            "unavailable": [],
            "incomplete": ["probe did not produce a result"],
        }
        exit_code = EXIT_INCOMPLETE
    result.setdefault("failed", [])
    result.setdefault("unavailable", [])
    result.setdefault("incomplete", [])
    if result.get("error"):
        exit_code = EXIT_ERROR
    elif result["failed"]:
        exit_code = EXIT_FAILED
    elif result["unavailable"] or result["incomplete"]:
        exit_code = EXIT_INCOMPLETE
    return _emit(result, exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
