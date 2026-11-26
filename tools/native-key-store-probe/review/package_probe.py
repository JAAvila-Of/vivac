#!/usr/bin/env python3
"""Disposable package publication and native-binding observations."""

import argparse
import base64
import errno
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import time

import openssh_probe as native


PROFILE = "openssh-ed25519-aes256gcm-bcrypt-v1"
MANIFEST_FIELDS = {
    "version", "profile", "device_id", "credential_id", "public_key",
    "key_sha256", "key_size",
}
PACKAGE_FILES = {"key.openssh", "manifest.json"}
PASSWORD = native.PASSWORD
INPUT = PASSWORD + b"\n" + PASSWORD + b"\n"
TIMEOUT = 20
OUTPUT_LIMIT = 32 * 1024
EXT_MAGIC = 0xEF53
FAILPOINTS = ("key-synced", "manifest-synced", "before-publish", "after-publish")
FOREIGN_UID = 65534 if os.geteuid() != 65534 else 65533
CURRENT_CASE = "preflight"


class ProbeError(Exception):
    pass


class BindingMismatch(ProbeError):
    pass


class LeaseInvalidated(ProbeError):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def mark_case(name):
    global CURRENT_CASE
    CURRENT_CASE = name


def child_environment(root, agent_socket=None):
    result = native.environment(root, agent_socket=agent_socket)
    result["VIVAC_OPENSSH_DISPOSABLE"] = "1"
    return result


def bounded(arguments, env, input_data=None, *, timeout=TIMEOUT):
    require(input_data is None or len(input_data) <= 2048, "fixture input exceeded its bound")
    process = subprocess.Popen(
        [str(part) for part in arguments], env=env,
        stdin=subprocess.DEVNULL if input_data is None else subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
        preexec_fn=native.child_setup, bufsize=0,
    )
    output = bytearray()
    deadline = time.monotonic() + timeout
    try:
        if input_data is not None:
            try:
                payload = memoryview(input_data)
                position = 0
                while position < len(payload):
                    written = process.stdin.write(payload[position:])
                    require(written is not None and written > 0, "fixture pipe write made no progress")
                    position += written
                process.stdin.close()
            except BrokenPipeError:
                process.stdin.close()
        import selectors
        os.set_blocking(process.stdout.fileno(), False)
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                require(time.monotonic() < deadline, "native package operation timed out")
                for key, _ in selector.select(0.05):
                    chunk = os.read(key.fd, 4096)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    output.extend(chunk)
                    require(len(output) <= OUTPUT_LIMIT, "native package output exceeded its bound")
            remaining = deadline - time.monotonic()
            require(remaining > 0, "native package operation timed out")
            return process.wait(timeout=remaining), bytes(output)
    except subprocess.TimeoutExpired as error:
        raise ProbeError("native package operation timed out") from error
    finally:
        native.stop(process)
        process.stdout.close()


def filesystem_magic(path):
    import ctypes

    class StatFs(ctypes.Structure):
        _fields_ = [
            ("f_type", ctypes.c_long), ("f_bsize", ctypes.c_long),
            ("f_blocks", ctypes.c_ulong), ("f_bfree", ctypes.c_ulong),
            ("f_bavail", ctypes.c_ulong), ("f_files", ctypes.c_ulong),
            ("f_ffree", ctypes.c_ulong), ("f_fsid", ctypes.c_int * 2),
            ("f_namelen", ctypes.c_long), ("f_frsize", ctypes.c_long),
            ("f_flags", ctypes.c_long), ("f_spare", ctypes.c_long * 4),
        ]

    result = StatFs()
    libc = ctypes.CDLL(None, use_errno=True)
    require(libc.statfs(os.fsencode(path), ctypes.byref(result)) == 0, "filesystem query failed")
    return int(result.f_type) & 0xFFFFFFFF


def interfaces_are_loopback_only():
    names = {entry.name for entry in Path("/sys/class/net").iterdir()}
    return names == {"lo"}


def mount_flags(path):
    target = Path(path).resolve(strict=True)
    best = None
    for line in Path("/proc/self/mountinfo").read_text(encoding="ascii").splitlines():
        fields = line.split()
        if len(fields) < 6:
            continue
        mountpoint = Path(fields[4].replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\"))
        try:
            target.relative_to(mountpoint)
        except ValueError:
            continue
        if best is None or len(str(mountpoint)) > len(str(best[0])):
            best = (mountpoint, set(fields[5].split(",")))
    require(best is not None, "mount metadata was unavailable")
    return best[1]


def is_mountpoint(path):
    target = Path(path).resolve(strict=True)
    for line in Path("/proc/self/mountinfo").read_text(encoding="ascii").splitlines():
        fields = line.split()
        if len(fields) >= 5:
            mountpoint = Path(fields[4].replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\"))
            if mountpoint == target:
                return True
    return False


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def acl_xattrs(path):
    values = []
    for name in ("system.posix_acl_access", "system.posix_acl_default"):
        try:
            value = os.getxattr(path, name) if isinstance(path, int) else os.getxattr(
                path, name, follow_symlinks=False,
            )
        except OSError as error:
            if error.errno in (errno.ENODATA, errno.EOPNOTSUPP):
                continue
            if error.errno in (errno.EACCES, errno.EPERM):
                values.append((name, "inaccessible"))
                continue
            raise
        values.append((name, value.hex()))
    return tuple(values)


def validate_safe_tree_root(path):
    metadata = path.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid()
            and stat.S_IMODE(metadata.st_mode) == 0o700,
            "fixture root ownership or mode was refused")
    return metadata.st_dev, metadata.st_ino


def cleanup_owned_tree(path, identity):
    metadata = path.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and not stat.S_ISLNK(metadata.st_mode)
            and (metadata.st_dev, metadata.st_ino) == identity,
            "fixture cleanup root identity was refused")
    os.chown(path, os.geteuid(), os.getegid(), follow_symlinks=False)
    os.chmod(path, 0o700, follow_symlinks=False)
    # This removes only the harness-created disposable root after identity validation.
    shutil.rmtree(path)


def create(writer, destination, comment, env, input_data=INPUT, extra_env=None):
    child_env = dict(env)
    if extra_env:
        child_env.update(extra_env)
    return bounded([writer, "--package-create", destination, comment], child_env, input_data)


def check(verifier_writer, destination, env):
    return bounded([verifier_writer, "--package-check", destination], env)


def read_manifest(package):
    manifest_path = package / "manifest.json"
    metadata = manifest_path.lstat()
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
            and metadata.st_uid == os.geteuid() and stat.S_IMODE(metadata.st_mode) == 0o600
            and 0 < metadata.st_size <= 2048,
            "manifest metadata was refused")
    with open(manifest_path, "rb") as source:
        raw = source.read(2049)
    require(len(raw) <= 2048, "manifest exceeded its bound")
    return read_manifest_bytes(raw)


def package_snapshot(package):
    result = []
    for item in sorted(package.iterdir(), key=lambda entry: entry.name):
        info = item.lstat()
        if stat.S_ISREG(info.st_mode) and os.access(item, os.R_OK):
            data = item.read_bytes()
            content = hashlib.sha256(data).hexdigest()
        else:
            content = None
        result.append((item.name, stat.S_IFMT(info.st_mode), info.st_dev, info.st_ino,
                       info.st_uid, info.st_gid, stat.S_IMODE(info.st_mode), info.st_nlink,
                       info.st_size, content, acl_xattrs(item)))
    info = package.lstat()
    return (info.st_dev, info.st_ino, info.st_uid, info.st_gid,
            stat.S_IMODE(info.st_mode), acl_xattrs(package), tuple(result))


def parent_snapshot(path):
    info = path.lstat()
    base = (stat.S_IFMT(info.st_mode), info.st_dev, info.st_ino, info.st_uid,
            info.st_gid, stat.S_IMODE(info.st_mode), info.st_nlink, acl_xattrs(path))
    if stat.S_ISDIR(info.st_mode):
        children = []
        for child in sorted(path.iterdir(), key=lambda entry: entry.name):
            item = child.lstat()
            children.append((child.name, stat.S_IFMT(item.st_mode), item.st_dev, item.st_ino,
                             item.st_uid, item.st_gid, stat.S_IMODE(item.st_mode), item.st_nlink,
                             acl_xattrs(child)))
        return base, tuple(children)
    return base


def ancestor_snapshots(path):
    snapshots = []
    current = path.parent
    while True:
        snapshots.append(parent_snapshot(current))
        if current == Path("/store") or current == Path("/"):
            break
        current = current.parent
    return tuple(snapshots)


