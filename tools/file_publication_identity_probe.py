"""Probe exclusive regular-file publication from a held source descriptor."""

from __future__ import annotations

import ctypes
import errno
import hashlib
import json
import os
import platform
import stat
import sys
import uuid
from pathlib import Path
from typing import Any

import directory_publication_probe as directory_probe


SCRATCH_NAME = ".scratch"
FIXTURE_ROOT_NAME = "file-publication-probe"
RUN_PREFIX = "run-"
LINUX_AT_FDCWD = -100
LINUX_AT_SYMLINK_FOLLOW = 0x400
MACOS_CLONE_NOFOLLOW = 0x0001
MACOS_CLONE_NOOWNERCOPY = 0x0002
EXIT_OK = 0
EXIT_FAILED = 1
EXIT_ERROR = 2
EXIT_INCOMPLETE = 3

REQUIRED_TESTS = (
    "normal_publish_old_sha",
    "source_name_swap_uses_held_fd",
    "occupied_file_empty_dir_nonempty_dir_symlink_preserved",
    "dangling_symlink_referent_not_created",
    "moved_parent_fd_receives_publish",
    "platform_source_identity_observation",
)


def _identity(info: os.stat_result) -> tuple[int, int]:
    return info.st_dev, info.st_ino


def _canonical_container(tag: str) -> tuple[bytes, str]:
    members = {
        "index.html": f"<!doctype html><title>{tag}</title>\n",
        "style.css": f"body{{--fixture:'{tag}';}}\n",
        "app.js": f"document.title='{tag}';\n",
    }
    descriptors = {
        name: {
            "bytes": len(content.encode("utf-8")),
            "sha256": hashlib.sha256(content.encode("utf-8")).hexdigest(),
        }
        for name, content in sorted(members.items())
    }
    payload = {
        "schema": 1,
        "members": {
            name: {"content": members[name], **descriptors[name]}
            for name in sorted(members)
        },
    }
    encoded = (json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    return encoded, hashlib.sha256(encoded).hexdigest()


def _write_container(parent_fd: int, leaf: str, tag: str) -> str:
    content, digest = _canonical_container(tag)
    directory_probe._write_file_at(parent_fd, leaf, content)
    return digest


def _open_file_at(parent_fd: int, leaf: str, flags: int = os.O_RDONLY) -> int:
    return os.open(
        leaf,
        flags | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
        dir_fd=parent_fd,
    )


def _container_digest(parent_fd: int, leaf: str) -> str:
    content = directory_probe._read_file_at(parent_fd, leaf)
    payload = json.loads(content.decode("utf-8"))
    if not isinstance(payload, dict) or payload.get("schema") != 1:
        raise OSError(errno.EINVAL, "synthetic container schema is invalid")
    members = payload.get("members")
    expected = {"index.html", "style.css", "app.js"}
    if not isinstance(members, dict) or set(members) != expected:
        raise OSError(errno.EINVAL, "synthetic container member set is invalid")
    for name, descriptor in members.items():
        if not isinstance(descriptor, dict) or not isinstance(descriptor.get("content"), str):
            raise OSError(errno.EINVAL, "synthetic container member is invalid")
        raw = descriptor["content"].encode("utf-8")
        if len(raw) != descriptor.get("bytes") or hashlib.sha256(raw).hexdigest() != descriptor.get("sha256"):
            raise OSError(errno.EILSEQ, "synthetic container member hash is invalid")
    return hashlib.sha256(content).hexdigest()


def _linux_linkat() -> tuple[Any, int]:
    if not sys.platform.startswith("linux"):
        raise directory_probe.ProbeUnavailable("Linux linkat is unavailable")
    try:
        function = getattr(ctypes.CDLL(None, use_errno=True), "linkat")
    except (AttributeError, OSError) as exc:
        raise directory_probe.ProbeUnavailable("Linux linkat symbol unavailable") from exc
    function.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
    function.restype = ctypes.c_int
    return function, LINUX_AT_SYMLINK_FOLLOW


def _macos_fclonefileat() -> tuple[Any, int]:
    if sys.platform != "darwin":
        raise directory_probe.ProbeUnavailable("macOS fclonefileat is unavailable")
    try:
        function = getattr(ctypes.CDLL(None, use_errno=True), "fclonefileat")
    except (AttributeError, OSError) as exc:
        raise directory_probe.ProbeUnavailable("macOS fclonefileat symbol unavailable") from exc
    function.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    function.restype = ctypes.c_int
    return function, MACOS_CLONE_NOOWNERCOPY | MACOS_CLONE_NOFOLLOW


def _api_observation() -> dict[str, Any]:
    if sys.platform.startswith("linux"):
        return {
            "api_mode": "linux-linkat-procfd",
            "api_options": {
                "source_locator": "proc-self-fd",
                "flags": LINUX_AT_SYMLINK_FOLLOW,
                "flag_names": ["AT_SYMLINK_FOLLOW"],
            },
        }
    if sys.platform == "darwin":
        return {
            "api_mode": "macos-fclonefileat",
            "api_options": {
                "source_locator": "held-fd",
                "flags": MACOS_CLONE_NOOWNERCOPY | MACOS_CLONE_NOFOLLOW,
                "flag_names": ["CLONE_NOOWNERCOPY", "CLONE_NOFOLLOW"],
            },
        }
    return {"api_mode": None, "api_options": None}


def _check_native_error(error: int) -> None:
    unsupported = {
        errno.ENOSYS,
        getattr(errno, "ENOTSUP", errno.EOPNOTSUPP),
        getattr(errno, "EOPNOTSUPP", errno.ENOTSUP),
        errno.EXDEV,
    }
    if error in unsupported:
        raise directory_probe.ProbeUnavailable(f"native API or filesystem unsupported (errno {error})")
    raise OSError(error, "native publication API failed")


def _publish(source_fd: int, parent_fd: int, leaf: str, api: str) -> None:
    if not leaf or leaf in {".", ".."} or "/" in leaf or "\\" in leaf:
        raise ValueError("destination must be one safe entry name")
    if api == "linux-linkat-procfd":
        function, flag = _linux_linkat()
        oldpath = f"/proc/self/fd/{source_fd}"
        result = function(
            LINUX_AT_FDCWD,
            os.fsencode(oldpath),
            parent_fd,
            os.fsencode(leaf),
            flag,
        )
    elif api == "macos-fclonefileat":
        function, flags = _macos_fclonefileat()
        result = function(source_fd, parent_fd, os.fsencode(leaf), flags)
    else:
        raise ValueError("unknown publication API")
    if result == 0:
        return
    error = ctypes.get_errno()
    _check_native_error(error)


def _require_proc_fd_identity(source_fd: int) -> None:
    alias = f"/proc/self/fd/{source_fd}"
    try:
        alias_info = os.stat(alias)
    except OSError as exc:
        raise directory_probe.ProbeUnavailable("/proc/self/fd unavailable or inaccessible") from exc
    held_info = os.fstat(source_fd)
    if not stat.S_ISREG(alias_info.st_mode) or _identity(alias_info) != _identity(held_info):
        raise directory_probe.ProbeUnavailable("/proc/self/fd did not resolve to the held regular file")


def _entry_snapshot(parent_fd: int, leaf: str) -> tuple[Any, ...] | None:
    try:
        info = directory_probe._lstat_at(parent_fd, leaf)
    except FileNotFoundError:
        return None
    kind = stat.S_IFMT(info.st_mode)
    detail: Any = None
    if stat.S_ISREG(info.st_mode):
        detail = hashlib.sha256(directory_probe._read_file_at(parent_fd, leaf)).hexdigest()
    elif stat.S_ISLNK(info.st_mode):
        detail = os.readlink(leaf, dir_fd=parent_fd)
    return (info.st_dev, info.st_ino, kind, detail)


def _occupied_case(source_fd: int, parent_fd: int, leaf: str, kind: str, api: str) -> dict[str, Any]:
    target_leaf: str | None = None
    if kind in {"empty-directory", "nonempty-directory"}:
        os.mkdir(leaf, 0o700, dir_fd=parent_fd)
        if kind == "nonempty-directory":
            child_fd = directory_probe._open_child_directory(parent_fd, leaf)
            try:
                directory_probe._write_file_at(child_fd, "sentinel", b"occupied-directory")
            finally:
                os.close(child_fd)
    elif kind == "file":
        directory_probe._write_file_at(parent_fd, leaf, b"occupied-file")
    elif kind in {"symlink", "dangling-symlink"}:
        target_leaf = "held-source" if kind == "symlink" else "dangling-target"
        if kind == "symlink":
            directory_probe._write_file_at(parent_fd, target_leaf, b"occupied-symlink-referent")
        os.symlink(target_leaf, leaf, dir_fd=parent_fd)
    before = directory_probe._lstat_at(parent_fd, leaf)
    before_identity = (before.st_dev, before.st_ino, before.st_mode)
    before_snapshot = _entry_snapshot(parent_fd, leaf)
    target_before = _entry_snapshot(parent_fd, target_leaf) if target_leaf is not None else None
    api_returned_success = False
    api_error: int | None = None
    try:
        _publish(source_fd, parent_fd, leaf, api)
        api_returned_success = True
    except OSError as exc:
        api_error = exc.errno
    after = directory_probe._lstat_at(parent_fd, leaf)
    after_snapshot = _entry_snapshot(parent_fd, leaf)
    destination_unchanged = (
        (after.st_dev, after.st_ino, after.st_mode) == before_identity
        and after_snapshot == before_snapshot
    )
    target_after = _entry_snapshot(parent_fd, target_leaf) if target_leaf is not None else None
    target_unchanged = target_after == target_before if target_leaf is not None else None
    if kind == "file":
        destination_unchanged = destination_unchanged and directory_probe._read_file_at(parent_fd, leaf) == b"occupied-file"
    elif kind == "empty-directory":
        child_fd = directory_probe._open_child_directory(parent_fd, leaf)
        try:
            destination_unchanged = destination_unchanged and os.listdir(child_fd) == []
        finally:
            os.close(child_fd)
    elif kind == "nonempty-directory":
        child_fd = directory_probe._open_child_directory(parent_fd, leaf)
        try:
            destination_unchanged = destination_unchanged and directory_probe._read_file_at(child_fd, "sentinel") == b"occupied-directory"
        finally:
            os.close(child_fd)
    elif target_leaf is not None:
        destination_unchanged = destination_unchanged and stat.S_ISLNK(after.st_mode) and os.readlink(leaf, dir_fd=parent_fd) == target_leaf
    passed = not api_returned_success and api_error == errno.EEXIST and destination_unchanged and target_unchanged is not False
    return {
        "status": "passed" if passed else "failed",
        "api_returned_success": api_returned_success,
        "errno": api_error,
        "errno_name": errno.errorcode.get(api_error) if api_error is not None else None,
        "destination_unchanged": destination_unchanged,
        "target_existed_before": target_before is not None if target_leaf is not None else None,
        "target_unchanged": target_unchanged,
        "target_created": target_before is None and target_after is not None if target_leaf is not None else None,
    }


def _run_suite(workspace: Path, root_fd: int, run_fd: int) -> dict[str, Any]:
    observation = _api_observation()
    api = observation["api_mode"]
    payload: dict[str, Any] = {
        "schema": 1,
        "platform": sys.platform,
        "os_release": platform.release(),
        **observation,
        "filesystem": directory_probe._linux_filesystem_name(workspace),
        "tests": {name: {"status": "not_run"} for name in REQUIRED_TESTS},
        "failed": [],
        "unavailable": [],
        "incomplete": [],
        "source_selection_bound_to_fd": False,
        "production_ready": False,
        "limits": [
            "synthetic one-file JSON container only; no project format or files",
            "Linux publication uses procfs linkat only; no AT_EMPTY_PATH or pathname fallback",
            "macOS publication uses fclonefileat only; no copy fallback",
            "source descriptor pins object identity but does not establish a concurrent-byte snapshot",
            "no directory-restore, durability, MSRV, performance, or production-readiness claim",
        ],
    }
    source_leaf = "source-container.json"
    expected_digest = _write_container(run_fd, source_leaf, "original")
    source_fd = _open_file_at(run_fd, source_leaf)
    try:
        source_info = os.fstat(source_fd)
        if not stat.S_ISREG(source_info.st_mode):
            raise OSError(errno.EINVAL, "synthetic source is not a regular file")
        if api == "linux-linkat-procfd":
            _require_proc_fd_identity(source_fd)
        _linux_linkat() if api == "linux-linkat-procfd" else _macos_fclonefileat()

        parent_fd = directory_probe._open_or_create_directory(run_fd, "publish-parent")
        try:
            _publish(source_fd, parent_fd, "published.json", api)
            digest = _container_digest(parent_fd, "published.json")
            published_info = directory_probe._lstat_at(parent_fd, "published.json")
            same_identity = _identity(published_info) == _identity(source_info)
            expected_identity = same_identity if api == "linux-linkat-procfd" else not same_identity
            payload["tests"]["normal_publish_old_sha"] = {
                "status": "passed" if digest == expected_digest and expected_identity else "failed",
                "old_sha_matches": digest == expected_digest,
                "source_and_destination_same_inode": same_identity,
            }
        finally:
            os.close(parent_fd)

        old_leaf = "source-container-original.json"
        os.rename(source_leaf, old_leaf, src_dir_fd=run_fd, dst_dir_fd=run_fd)
        replacement_digest = _write_container(run_fd, source_leaf, "replacement")
        if replacement_digest == expected_digest:
            raise OSError(errno.EILSEQ, "synthetic replacement unexpectedly matches original")
        swap_parent_fd = directory_probe._open_or_create_directory(run_fd, "swap-parent")
        clone_bytes_before_mutation: bytes | None = None
        try:
            _publish(source_fd, swap_parent_fd, "published.json", api)
            digest = _container_digest(swap_parent_fd, "published.json")
            published_info = directory_probe._lstat_at(swap_parent_fd, "published.json")
            same_identity = _identity(published_info) == _identity(source_info)
            expected_identity = same_identity if api == "linux-linkat-procfd" else not same_identity
            payload["tests"]["source_name_swap_uses_held_fd"] = {
                "status": "passed" if digest == expected_digest and expected_identity else "failed",
                "old_sha_published": digest == expected_digest,
                "replacement_sha_published": digest == replacement_digest,
                "source_and_destination_same_inode": same_identity,
            }
            if api == "macos-fclonefileat":
                clone_bytes_before_mutation = directory_probe._read_file_at(swap_parent_fd, "published.json")
            else:
                payload["tests"]["platform_source_identity_observation"] = {
                    "status": "passed" if same_identity else "failed",
                    "published_hardlink_same_inode_as_source": same_identity,
                    "source_mutation_independence": "not_applicable_hardlink",
                }
        finally:
            os.close(swap_parent_fd)

        occupied_fd = directory_probe._open_or_create_directory(run_fd, "occupied-parent")
        try:
            occupied = {
                kind: _occupied_case(source_fd, occupied_fd, leaf, kind, api)
                for kind, leaf in (
                    ("file", "occupied-file"),
                    ("empty-directory", "occupied-empty-directory"),
                    ("nonempty-directory", "occupied-nonempty-directory"),
                    ("symlink", "occupied-symlink"),
                )
            }
            payload["tests"]["occupied_file_empty_dir_nonempty_dir_symlink_preserved"] = {
                "status": "passed" if all(case["status"] == "passed" for case in occupied.values()) else "failed",
                "cases": occupied,
            }
            dangling = _occupied_case(source_fd, occupied_fd, "dangling-link", "dangling-symlink", api)
            payload["tests"]["dangling_symlink_referent_not_created"] = dangling
        finally:
            os.close(occupied_fd)

        parent_name = "parent-before-move"
        moved_name = "parent-after-move"
        held_parent_fd = directory_probe._open_or_create_directory(run_fd, parent_name)
        try:
            directory_probe._write_file_at(held_parent_fd, "sentinel", b"original-parent")
            os.rename(parent_name, moved_name, src_dir_fd=run_fd, dst_dir_fd=run_fd)
            recreated_fd = directory_probe._open_or_create_directory(run_fd, parent_name)
            try:
                directory_probe._write_file_at(recreated_fd, "sentinel", b"recreated-parent")
            finally:
                os.close(recreated_fd)
            _publish(source_fd, held_parent_fd, "published.json", api)
            in_held_parent = _container_digest(held_parent_fd, "published.json") == expected_digest
            recreated_fd = directory_probe._open_child_directory(run_fd, parent_name)
            try:
                recreated_unchanged = directory_probe._read_file_at(recreated_fd, "sentinel") == b"recreated-parent"
                try:
                    directory_probe._lstat_at(recreated_fd, "published.json")
                    absent_at_recreated = False
                except FileNotFoundError:
                    absent_at_recreated = True
            finally:
                os.close(recreated_fd)
            payload["tests"]["moved_parent_fd_receives_publish"] = {
                "status": "passed" if in_held_parent and recreated_unchanged and absent_at_recreated else "failed",
                "publication_in_held_parent": in_held_parent,
                "recreated_parent_sentinel_unchanged": recreated_unchanged,
                "recreated_parent_has_no_publication": absent_at_recreated,
            }
        finally:
            os.close(held_parent_fd)

        if api == "macos-fclonefileat":
            if clone_bytes_before_mutation is None:
                raise OSError(errno.EINVAL, "clone bytes were not captured before source mutation")
            mutation_fd = _open_file_at(run_fd, old_leaf, os.O_RDWR)
            try:
                if os.pwrite(mutation_fd, b"#", 0) != 1:
                    raise OSError(errno.EIO, "source mutation did not complete")
                os.fsync(mutation_fd)
            finally:
                os.close(mutation_fd)
            swap_parent_fd = directory_probe._open_child_directory(run_fd, "swap-parent")
            try:
                clone_unchanged = directory_probe._read_file_at(swap_parent_fd, "published.json") == clone_bytes_before_mutation
                clone_info = directory_probe._lstat_at(swap_parent_fd, "published.json")
                clone_independent = _identity(clone_info) != _identity(source_info)
            finally:
                os.close(swap_parent_fd)
            payload["tests"]["platform_source_identity_observation"] = {
                "status": "passed" if clone_unchanged and clone_independent else "failed",
                "clone_unchanged_after_source_mutation": clone_unchanged,
                "clone_has_independent_inode": clone_independent,
            }
    finally:
        os.close(source_fd)

    payload["failed"] = [name for name, value in payload["tests"].items() if value["status"] == "failed"]
    not_run = [name for name, value in payload["tests"].items() if value["status"] != "passed"]
    if not_run:
        payload["incomplete"] = [f"required test not passed: {name}" for name in not_run]
    payload["source_selection_bound_to_fd"] = not not_run and not payload["failed"]
    return payload


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", required=True, type=Path)
    args = parser.parse_args()
    common: dict[str, Any] = {
        "schema": 1,
        "platform": sys.platform,
        "os_release": platform.release(),
        **_api_observation(),
        "filesystem": directory_probe._linux_filesystem_name(args.workspace),
        "tests": {name: {"status": "not_run"} for name in REQUIRED_TESTS},
        "failed": [],
        "unavailable": [],
        "incomplete": [],
        "source_selection_bound_to_fd": False,
        "production_ready": False,
    }
    if not (sys.platform.startswith("linux") or sys.platform == "darwin"):
        common["unavailable"] = ["platform unsupported; no fallback is permitted"]
        common["incomplete"] = [f"required test not run: {name}" for name in REQUIRED_TESTS]
        return directory_probe._emit(common, EXIT_INCOMPLETE)

    workspace_fd = scratch_fd = root_fd = run_fd = -1
    run_name = f"{RUN_PREFIX}{uuid.uuid4().hex}"
    run_identity: tuple[int, int] | None = None
    result: dict[str, Any] = common
    exit_code = EXIT_ERROR
    try:
        workspace_fd = directory_probe._open_absolute_directory(args.workspace)
        scratch_fd = directory_probe._open_or_create_directory(workspace_fd, SCRATCH_NAME)
        root_fd = directory_probe._open_or_create_directory(scratch_fd, FIXTURE_ROOT_NAME)
        os.mkdir(run_name, 0o700, dir_fd=root_fd)
        run_fd = directory_probe._open_child_directory(root_fd, run_name)
        held = os.fstat(run_fd)
        named = directory_probe._lstat_at(root_fd, run_name)
        run_identity = _identity(held)
        if not stat.S_ISDIR(named.st_mode) or _identity(named) != run_identity:
            raise OSError(errno.ESTALE, "new fixture identity changed")
        result = _run_suite(args.workspace, root_fd, run_fd)
        exit_code = EXIT_FAILED if result["failed"] else EXIT_OK
    except directory_probe.ProbeUnavailable as exc:
        result = common
        result["unavailable"] = [type(exc).__name__]
        result["incomplete"] = [f"required test not run: {name}" for name in REQUIRED_TESTS]
        result.update(_api_observation())
        exit_code = EXIT_INCOMPLETE
    except BaseException as exc:
        result = common
        result["error"] = directory_probe._error_payload(exc)["error"]
        result.update(_api_observation())
        result["incomplete"] = [f"required test status unknown: {name}" for name in REQUIRED_TESTS]
        exit_code = EXIT_ERROR
    finally:
        if run_fd >= 0:
            try:
                if root_fd < 0 or run_identity is None:
                    raise OSError(errno.ESTALE, "fixture identity unavailable for cleanup")
                directory_probe._cleanup_run(root_fd, run_name, run_fd, run_identity)
                result["cleanup"] = "removed-owned-fixture"
            except directory_probe.ProbeUnavailable:
                result.setdefault("incomplete", []).append("safe fixture cleanup unavailable")
                exit_code = EXIT_INCOMPLETE
            except BaseException as exc:
                result["cleanup"] = "refused"
                result["cleanup_error"] = directory_probe._error_payload(exc)["error"]
                result.setdefault("incomplete", []).append("fixture cleanup refused after identity check")
                exit_code = EXIT_ERROR
            os.close(run_fd)
        for fd in (root_fd, scratch_fd, workspace_fd):
            if fd >= 0:
                os.close(fd)
    result["production_ready"] = False
    if result.get("error") or result.get("cleanup_error"):
        exit_code = EXIT_ERROR
    elif result["failed"]:
        exit_code = EXIT_FAILED
    elif result["unavailable"] or result["incomplete"]:
        exit_code = EXIT_INCOMPLETE
    else:
        exit_code = EXIT_OK
    return directory_probe._emit(result, exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