def check_read_only(writer, package, env, expected_success, case):
    mark_case(case)
    before = (package_snapshot(package) if package.is_dir() and not package.is_symlink()
              and os.access(package, os.R_OK | os.X_OK) else None)
    parents_before = ancestor_snapshots(package)
    temp_before = parent_snapshot(Path("/tmp"))
    code, _ = check(writer, package, env)
    if before is not None:
        require(package_snapshot(package) == before, case + " package-check modified the package")
    require(ancestor_snapshots(package) == parents_before,
            case + " package-check modified an ancestor directory")
    require(parent_snapshot(Path("/tmp")) == temp_before,
            case + " package-check wrote into the private temporary filesystem")
    require((code == 0) == expected_success, case + " package-check returned an unexpected result")
    return code


def write_in_place(path, contents):
    descriptor = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "r+b") as target:
        target.seek(0)
        target.write(contents)
        target.truncate()
        target.flush()
        os.fsync(target.fileno())


def manifest_bytes(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8") + b"\n"


def restore_file(path, contents, metadata):
    write_in_place(path, contents)
    os.chown(path, metadata.st_uid, metadata.st_gid, follow_symlinks=False)
    os.chmod(path, stat.S_IMODE(metadata.st_mode), follow_symlinks=False)


def expect_refusal(writer, package, env, case):
    return check_read_only(writer, package, env, False, case)


def encode_native(decoded):
    body = base64.b64encode(decoded)
    return b"-----BEGIN OPENSSH PRIVATE KEY-----\n" + b"\n".join(
        body[index:index + 70] for index in range(0, len(body), 70)
    ) + b"\n-----END OPENSSH PRIVATE KEY-----\n"


def replace_key_field(decoded, field_index, replacement):
    reader = native.Reader(decoded)
    require(reader.take(15) == b"openssh-key-v1\0", "native key magic was refused")
    fields = [reader.string(1024) for _ in range(3)]
    count = reader.number()
    public_blob = reader.string(128)
    encrypted = reader.string(native.KEY_LIMIT)
    tail = reader.take(len(decoded) - reader.position)
    fields[field_index] = replacement
    return (b"openssh-key-v1\0" + b"".join(native.wire_string(field) for field in fields)
            + struct.pack(">I", count) + native.wire_string(public_blob)
            + native.wire_string(encrypted) + tail)


def update_manifest_for_key(manifest, key_bytes):
    changed = dict(manifest)
    changed["key_sha256"] = hashlib.sha256(key_bytes).hexdigest()
    changed["key_size"] = len(key_bytes)
    return changed


def stat_signature(metadata):
    return (
        metadata.st_dev, metadata.st_ino, metadata.st_uid, metadata.st_gid,
        stat.S_IFMT(metadata.st_mode), stat.S_IMODE(metadata.st_mode),
        metadata.st_nlink, metadata.st_size,
    )


def read_descriptor(descriptor, limit):
    metadata = os.fstat(descriptor)
    require(0 <= metadata.st_size <= limit, "retained file exceeded its read bound")
    contents = os.pread(descriptor, limit + 1, 0)
    require(len(contents) == metadata.st_size and len(contents) <= limit,
            "retained file changed during bounded read")
    return contents


def open_retained_package(package):
    directory_fd = os.open(package, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW)
    key_fd = None
    manifest_fd = None
    try:
        directory_stat = os.fstat(directory_fd)
        require(stat.S_ISDIR(directory_stat.st_mode) and directory_stat.st_uid == os.geteuid()
                and stat.S_IMODE(directory_stat.st_mode) == 0o700 and not acl_xattrs(package),
                "retained package directory metadata was refused")
        require(set(os.listdir(directory_fd)) == PACKAGE_FILES,
                "retained package membership was refused")
        key_fd = os.open("key.openssh", os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW, dir_fd=directory_fd)
        manifest_fd = os.open("manifest.json", os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW, dir_fd=directory_fd)
        key_stat = os.fstat(key_fd)
        manifest_stat = os.fstat(manifest_fd)
        require(stat.S_ISREG(key_stat.st_mode) and key_stat.st_uid == os.geteuid()
                and stat.S_IMODE(key_stat.st_mode) == 0o600 and key_stat.st_nlink == 1
                and 0 < key_stat.st_size <= 65536 and not acl_xattrs(key_fd),
                "retained key metadata was refused")
        require(stat.S_ISREG(manifest_stat.st_mode) and manifest_stat.st_uid == os.geteuid()
                and stat.S_IMODE(manifest_stat.st_mode) == 0o600 and manifest_stat.st_nlink == 1
                and 0 < manifest_stat.st_size <= 2048
                and not acl_xattrs(manifest_fd),
                "retained manifest metadata was refused")
        require(stat_signature(os.stat("key.openssh", dir_fd=directory_fd, follow_symlinks=False))
                == stat_signature(key_stat)
                and stat_signature(os.stat("manifest.json", dir_fd=directory_fd, follow_symlinks=False))
                == stat_signature(manifest_stat),
                "retained package members changed during opening")
        key_bytes = read_descriptor(key_fd, 65536)
        manifest_bytes_value = read_descriptor(manifest_fd, 2048)
        parsed_manifest, parsed_manifest_bytes = read_manifest_bytes(manifest_bytes_value)
        require(parsed_manifest_bytes == manifest_bytes_value,
                "retained manifest encoding changed during parsing")
        return {
            "path": package, "directory_fd": directory_fd, "key_fd": key_fd,
            "manifest_fd": manifest_fd, "directory_stat": directory_stat,
            "key_stat": key_stat, "manifest_stat": manifest_stat,
            "key_bytes": key_bytes, "manifest_bytes": manifest_bytes_value,
            "manifest": parsed_manifest, "key_sha256": hashlib.sha256(key_bytes).hexdigest(),
            "directory_acl": acl_xattrs(package),
            "key_acl": acl_xattrs(key_fd),
            "manifest_acl": acl_xattrs(manifest_fd),
        }
    except BaseException:
        if manifest_fd is not None:
            os.close(manifest_fd)
        if key_fd is not None:
            os.close(key_fd)
        os.close(directory_fd)
        raise


def read_manifest_bytes(raw):
    require(0 < len(raw) <= 2048, "manifest exceeded its bound")

    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "manifest contains duplicate fields")
            result[key] = value
        return result

    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object)
    except (UnicodeError, ValueError) as error:
        raise ProbeError("manifest encoding was refused") from error
    require(isinstance(value, dict) and set(value) == MANIFEST_FIELDS,
            "manifest schema did not match the fixed package contract")
    require(type(value["version"]) is int and value["version"] == 1
            and value["profile"] == PROFILE,
            "manifest version or profile was refused")
    for name in ("device_id", "credential_id"):
        item = value[name]
        require(isinstance(item, str) and len(item) == 26
                and all(character in native.ALPHABET for character in item),
                "manifest identity format was refused")
    for name in ("public_key", "key_sha256"):
        item = value[name]
        require(isinstance(item, str) and len(item) == 64 and item == item.lower()
                and all(character in "0123456789abcdef" for character in item),
                "manifest hexadecimal field was refused")
    require(type(value["key_size"]) is int and 1 <= value["key_size"] <= 65536,
            "manifest key size was refused")
    return value, raw


def revalidate_retained_package(snapshot):
    package = snapshot["path"]
    directory_fd = snapshot["directory_fd"]
    if not (stat_signature(os.fstat(directory_fd)) == stat_signature(snapshot["directory_stat"])
            and stat_signature(package.lstat()) == stat_signature(snapshot["directory_stat"])):
        raise LeaseInvalidated("retained package directory identity changed")
    if not (set(os.listdir(directory_fd)) == PACKAGE_FILES
            and set(item.name for item in package.iterdir()) == PACKAGE_FILES):
        raise LeaseInvalidated("retained package membership changed")
    if acl_xattrs(package) != snapshot["directory_acl"]:
        raise LeaseInvalidated("retained package ACL changed")
    for name, descriptor, original in (
        ("key.openssh", snapshot["key_fd"], snapshot["key_stat"]),
        ("manifest.json", snapshot["manifest_fd"], snapshot["manifest_stat"]),
    ):
        if not (stat_signature(os.fstat(descriptor)) == stat_signature(original)
                and stat_signature(os.stat(name, dir_fd=directory_fd, follow_symlinks=False))
                == stat_signature(original)
                and stat_signature((package / name).lstat()) == stat_signature(original)):
            raise LeaseInvalidated("retained package member identity changed")
    if not (acl_xattrs(snapshot["key_fd"]) == snapshot["key_acl"]
            and acl_xattrs(snapshot["manifest_fd"]) == snapshot["manifest_acl"]):
        raise LeaseInvalidated("retained package member ACL changed")
    key_bytes = read_descriptor(snapshot["key_fd"], 65536)
    manifest_bytes_value = read_descriptor(snapshot["manifest_fd"], 2048)
    if not (key_bytes == snapshot["key_bytes"]
            and hashlib.sha256(key_bytes).hexdigest() == snapshot["key_sha256"]):
        raise LeaseInvalidated("retained encrypted key digest changed")
    if manifest_bytes_value != snapshot["manifest_bytes"]:
        raise LeaseInvalidated("retained manifest content changed")


def close_retained_package(snapshot):
    for name in ("manifest_fd", "key_fd", "directory_fd"):
        descriptor = snapshot.get(name)
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError as error:
                if error.errno != errno.EBADF:
                    raise
            snapshot[name] = None


def make_sealed_key_memfd(key_bytes):
    import fcntl

    memfd_create = getattr(os, "memfd_create", None)
    require(memfd_create is not None, "sealed anonymous file support is unavailable")
    fd = memfd_create("vivac-encrypted-key-fixture", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    try:
        os.fchmod(fd, 0o600)
        payload = memoryview(key_bytes)
        position = 0
        while position < len(payload):
            written = os.write(fd, payload[position:])
            require(written > 0, "sealed snapshot write made no progress")
            position += written
        require(stat.S_IMODE(os.fstat(fd).st_mode) == 0o600
                and fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE == os.O_RDWR,
                "sealed snapshot metadata was refused")
        seals = (fcntl.F_SEAL_WRITE | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SEAL)
        fcntl.fcntl(fd, fcntl.F_ADD_SEALS, seals)
        actual_seals = fcntl.fcntl(fd, fcntl.F_GET_SEALS)
        require(actual_seals & seals == seals, "sealed snapshot protections were incomplete")
        write_refused = False
        try:
            os.pwrite(fd, b"x", 0)
        except OSError as error:
            write_refused = error.errno in (errno.EPERM, errno.EBUSY)
        truncate_refused = False
        try:
            os.ftruncate(fd, max(0, len(key_bytes) - 1))
        except OSError as error:
            truncate_refused = error.errno in (errno.EPERM, errno.EBUSY)
        require(write_refused and truncate_refused,
                "sealed snapshot allowed a write or truncation")
        require(os.pread(fd, len(key_bytes) + 1, 0) == key_bytes,
                "sealed snapshot bytes changed during refusal checks")
        os.lseek(fd, 0, os.SEEK_SET)
        return fd, {"sealed": True, "write_refused": True, "truncate_refused": True}
    except BaseException:
        os.close(fd)
        raise


def mutation_cases(writer, package, env, baseline_manifest, baseline_manifest_bytes, baseline_key):
    manifest_path = package / "manifest.json"
    key_path = package / "key.openssh"
    manifest_stat = manifest_path.lstat()
    key_stat = key_path.lstat()
    cases = {}

    def manifest_case(name, mutate):
        mark_case(name)
        try:
            replacement = mutate(dict(baseline_manifest))
            raw = replacement if isinstance(replacement, bytes) else manifest_bytes(replacement)
            write_in_place(manifest_path, raw)
            expect_refusal(writer, package, env, name)
            cases[name] = True
        finally:
            restore_file(manifest_path, baseline_manifest_bytes, manifest_stat)
        check_read_only(writer, package, env, True, name + " restored baseline")

    manifest_case("manifest-malformed", lambda value: b"{invalid\n")
    manifest_case("manifest-oversize", lambda value: b" " * 2049)
    manifest_case("manifest-extra-field", lambda value: {**value, "extra": True})
    manifest_case("manifest-legacy-filesystem-identities-unknown", lambda value: {
        **value, "store_device": 1, "store_inode": 1, "key_device": 1, "key_inode": 1,
    })
    manifest_case("manifest-missing-field", lambda value: {key: item for key, item in value.items()
                                                              if key != "key_size"})
    manifest_case("manifest-duplicate-field", lambda value: (
        manifest_bytes(value).rstrip(b"\n")[:-1] + b',"version":1}\n'
    ))
    manifest_case("manifest-wrong-version", lambda value: {**value, "version": 2})
    manifest_case("manifest-wrong-profile", lambda value: {**value, "profile": "other"})
    manifest_case("manifest-invalid-device-id", lambda value: {**value, "device_id": "i" * 26})
    manifest_case("manifest-invalid-credential-id", lambda value: {**value, "credential_id": "o" * 26})
    manifest_case("manifest-invalid-public-key-hex", lambda value: {**value, "public_key": "G" * 64})
    manifest_case("manifest-public-key-header-mismatch", lambda value: {
        **value, "public_key": ("00" * 32 if value["public_key"] != "00" * 32 else "01" * 32),
    })
    manifest_case("manifest-invalid-digest-hex", lambda value: {**value, "key_sha256": "z" * 64})
    manifest_case("manifest-float-version", lambda value: {**value, "version": 1.0})
    manifest_case("manifest-negative-size", lambda value: {**value, "key_size": -1})
    manifest_case("manifest-oversize-key-size", lambda value: {**value, "key_size": 65537})
    manifest_case("manifest-key-size-mismatch", lambda value: {**value, "key_size": value["key_size"] + 1})
    manifest_case("manifest-key-digest-mismatch", lambda value: {**value, "key_sha256": "0" * 64})
    require((manifest_path.lstat().st_dev, manifest_path.lstat().st_ino)
            == (manifest_stat.st_dev, manifest_stat.st_ino), "manifest fixture inode changed")

    def key_case(name, contents, *, rehash=False):
        mark_case(name)
        try:
            write_in_place(key_path, contents)
            if rehash:
                updated = update_manifest_for_key(baseline_manifest, contents)
                write_in_place(manifest_path, manifest_bytes(updated))
            expect_refusal(writer, package, env, name)
            cases[name] = True
        finally:
            restore_file(key_path, baseline_key, key_stat)
            restore_file(manifest_path, baseline_manifest_bytes, manifest_stat)
        check_read_only(writer, package, env, True, name + " restored baseline")

    key_case("key-truncated", baseline_key[: max(1, len(baseline_key) // 2)], rehash=True)
    key_case("key-invalid-pem", b"not an openssh key\n", rehash=True)
    parsed = native.parse_key(key_path)
    bad_cipher = replace_key_field(parsed["decoded"], 0, b"aes128-ctr")
    key_case("key-wrong-cipher", encode_native(bad_cipher), rehash=True)
    key_case("key-plaintext-envelope", encode_native(replace_key_field(parsed["decoded"], 0, b"none")), rehash=True)
    for rounds in (0, 65):
        options = native.wire_string(b"s" * 16) + struct.pack(">I", rounds)
        malformed = replace_key_field(parsed["decoded"], 2, options)
        key_case("key-rounds-" + str(rounds), encode_native(malformed), rehash=True)
    malicious_reader = native.Reader(parsed["decoded"])
    malicious_reader.take(15)
    cipher, kdf, options = (malicious_reader.string(1024) for _ in range(3))
    public_count = malicious_reader.number()
    public_blob = malicious_reader.string(128)
    declared_prefix = (b"openssh-key-v1\0" + b"".join(native.wire_string(item) for item in (cipher, kdf, options))
                       + struct.pack(">I", public_count) + native.wire_string(public_blob)
                       + struct.pack(">I", 1024 * 1024) + b"x" * 256)
    malicious_key = encode_native(declared_prefix)
    require(len(malicious_key) <= 64 * 1024, "malicious native prefix fixture exceeded its input bound")
    key_case("key-malicious-ssh-prefix", malicious_key, rehash=True)
    require((key_path.lstat().st_dev, key_path.lstat().st_ino) == (key_stat.st_dev, key_stat.st_ino),
            "key fixture inode changed")
    check_read_only(writer, package, env, True, "all restored mutations")
    return cases


def create_positive(writer, package, env, case="positive-package-create"):
    mark_case(case)
    device_id, credential_id = native.identifier(), native.identifier()
    comment = "vivac.device-key.v1:" + device_id + ":" + credential_id
    code, _ = create(writer, package, comment, env)
    require(code == 0 and package.is_dir(), "package creation failed")
    metadata = package.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid()
            and stat.S_IMODE(metadata.st_mode) == 0o700,
            "published package directory metadata was refused")
    require(not acl_xattrs(package), "published package directory carried a POSIX ACL")
    require({item.name for item in package.iterdir()} == PACKAGE_FILES,
            "published package membership was refused")
    manifest, raw = read_manifest(package)
    key_path = package / "key.openssh"
    manifest_path = package / "manifest.json"
    require(not acl_xattrs(key_path) and not acl_xattrs(manifest_path),
            "published package member carried a POSIX ACL")
    key_data = native.read_private_file(key_path)
    parsed = native.parse_key(key_path)
    require(len(key_data) + len(raw) <= 67584, "package members exceeded the combined size bound")
    require(parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["kdf"] == "bcrypt"
            and parsed["rounds"] == 64, "published key protection profile was refused")
    require(len(parsed["public_key"]) == 32
            and manifest["public_key"] == parsed["public_key"].hex(),
            "manifest public key did not match the native key header")
    require(manifest["device_id"] == device_id and manifest["credential_id"] == credential_id,
            "manifest identities did not match the requested protected comment")
    require(manifest["key_sha256"] == hashlib.sha256(key_data).hexdigest()
            and manifest["key_size"] == len(key_data), "manifest key digest or size did not match")
    require(check_read_only(writer, package, env, True, "valid package") == 0,
            "valid package was not accepted")
    return {
        "device_id": device_id, "credential_id": credential_id, "comment": comment,
        "manifest": manifest, "manifest_bytes": raw, "key_bytes": key_data,
        "public_key": parsed["public_key"], "public_blob": parsed["public_blob"],
    }


def validate_invalid_inputs(writer, parent, env, comment):
    observations = {}
    fixtures = {"eof": b"", "mismatch": PASSWORD + b"\n" + native.WRONG_PASSWORD + b"\n"}
    for name, payload in fixtures.items():
        mark_case("invalid-input-" + name)
        target = parent / ("invalid-" + name)
        before = set(parent.iterdir())
        code, _ = create(writer, target, comment, env, payload)
        require(code != 0 and not target.exists(), name + " input was not refused before publication")
        require(set(parent.iterdir()) == before, name + " input left preparation artifacts")
        observations[name] = True
    return observations


def run_failpoints(writer, parent, env, comment):
    result = {}
    for failpoint in FAILPOINTS:
        mark_case("failpoint-" + failpoint)
        destination = parent / ("failpoint-" + failpoint)
        before = set(parent.iterdir())
        code, _ = create(writer, destination, comment, env, extra_env={"VIVAC_OPENSSH_PACKAGE_EXIT_AT": failpoint})
        require(code == 77, failpoint + " did not terminate at the requested failpoint")
        if failpoint == "after-publish":
            require(destination.is_dir(), "post-publication failpoint lost the committed package")
            checked = check_read_only(writer, destination, env, True, "after-publish failpoint")
            leftovers = set(parent.iterdir()) - before - {destination}
            require(not leftovers, "post-publication failpoint left an unexpected sibling artifact")
            result[failpoint] = {"exit_77": True, "destination_absent": False, "complete_package_accepted": checked == 0}
        else:
            require(not destination.exists(), failpoint + " published a package before its commit point")
            checked = check_read_only(writer, destination, env, False, failpoint + " absent destination")
            require(checked != 0, failpoint + " absent destination was accepted by package-check")
            pending = [item for item in set(parent.iterdir()) - before if item != destination]
            for item in pending:
                check_read_only(writer, item, env, False, failpoint + " pending preparation")
            result[failpoint] = {
                "exit_77": True, "destination_absent": True,
                "pending_observed": bool(pending), "pending_not_accepted": True,
            }
    return result


def run_race(writer, parent, env, comment_a, comment_b):
    mark_case("concurrent-creators")
    destination = parent / "race-destination"
    before = set(parent.iterdir())
    arguments = [
        [str(writer), "--package-create", str(destination), comment_a],
        [str(writer), "--package-create", str(destination), comment_b],
    ]
    children = []
    try:
        for argv in arguments:
            children.append(subprocess.Popen(
                argv, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, start_new_session=True,
                preexec_fn=native.child_setup, bufsize=0,
            ))
        for child in children:
            payload = memoryview(INPUT)
            position = 0
            while position < len(payload):
                written = child.stdin.write(payload[position:])
                require(written is not None and written > 0, "race fixture pipe write made no progress")
                position += written
            child.stdin.close()
        import selectors
        selector = selectors.DefaultSelector()
        outputs = {child: bytearray() for child in children}
        for child in children:
            os.set_blocking(child.stdout.fileno(), False)
            selector.register(child.stdout, selectors.EVENT_READ, child)
        deadline = time.monotonic() + TIMEOUT
        while selector.get_map():
            require(time.monotonic() < deadline, "concurrent package creators timed out")
            for key, _ in selector.select(0.05):
                chunk = os.read(key.fd, 4096)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                captured = outputs[key.data]
                captured.extend(chunk)
                require(len(captured) <= OUTPUT_LIMIT, "concurrent creator output exceeded its bound")
        codes = []
        for child in children:
            remaining = deadline - time.monotonic()
            require(remaining > 0, "concurrent package creators timed out")
            codes.append(child.wait(timeout=remaining))
        require(sorted(code == 0 for code in codes) == [False, True],
                "concurrent creators did not produce exactly one winner")
        require(destination.is_dir(), "concurrent creator winner did not publish")
        require(set(parent.iterdir()) - before == {destination},
                "concurrent creators left an unexpected sibling artifact")
        check_read_only(writer, destination, env, True, "race winner")
        return {"one_winner": True, "loser_refused": True, "winner_package_accepted": True}
    finally:
        for child in children:
            native.stop(child)


def produce_loaded_proof(verifier, root, env, agent_socket, snapshot, expected_public_blob,
                         expected_comment, proof_name, signer, *, verifier_runner=None):
    revalidate_retained_package(snapshot)
    identities = native.identities(agent_socket)
    require(len(identities) == 1, "private foreground SSH agent did not contain exactly one identity")
    public_blob, protected_comment = identities[0]
    manifest = snapshot["manifest"]
    parsed = native.parse_key_bytes(snapshot["key_bytes"])
    requested_comment = ("vivac.device-key.v1:" + manifest["device_id"] + ":" + manifest["credential_id"]).encode("ascii")
    if (public_blob != expected_public_blob or protected_comment != expected_comment.encode("ascii")
            or public_blob != parsed["public_blob"]
            or manifest["public_key"] != parsed["public_key"].hex()
            or protected_comment != requested_comment):
        raise BindingMismatch("loaded identity did not match the current manifest")
    challenge = native.challenge(parsed["public_key"], manifest["device_id"], manifest["credential_id"])
    signature = signer(public_blob, native.canonical(challenge))
    document = native.proof(challenge, signature)
    (native.verify if verifier_runner is None else verifier_runner)(
        verifier, root, env, document, proof_name, True)
    return True


def run_cached_key_lease_case(writer, verifier, package, env, root, agent_socket, snapshot, expected):
    mark_case("cached-agent-key-mutation-invalidates-lease")
    key_path = package / "key.openssh"
    original_bytes = snapshot["key_bytes"]
    original_stat = snapshot["key_stat"]
    original_identity = (original_stat.st_dev, original_stat.st_ino)
    require(native.identities(agent_socket) == [(expected["public_blob"], expected["comment"].encode("ascii"))],
            "cached native agent identity was not correct before lease mutation")
    changed_bytes = bytearray(original_bytes)
    position = len(changed_bytes) // 2
    changed_bytes[position] ^= 1
    attempted_mutation = False
    try:
        attempted_mutation = True
        write_in_place(key_path, bytes(changed_bytes))
        current = key_path.lstat()
        require((current.st_dev, current.st_ino) == original_identity
                and current.st_size == original_stat.st_size,
                "key mutation did not preserve the selected file inode and size")
        require(native.identities(agent_socket) == [(expected["public_blob"], expected["comment"].encode("ascii"))],
                "cached native agent identity changed after key file mutation")
        signer_calls = [0]

        def counted_signer(public_blob, message):
            signer_calls[0] += 1
            return native.sign(agent_socket, public_blob, message)

        negative_proof = root / "package-proof-mutated-key.json"
        require(not negative_proof.exists(), "mutated-key proof destination already existed")
        try:
            produce_loaded_proof(
                verifier, root, env, agent_socket, snapshot, expected["public_blob"], expected["comment"],
                "package-proof-mutated-key", counted_signer,
            )
        except LeaseInvalidated as error:
            require(str(error) == "retained encrypted key digest changed",
                    "mutated key file was refused for an unexpected lease reason")
            pass
        else:
            raise ProbeError("mutated key file did not invalidate the retained package lease")
        require(signer_calls[0] == 0 and not negative_proof.exists(),
                "mutated key file reached native signing or proof verification")
        require(native.identities(agent_socket) == [(expected["public_blob"], expected["comment"].encode("ascii"))],
                "cached native agent identity was not preserved during lease refusal")
    finally:
        if attempted_mutation:
            restore_file(key_path, original_bytes, original_stat)
        require((key_path.lstat().st_dev, key_path.lstat().st_ino) == original_identity,
                "key file inode changed during mutation restoration")
        close_retained_package(snapshot)

    check_read_only(writer, package, env, True, "restored package after lease mutation")
    refreshed = open_retained_package(package)
    revalidate_retained_package(refreshed)
    require(refreshed["manifest"] == expected["manifest"]
            and refreshed["key_bytes"] == original_bytes,
            "refreshed package lease did not match the restored baseline")
    require(native.identities(agent_socket) == [(expected["public_blob"], expected["comment"].encode("ascii"))],
            "cached native agent identity was not correct after lease refresh")
    baseline_sign_calls = [0]

    def refreshed_signer(public_blob, message):
        revalidate_retained_package(refreshed)
        baseline_sign_calls[0] += 1
        signature = native.sign(agent_socket, public_blob, message)
        revalidate_retained_package(refreshed)
        return signature

    require(produce_loaded_proof(
        verifier, root, env, agent_socket, refreshed, expected["public_blob"], expected["comment"],
        "package-proof-after-key-restore", refreshed_signer,
    ), "baseline proof did not recover with a fresh lease")
    require(baseline_sign_calls[0] == 1,
            "fresh baseline proof did not call the native agent exactly once")
    return refreshed, {
        "cached_identity_unchanged": True,
        "same_inode_key_byte_mutation": True,
        "lease_invalidated_before_sign": True,
        "negative_sign_calls": 0,
        "negative_proof_not_generated": True,
        "fresh_lease_after_restore": True,
        "baseline_proof_after_restore_passed": True,
    }


def run_agent_proof(writer, verifier, package, env, root, expected, report):
    mark_case("native-agent-and-proof")
    socket_path = root / "agent.sock"
    agent_env = child_environment(root, socket_path)
    baseline = None
    changed = None
    memfds = []
    agent = None
    manifest_path = package / "manifest.json"
    baseline_manifest_stat = None
    baseline_manifest_bytes = expected["manifest_bytes"]
    try:
        check_read_only(writer, package, env, True, "package before retained native load")
        baseline = open_retained_package(package)
        baseline_manifest_stat = baseline["manifest_stat"]
        revalidate_retained_package(baseline)
        parsed = native.parse_key_bytes(baseline["key_bytes"])
        manifest = baseline["manifest"]
        require(parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["kdf"] == "bcrypt"
                and parsed["rounds"] == 64,
                "retained native key profile was refused before provider access")
        require(manifest["key_size"] == len(baseline["key_bytes"])
                and manifest["key_sha256"] == baseline["key_sha256"]
                and manifest["public_key"] == parsed["public_key"].hex()
                and manifest == expected["manifest"],
                "retained manifest did not bind the bounded encrypted snapshot")
        base_memfd, base_seals = make_sealed_key_memfd(baseline["key_bytes"])
        memfds.append(base_memfd)
        report["sealed_encrypted_snapshot"] = {
            "memfd_created": True, "write_sealed": base_seals["write_refused"],
            "truncate_sealed": base_seals["truncate_refused"],
            "inherited_by_ssh_add": True, "filesystem_path_passed_to_provider": False,
        }
        revalidate_retained_package(baseline)
        agent = subprocess.Popen(
            ["ssh-agent", "-D", "-a", str(socket_path)], env=agent_env,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True, preexec_fn=native.child_setup,
        )
        ready_by = time.monotonic() + 5
        while not socket_path.exists():
            require(agent.poll() is None, "private foreground SSH agent exited before readiness")
            require(time.monotonic() < ready_by, "private foreground SSH agent readiness timed out")
            time.sleep(0.02)
        require(native.identities(socket_path) == [], "private foreground SSH agent was not empty")
        ssh_add = shutil.which("ssh-add", path=os.defpath)
        # The same retained ciphertext and memfd used above are passed to the loader.
        revalidate_retained_package(baseline)
        try:
            code, _ = native.pty_run(
                [ssh_add, "-k", "/proc/self/fd/" + str(base_memfd)], agent_env, [PASSWORD],
                inherited_fds=(base_memfd,),
            )
        finally:
            revalidate_retained_package(baseline)
        require(code == 0, "native ssh-add refused the sealed package snapshot")

        baseline_sign_count = [0]

        def baseline_signer(public_blob, message):
            revalidate_retained_package(baseline)
            baseline_sign_count[0] += 1
            signature = native.sign(socket_path, public_blob, message)
            revalidate_retained_package(baseline)
            return signature

        require(produce_loaded_proof(
            verifier, root, env, socket_path, baseline, expected["public_blob"], expected["comment"],
            "package-proof", baseline_signer,
        ), "baseline package proof was not produced")
        revalidate_retained_package(baseline)
        require(baseline_sign_count[0] == 1, "baseline proof did not call the native agent exactly once")
        report["native_binding"] = {"exactly_one_identity": True, "protected_comment_match": True,
                                    "public_key_match": True, "shared_proof_verifier_passed": True}
        baseline, report["cached_key_mutation"] = run_cached_key_lease_case(
            writer, verifier, package, env, root, socket_path, baseline, expected,
        )
        baseline_manifest_stat = baseline["manifest_stat"]
        manifest = baseline["manifest"]
        native.clear_agent(socket_path)
        require(native.identities(socket_path) == [], "private foreground SSH agent did not clear baseline identity")

        mark_case("changed-public-identities-before-proof")
        altered = dict(manifest)
        altered["device_id"] = native.identifier()
        altered["credential_id"] = native.identifier()
        try:
            write_in_place(manifest_path, manifest_bytes(altered))
            before = package_snapshot(package)
            ancestors = ancestor_snapshots(package)
            temp_before = parent_snapshot(Path("/tmp"))
            structure_code, _ = check(writer, package, env)
            require(structure_code == 0,
                    "Rust package-check refused structurally valid changed public IDs")
            require(package_snapshot(package) == before
                    and ancestor_snapshots(package) == ancestors
                    and parent_snapshot(Path("/tmp")) == temp_before,
                    "changed-id package-check modified package or ancestors")
            changed = open_retained_package(package)
            revalidate_retained_package(changed)
            require(changed["manifest"]["device_id"] == altered["device_id"]
                    and changed["manifest"]["credential_id"] == altered["credential_id"]
                    and changed["manifest"]["key_sha256"] == baseline["key_sha256"]
                    and changed["manifest"]["public_key"] == expected["manifest"]["public_key"],
                    "changed-id snapshot did not preserve the key and public header")
            changed_memfd, changed_seals = make_sealed_key_memfd(changed["key_bytes"])
            memfds.append(changed_memfd)
            require(changed_seals["write_refused"] and changed_seals["truncate_refused"],
                    "changed-id encrypted snapshot was not sealed")
            native.clear_agent(socket_path)
            require(native.identities(socket_path) == [],
                    "private foreground SSH agent was not empty before changed-id load")
            revalidate_retained_package(changed)
            try:
                code, _ = native.pty_run(
                    [ssh_add, "-k", "/proc/self/fd/" + str(changed_memfd)], agent_env, [PASSWORD],
                    inherited_fds=(changed_memfd,),
                )
            finally:
                revalidate_retained_package(changed)
            require(code == 0, "native ssh-add did not load changed-id package snapshot")
            loaded = native.identities(socket_path)
            require(len(loaded) == 1 and loaded[0] == (expected["public_blob"], expected["comment"].encode("ascii")),
                    "changed-id package did not load exactly its unchanged protected identity")

            negative_sign_count = [0]

            def negative_signer(public_blob, message):
                revalidate_retained_package(changed)
                negative_sign_count[0] += 1
                signature = native.sign(socket_path, public_blob, message)
                revalidate_retained_package(changed)
                return signature

            negative_proof = root / "package-proof-changed-ids.json"
            require(not negative_proof.exists(), "changed-id proof destination already existed")
            try:
                produce_loaded_proof(
                    verifier, root, env, socket_path, changed, expected["public_blob"], expected["comment"],
                    "package-proof-changed-ids", negative_signer,
                )
            except BindingMismatch:
                pass
            else:
                raise ProbeError("changed manifest IDs passed the native identity binding gate")
            revalidate_retained_package(changed)
            require(negative_sign_count[0] == 0 and not negative_proof.exists(),
                    "changed manifest IDs reached signing or proof verification")
            report["changed_public_ids"] = {
                "structural_check_passed": True,
                "encrypted_snapshot_loaded": True,
                "one_unchanged_protected_identity_observed": True,
                "binding_gate_refused_before_sign": True,
                "sign_calls": 0,
                "proof_not_generated": True,
                "identity_authentication_claim": False,
            }
            native.clear_agent(socket_path)
            require(native.identities(socket_path) == [], "private foreground SSH agent did not clear changed identity")
        finally:
            if changed is not None:
                close_retained_package(changed)
                changed = None
            restore_file(manifest_path, baseline_manifest_bytes, baseline_manifest_stat)
            check_read_only(writer, package, env, True, "identity manifest restored")
            revalidate_retained_package(baseline)
    finally:
        if agent is not None:
            if socket_path.exists():
                try:
                    native.clear_agent(socket_path)
                except (native.ProbeError, OSError):
                    pass
            native.stop(agent)
        try:
            socket_path.unlink()
        except FileNotFoundError:
            pass
        for descriptor in memfds:
            try:
                os.close(descriptor)
            except OSError:
                pass
        if changed is not None:
            close_retained_package(changed)
        if baseline is not None:
            close_retained_package(baseline)


def permission_cases(writer, package, parent, env, setfacl):
    key_path = package / "key.openssh"
    manifest_path = package / "manifest.json"
    directory_stat = package.lstat()
    key_stat = key_path.lstat()
    manifest_stat = manifest_path.lstat()
    results = {}

    def check_mutated(name, mutate, restore):
        mark_case(name)
        try:
            mutate()
            expect_refusal(writer, package, env, name)
            results[name] = True
        finally:
            restore()
        check_read_only(writer, package, env, True, name + " restored")

    check_mutated(
        "permissive-package-mode",
        lambda: os.chmod(package, 0o755),
        lambda: os.chmod(package, stat.S_IMODE(directory_stat.st_mode)),
    )
    check_mutated(
        "permissive-key-mode",
        lambda: os.chmod(key_path, 0o644),
        lambda: os.chmod(key_path, stat.S_IMODE(key_stat.st_mode)),
    )
    check_mutated(
        "permissive-manifest-mode",
        lambda: os.chmod(manifest_path, 0o644),
        lambda: os.chmod(manifest_path, stat.S_IMODE(manifest_stat.st_mode)),
    )

    def foreign_key_owner():
        os.chown(key_path, FOREIGN_UID, key_stat.st_gid, follow_symlinks=False)
        os.chmod(key_path, stat.S_IMODE(key_stat.st_mode), follow_symlinks=False)

    def restore_key_owner():
        os.chown(key_path, key_stat.st_uid, key_stat.st_gid, follow_symlinks=False)
        os.chmod(key_path, stat.S_IMODE(key_stat.st_mode), follow_symlinks=False)

    check_mutated("foreign-key-owner", foreign_key_owner, restore_key_owner)

    def foreign_manifest_owner():
        os.chown(manifest_path, FOREIGN_UID, manifest_stat.st_gid, follow_symlinks=False)
        os.chmod(manifest_path, stat.S_IMODE(manifest_stat.st_mode), follow_symlinks=False)

    def restore_manifest_owner():
        os.chown(manifest_path, manifest_stat.st_uid, manifest_stat.st_gid, follow_symlinks=False)
        os.chmod(manifest_path, stat.S_IMODE(manifest_stat.st_mode), follow_symlinks=False)

    check_mutated("foreign-manifest-owner", foreign_manifest_owner, restore_manifest_owner)

    def acl_mutation(path, arguments):
        require(setfacl is not None, "setfacl executable is absent")
        code, _ = bounded([setfacl, *arguments, str(path)], env)
        require(code == 0, "POSIX ACL fixture operation failed")

    def require_acl_entry(path, expected):
        getfacl = shutil.which("getfacl", path=os.defpath)
        require(getfacl is not None, "getfacl executable is absent")
        code, output = bounded([getfacl, "-cpn", str(path)], env)
        entries = [line.split("#", 1)[0].rstrip()
                   for line in output.decode("utf-8", errors="replace").splitlines()]
        require(code == 0 and expected in entries,
                "POSIX ACL fixture was not installed")

    check_mutated(
        "key-access-acl",
        lambda: (acl_mutation(key_path, ["-m", f"u:{FOREIGN_UID}:r--"]), os.chmod(key_path, 0o600),
                 require_acl_entry(key_path, f"user:{FOREIGN_UID}:r--")),
        lambda: (acl_mutation(key_path, ["-b"]), os.chmod(key_path, 0o600)),
    )
    check_mutated(
        "manifest-access-acl",
        lambda: (acl_mutation(manifest_path, ["-m", f"u:{FOREIGN_UID}:r--"]), os.chmod(manifest_path, 0o600),
                 require_acl_entry(manifest_path, f"user:{FOREIGN_UID}:r--")),
        lambda: (acl_mutation(manifest_path, ["-b"]), os.chmod(manifest_path, 0o600)),
    )
    check_mutated(
        "package-default-acl",
        lambda: (acl_mutation(package, ["-d", "-m", f"u:{FOREIGN_UID}:r-x"]),
                 require_acl_entry(package, f"default:user:{FOREIGN_UID}:r-x")),
        lambda: (acl_mutation(package, ["-k"]), os.chmod(package, 0o700)),
    )
    check_mutated(
        "parent-access-acl",
        lambda: (acl_mutation(parent, ["-m", f"u:{FOREIGN_UID}:r-x"]), os.chmod(parent, 0o700),
                 require_acl_entry(parent, f"user:{FOREIGN_UID}:r-x")),
        lambda: (acl_mutation(parent, ["-b"]), os.chmod(parent, 0o700)),
    )
    check_mutated(
        "parent-default-acl",
        lambda: (acl_mutation(parent, ["-d", "-m", f"u:{FOREIGN_UID}:r-x"]),
                 require_acl_entry(parent, f"default:user:{FOREIGN_UID}:r-x")),
        lambda: acl_mutation(parent, ["-k"]),
    )
    for name, setup, cleanup in (
        ("extra-entry", lambda: (package / "extra").write_bytes(b"fixture"), lambda: (package / "extra").unlink()),
        ("extra-subdirectory", lambda: (package / "extra-dir").mkdir(mode=0o700), lambda: (package / "extra-dir").rmdir()),
        ("key-symlink", lambda: os.symlink("key.openssh", package / "extra-link"), lambda: (package / "extra-link").unlink()),
    ):
        check_mutated(name, setup, cleanup)
    hardlink = parent / "key-hardlink"
    try:
        os.link(key_path, hardlink)
        expect_refusal(writer, package, env, "key-extra-hardlink")
        results["key-extra-hardlink"] = True
    finally:
        if hardlink.exists():
            hardlink.unlink()
    check_read_only(writer, package, env, True, "hardlink restored")

    def foreign_package_owner():
        os.chown(package, FOREIGN_UID, directory_stat.st_gid, follow_symlinks=False)

    def restore_package_owner():
        os.chown(package, directory_stat.st_uid, directory_stat.st_gid, follow_symlinks=False)
        os.chmod(package, stat.S_IMODE(directory_stat.st_mode), follow_symlinks=False)

    check_mutated("foreign-package-owner", foreign_package_owner, restore_package_owner)
    require((package.lstat().st_dev, package.lstat().st_ino) == (directory_stat.st_dev, directory_stat.st_ino)
            and (key_path.lstat().st_dev, key_path.lstat().st_ino) == (key_stat.st_dev, key_stat.st_ino),
            "permission fixture changed a retained package inode")
    return results


def unsafe_path_cases(writer, root, env, comment):
    results = {}
    for label in ("git-ancestor", ".vivac-ancestor"):
        mark_case(label)
        ancestor = root / label
        ancestor.mkdir(mode=0o700)
        if label == "git-ancestor":
            (ancestor / ".git").mkdir(mode=0o700)
            destination_parent = ancestor
        else:
            destination_parent = ancestor / ".vivac"
            destination_parent.mkdir(mode=0o700)
        target = destination_parent / "destination"
        before = set(ancestor.iterdir())
        destination_parent_before = set(destination_parent.iterdir())
        code, _ = create(writer, target, comment, env)
        require(code != 0 and not target.exists() and set(ancestor.iterdir()) == before
                and set(destination_parent.iterdir()) == destination_parent_before,
                label + " destination was not refused before parent changes")
        results[label] = True
    private_parent = root / "unprivate-parent"
    mark_case("unprivate-parent")
    private_parent.mkdir(mode=0o755)
    os.chmod(private_parent, 0o755)
    target = private_parent / "destination"
    before = set(private_parent.iterdir())
    code, _ = create(writer, target, comment, env)
    require(code != 0 and not target.exists() and set(private_parent.iterdir()) == before,
            "unprivate parent was not refused before parent changes")
    results["unprivate-parent"] = True
    foreign_parent = root / "foreign-parent"
    mark_case("foreign-parent")
    foreign_parent.mkdir(mode=0o700)
    foreign_stat = foreign_parent.lstat()
    os.chown(foreign_parent, FOREIGN_UID, foreign_stat.st_gid, follow_symlinks=False)
    target = foreign_parent / "destination"
    try:
        code, _ = create(writer, target, comment, env)
    finally:
        os.chown(foreign_parent, foreign_stat.st_uid, foreign_stat.st_gid, follow_symlinks=False)
        os.chmod(foreign_parent, stat.S_IMODE(foreign_stat.st_mode), follow_symlinks=False)
    require(code != 0 and not target.exists() and not list(foreign_parent.iterdir()),
            "foreign-owned parent was not refused before parent changes")
    results["foreign-parent"] = True
    link_target = root / "parent-link"
    mark_case("symlink-parent")
    os.symlink(root, link_target)
    target = link_target / "symlink-destination"
    before = set(root.iterdir())
    code, _ = create(writer, target, comment, env)
    require(code != 0 and not target.exists() and set(root.iterdir()) == before,
            "symlink parent was not refused before parent changes")
    results["symlink-parent"] = True
    symlink_destination = root / "destination-link"
    mark_case("symlink-destination")
    os.symlink(root, symlink_destination)
    before = set(root.iterdir())
    code, _ = create(writer, symlink_destination, comment, env)
    require(code != 0 and symlink_destination.is_symlink() and set(root.iterdir()) == before,
            "symlink destination was replaced or modified its target")
    results["symlink-destination-untouched"] = True
    return results


def linux_process_start_time(pid):
    record = (Path("/proc") / str(pid) / "stat").read_bytes()
    fields = record[record.rfind(b")") + 2:].split()
    require(len(fields) > 19, "child process identity receipt was malformed")
    return fields[19].decode("ascii")


def wait_for_child_parent_fd(process, writer, parent, start_time, timeout=8):
    expected = parent.stat()
    deadline = time.monotonic() + timeout
    proc = Path("/proc") / str(process.pid)
    while time.monotonic() < deadline:
        require(process.poll() is None, "package creator exited before retaining its parent")
        require(linux_process_start_time(process.pid) == start_time
                and (proc / "exe").resolve() == writer.resolve(),
                "package creator process identity changed")
        descriptors = proc / "fd"
        for entry in descriptors.iterdir():
            try:
                actual = entry.stat()
            except OSError:
                continue
            if (actual.st_dev, actual.st_ino) == (expected.st_dev, expected.st_ino):
                return True
        time.sleep(0.01)
    return False


def finish_pipe_child(process, deadline):
    import selectors

    output = bytearray()
    os.set_blocking(process.stdout.fileno(), False)
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        while selector.get_map():
            require(time.monotonic() < deadline, "parent-replacement child timed out")
            for key, _ in selector.select(0.05):
                chunk = os.read(key.fd, 4096)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                output.extend(chunk)
                require(len(output) <= OUTPUT_LIMIT, "parent-replacement output exceeded its bound")
    remaining = deadline - time.monotonic()
    require(remaining > 0, "parent-replacement child timed out")
    return process.wait(timeout=remaining), bytes(output)


def run_parent_replacement_case(writer, parent, env, comment):
    mark_case("retained-parent-replacement")
    target = parent / "held-parent-target"
    held_parent = parent.with_name(parent.name + "-held")
    require(not target.exists() and not held_parent.exists(),
            "parent-replacement fixture names were not fresh")
    parent_stat = parent.lstat()
    parent_identity = (parent_stat.st_dev, parent_stat.st_ino)
    before_entries = set(parent.iterdir())
    process = subprocess.Popen(
        [str(writer), "--package-create", str(target), comment], env=env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        start_new_session=True, preexec_fn=native.child_setup, bufsize=0,
    )
    start_time = None
    replacement_identity = None
    renamed = False
    try:
        start_time = linux_process_start_time(process.pid)
        require(wait_for_child_parent_fd(process, writer, parent, start_time),
                "creator did not retain the selected parent before passphrase input")
        require(process.poll() is None, "creator exited before the parent replacement")
        require(linux_process_start_time(process.pid) == start_time
                and (Path("/proc") / str(process.pid) / "exe").resolve() == writer.resolve(),
                "creator process identity changed before parent replacement")
        os.rename(parent, held_parent)
        renamed = True
        parent.mkdir(mode=0o700)
        replacement_stat = parent.lstat()
        replacement_identity = (replacement_stat.st_dev, replacement_stat.st_ino)
        sentinel = parent / "replacement-sentinel"
        native.private_write(sentinel, b"package parent replacement fixture\n")
        sentinel_stat = sentinel.lstat()
        sentinel_identity = (sentinel_stat.st_dev, sentinel_stat.st_ino)
        replacement_before = set(parent.iterdir())
        payload = memoryview(INPUT)
        position = 0
        while position < len(payload):
            written = process.stdin.write(payload[position:])
            require(written is not None and written > 0,
                    "parent-replacement passphrase pipe write made no progress")
            position += written
        process.stdin.close()
        code, _ = finish_pipe_child(process, time.monotonic() + TIMEOUT)
        require(code != 0, "creator published through a replaced parent path")
        require(not target.exists(), "creator published at the replacement pathname")
        require(set(held_parent.iterdir()) == before_entries,
                "creator wrote into the moved retained parent")
        require(set(parent.iterdir()) == replacement_before
                and sentinel.read_bytes() == b"package parent replacement fixture\n"
                and (sentinel.lstat().st_dev, sentinel.lstat().st_ino) == sentinel_identity
                and (parent.lstat().st_dev, parent.lstat().st_ino) == replacement_identity,
                "creator modified the replacement parent or its sentinel")
        return {"retained_parent_detected": True, "replacement_refused": True,
                "moved_parent_unchanged": True, "replacement_unchanged": True}
    finally:
        if process.poll() is None:
            native.stop(process)
        if process.stdin is not None and not process.stdin.closed:
            process.stdin.close()
        if process.stdout is not None:
            process.stdout.close()
        if replacement_identity is not None and parent.exists():
            current = parent.lstat()
            require((current.st_dev, current.st_ino) == replacement_identity,
                    "replacement parent identity changed during cleanup")
            cleanup_owned_tree(parent, replacement_identity)
        if renamed and held_parent.exists():
            moved_stat = held_parent.lstat()
            require((moved_stat.st_dev, moved_stat.st_ino) == parent_identity,
                    "moved original parent identity changed")
            require(not parent.exists(), "original parent restore path was occupied")
            os.rename(held_parent, parent)
            restored = parent.lstat()
            require((restored.st_dev, restored.st_ino) == parent_identity,
                    "original parent identity was not restored")


def unsupported_filesystem_case(writer, env, comment):
    mark_case("unsupported-filesystem")
    candidate_base = Path("/dev/shm")
    if not candidate_base.is_dir() or filesystem_magic(candidate_base) == EXT_MAGIC:
        candidate_base = Path("/tmp")
    candidate = Path(tempfile.mkdtemp(prefix="vivac-package-unsupported-", dir=candidate_base))
    candidate.chmod(0o700)
    candidate_identity = (candidate.lstat().st_dev, candidate.lstat().st_ino)
    try:
        magic = filesystem_magic(candidate)
        if magic == EXT_MAGIC:
            return {"unsupported_filesystem_refused": None, "tested": False}
        target = candidate / "unsupported-store"
        comment = "vivac.device-key.v1:" + native.identifier() + ":" + native.identifier()
        code, _ = create(writer, target, comment, env)
        require(code != 0 and not target.exists(), "unsupported filesystem was not refused before publication")
        require(set(candidate.iterdir()) == set(), "unsupported filesystem refusal left a package artifact")
        return {"unsupported_filesystem_refused": True, "tested": True}
    finally:
        current = candidate.lstat()
        require((current.st_dev, current.st_ino) == candidate_identity
                and current.st_uid == os.geteuid() and stat.S_IMODE(current.st_mode) == 0o700,
                "unsupported filesystem fixture root ownership changed")
        # This is the harness-owned disposable root, not a product cleanup claim.
        shutil.rmtree(candidate)


def run_race_and_existing(writer, parent, env):
    race_a = "vivac.device-key.v1:" + native.identifier() + ":" + native.identifier()
    race_b = "vivac.device-key.v1:" + native.identifier() + ":" + native.identifier()
    race = run_race(writer, parent, env, race_a, race_b)
    mark_case("existing-destination-untouched")
    destination = parent / "existing-destination"
    create_positive(writer, destination, env, "existing-destination-baseline")
    before = package_snapshot(destination)
    other_comment = "vivac.device-key.v1:" + native.identifier() + ":" + native.identifier()
    code, _ = create(writer, destination, other_comment, env)
    after = package_snapshot(destination)
    require(code != 0 and before == after, "existing destination was overwritten or modified")
    check_read_only(writer, destination, env, True, "existing destination after refusal")
    return {"race": race, "existing_destination_untouched": True}


def run_probe(writer, verifier, store, root, env):
    store_metadata = store.lstat()
    require(stat.S_ISDIR(store_metadata.st_mode) and store_metadata.st_uid == os.geteuid(),
            "store root ownership was refused")
    require(is_mountpoint(store), "store root is not a dedicated mountpoint")
    require(filesystem_magic(store) == EXT_MAGIC, "store filesystem is outside the ext-family experiment")
    temp_parent = Path(tempfile.mkdtemp(prefix="vivac-package-private-", dir=store))
    temp_parent.chmod(0o700)
    root_identity = validate_safe_tree_root(temp_parent)
    report = {
        "version": 1, "status": "incomplete", "structural_package_check_only": True,
        "filesystem_inode_metadata_runtime_only": True,
        "custody_accredited": False, "native_identity_authentication_claim": False,
        "filesystem_persistence_attested": False, "power_loss_claim": False, "cases": {},
    }
    try:
        comment = "vivac.device-key.v1:" + native.identifier() + ":" + native.identifier()
        report["cases"]["invalid_inputs"] = validate_invalid_inputs(writer, temp_parent, env, comment)
        report["cases"]["retained_parent_replacement"] = run_parent_replacement_case(
            writer, temp_parent, env, comment,
        )
        valid = temp_parent / "valid-package"
        expected = create_positive(writer, valid, env, "initial-valid-package")
        report["cases"]["valid_package"] = {"created": True, "exact_two_files": True,
                                             "encrypted_profile": True, "read_only_check": True}
        report["cases"]["package_mutations"] = mutation_cases(
            writer, valid, env, expected["manifest"], expected["manifest_bytes"], expected["key_bytes"],
        )
        report["cases"]["filesystem_safety"] = permission_cases(
            writer, valid, temp_parent, env, shutil.which("setfacl", path=os.defpath),
        )
        report["cases"]["unsafe_destinations"] = unsafe_path_cases(writer, temp_parent, env, comment)
        report["cases"]["publication_failpoints"] = run_failpoints(writer, temp_parent, env, comment)
        report["cases"]["race_and_existing"] = run_race_and_existing(writer, temp_parent, env)
        report["cases"]["unsupported_filesystem"] = unsupported_filesystem_case(writer, env, comment)
        require(report["cases"]["unsupported_filesystem"]["tested"] is True,
                "unsupported filesystem refusal could not be exercised")
        run_agent_proof(writer, verifier, valid, env, temp_parent, expected, report["cases"])
        report["status"] = "observed"
        report["provenance"] = {
            "writer_sha256": sha256_file(writer),
            "proof_verifier_sha256": sha256_file(verifier),
            "python_sha256": sha256_file(sys.executable),
            "package_probe_sha256": sha256_file(Path(__file__).resolve()),
            "openssh_probe_sha256": sha256_file(Path(native.__file__).resolve()),
            "dockerfile_sha256": sha256_file(Path(__file__).with_name("Dockerfile.package")),
            "ssh_sha256": sha256_file(shutil.which("ssh", path=os.defpath)),
            "ssh_add_sha256": sha256_file(shutil.which("ssh-add", path=os.defpath)),
            "ssh_agent_sha256": sha256_file(shutil.which("ssh-agent", path=os.defpath)),
            "setfacl_sha256": sha256_file(shutil.which("setfacl", path=os.defpath)),
            "getfacl_sha256": sha256_file(shutil.which("getfacl", path=os.defpath)),
            "openssh_version": get_openssh_version(env),
            "filesystem_magic": f"0x{EXT_MAGIC:04x}",
            "runtime_platform": platform.platform(),
        }
        return report
    finally:
        cleanup_owned_tree(temp_parent, root_identity)


def get_openssh_version(env):
    ssh = shutil.which("ssh", path=os.defpath)
    require(ssh is not None, "OpenSSH client executable is absent")
    code, output = bounded([ssh, "-V"], env)
    output = output.strip()
    require(code == 0 and 8 <= len(output) <= 256 and output.startswith(b"OpenSSH_"),
            "OpenSSH version receipt was refused")
    return output.decode("ascii", errors="strict")


def main():
    mark_case("preflight")
    parser = argparse.ArgumentParser()
    parser.add_argument("--writer", required=True, type=Path)
    parser.add_argument("--verifier", required=True, type=Path)
    parser.add_argument("--store-root", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    report_path = None
    root = None
    root_identity = None
    phase = "preflight"
    try:
        require(sys.platform.startswith("linux"), "Linux runtime is required")
        require(os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "disposable runtime declaration is required")
        require(interfaces_are_loopback_only(), "runtime network interfaces are not loopback-only")
        require("ro" in mount_flags("/"), "container root filesystem is not read-only")
        require(args.store_root == Path("/store") and not args.store_root.is_symlink(),
                "store root must be the explicit /store mount")
        writer = args.writer.resolve(strict=True)
        verifier = args.verifier.resolve(strict=True)
        store = args.store_root.resolve(strict=True)
        require(writer.is_file() and os.access(writer, os.X_OK), "writer executable was refused")
        require(verifier.is_file() and os.access(verifier, os.X_OK), "proof verifier executable was refused")
        require(args.report.is_absolute(), "report path must be absolute")
        report_parent = args.report.parent.resolve(strict=True)
        require(report_parent == Path("/reports"), "report must be written to the explicit /reports mount")
        require(is_mountpoint(report_parent) and "rw" in mount_flags(report_parent),
                "report parent is not a distinct writable mount")
        report_path = report_parent / args.report.name
        require(not report_path.exists() and not report_path.is_symlink(), "report destination already exists")
        require(store == Path("/store") and stat.S_ISDIR(store.lstat().st_mode),
                "store root must be the explicit /store mount")
        require("ro" in mount_flags(writer) and "ro" in mount_flags(verifier)
                and "ro" in mount_flags(Path(__file__).resolve())
                and "ro" in mount_flags(Path(native.__file__).resolve()),
                "probe source or native executables are not mounted read-only")
        require("rw" in mount_flags(store) and is_mountpoint(store),
                "private store must be a distinct writable mount")
        for executable in ("ssh", "ssh-add", "ssh-agent", "setfacl", "getfacl"):
            path = shutil.which(executable, path=os.defpath)
            require(path is not None and "ro" in mount_flags(path),
                    executable + " executable is absent or not mounted read-only")
        root = Path(tempfile.mkdtemp(prefix="vivac-package-runtime-", dir="/tmp"))
        root.chmod(0o700)
        root_identity = validate_safe_tree_root(root)
        (root / "home").mkdir(mode=0o700)
        env = child_environment(root)
        phase = "package publication and safety cases"
        mark_case("package-publication-and-safety")
        report = run_probe(writer, verifier, store, root, env)
        phase = "metadata report"
        mark_case("metadata-report")
        native.private_write(report_path, json.dumps(report, indent=2, sort_keys=True).encode("utf-8") + b"\n")
        print("Package observations: structural=true native_binding=true custody=false", flush=True)
        return 0
    except (ProbeError, native.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        number = error.errno if isinstance(error, OSError) else None
        if isinstance(error, (ProbeError, native.ProbeError)):
            detail = str(error)
        else:
            detail = f"{type(error).__name__}, errno={number}"
        print(f"Package probe failed during {phase}/{CURRENT_CASE}: {detail}; no result is accredited.",
              file=sys.stderr)
        return 1
    finally:
        if root is not None and root_identity is not None:
            try:
                cleanup_owned_tree(root, root_identity)
            except (ProbeError, OSError) as error:
                number = error.errno if isinstance(error, OSError) else None
                print(f"Package probe failed during fixture-cleanup: {type(error).__name__}, errno={number}; "
                      "no result is accredited.", file=sys.stderr)
                return 1


if __name__ == "__main__":
    sys.exit(main())
