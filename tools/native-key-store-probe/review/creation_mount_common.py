"""Bounded evidence shared by isolated live-mount observers and auxiliaries."""
import ctypes
import hashlib
import json
import os
from pathlib import PurePosixPath
import re
import stat

MOUNT_CHECKS = ("writer-stage", "writer-key", "writer-manifest", "reader-package",
                "reader-key", "reader-manifest", "reader-record")
STATE = PurePosixPath("/var/lib/vivac/custody-v1/users/65534/state")
PREFIX = ".vivac-openssh-preparing-"
CAPABILITIES = (2, 18, 19, 21)
CLONE_NEWNS = 0x00020000
MS_BIND = 4096
MS_PRIVATE = 1 << 18
LIMIT = 65536
PROCESS_FIELDS = {"Uid", "Gid", "NoNewPrivs", "Seccomp", "Seccomp_filters",
                  "CapEff", "CapPrm", "CapBnd", "CapInh", "CapAmb"}
MASTER_VOLUMES = {"/draft-build": True, "/launcher-build": True, "/decoy": True,
                  "/reports": False, "/var/lib/vivac": False}


class ProbeError(Exception):
    def __init__(self, message, err=None):
        super().__init__(message)
        self.errno = err


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("ascii")


def decode_json(payload, limit=8192):
    require(type(payload) in (str, bytes), "JSON input type differs")
    try:
        if type(payload) is str:
            payload = payload.encode("ascii")
        require(len(payload) <= limit, "JSON input exceeded its bound")
        def pairs(items):
            result = {}
            for key, value in items:
                require(key not in result, "JSON duplicate member was refused")
                result[key] = value
            return result
        return json.loads(payload.decode("ascii"), object_pairs_hook=pairs,
                          parse_constant=lambda value: (_ for _ in ()).throw(ProbeError("JSON constant was refused")))
    except (UnicodeError, ValueError, RecursionError) as error:
        raise ProbeError("JSON input was refused") from error


def hexadecimal(value, size):
    return type(value) is str and re.fullmatch("[0-9a-f]{" + str(size) + "}", value) is not None


def identifier(value):
    return type(value) is str and re.fullmatch("[0-7][0-9abcdefghjkmnpqrstvwxyz]{25}", value) is not None


def selected_target(check, selector):
    require(check in MOUNT_CHECKS and type(selector) is str, "mount selection differs")
    if check.startswith("writer-"):
        require(selector.startswith(PREFIX) and hexadecimal(selector[len(PREFIX):], 32), "writer stage selector differs")
    else:
        require(identifier(selector), "reader credential selector differs")
    parent = STATE / ("admissions" if check == "reader-record" else "packages") / selector
    member = {"writer-key": "key.openssh", "reader-key": "key.openssh",
              "writer-manifest": "manifest.json", "reader-manifest": "manifest.json",
              "reader-record": "record.json"}.get(check)
    return str(parent if member is None else parent / member), "directory" if member is None else "file"


def validate_target(check, target, kind):
    require(check in MOUNT_CHECKS and type(target) is str and type(kind) is str, "mount target type differs")
    prefix = str(STATE) + "/"
    require(target.startswith(prefix) and len(target) <= 512, "mount target escaped the fixed root")
    parts = target[len(prefix):].split("/")
    require(len(parts) in (2, 3), "mount target shape differs")
    expected, expected_kind = selected_target(check, parts[1])
    require(target == expected and kind == expected_kind, "mount target does not match its selection")
    return target


def validate_leaf(value):
    require(type(value) is dict and set(value) == {"device", "inode", "mode", "size", "sha256"},
            "mount member evidence fields differ")
    for field in ("device", "inode", "size"):
        require(type(value[field]) is int and value[field] > 0, "mount member numeric evidence differs")
    require(type(value["mode"]) is int and value["mode"] == 0o600 and value["size"] <= LIMIT
            and hexadecimal(value["sha256"], 64), "mount member evidence differs")


def validate_snapshot(value, kind):
    base = {"kind", "mount_id", "device", "inode", "mode", "size"}
    require(type(value) is dict and set(value) == base | ({"sha256"} if kind == "file" else {"members"}),
            "mount snapshot fields differ")
    require(value["kind"] == kind, "mount snapshot kind differs")
    for field in ("mount_id", "device", "inode", "size"):
        require(type(value[field]) is int and value[field] > 0, "mount snapshot numeric evidence differs")
    require(type(value["mode"]) is int and value["mode"] == (0o600 if kind == "file" else 0o700)
            and value["size"] <= LIMIT, "mount snapshot mode or size differs")
    if kind == "file":
        validate_leaf({field: value[field] for field in ("device", "inode", "mode", "size", "sha256")})
    else:
        require(type(value["members"]) is dict and set(value["members"]) == {"key.openssh", "manifest.json"},
                "mount directory membership differs")
        for member in value["members"].values():
            validate_leaf(member)
    return value


def same_object(left, right):
    return {key: value for key, value in left.items() if key != "mount_id"} == {
            key: value for key, value in right.items() if key != "mount_id"}


def validate_request(value):
    require(type(value) is dict and set(value) == {"version", "operation", "check", "target", "kind", "token", "original"},
            "mount request fields differ")
    require(type(value["version"]) is int and value["version"] == 1
            and value["operation"] in ("mount", "unmount") and hexadecimal(value["token"], 64),
            "mount request identity differs")
    validate_target(value["check"], value["target"], value["kind"])
    validate_snapshot(value["original"], value["kind"])
    return value


def validate_process(value, owner, capabilities):
    require(type(value) is dict and set(value) == PROCESS_FIELDS, "mount process fields differ")
    for field in ("Uid", "Gid"):
        require(type(value[field]) is str and value[field].split() == [str(owner)] * 4,
                "mount process credentials differ")
    require(value["NoNewPrivs"] == "1" and value["Seccomp"] == "2", "mount process protection differs")
    filters = value["Seccomp_filters"]
    require(filters is None or (type(filters) is str and filters.isdigit() and int(filters) > 0),
            "mount process filter count differs")
    for field in ("CapEff", "CapPrm", "CapBnd", "CapInh", "CapAmb"):
        require(type(value[field]) is str and re.fullmatch("[0-9a-f]{1,16}", value[field]) is not None,
                "mount process capability encoding differs")
        require(int(value[field], 16) == (0 if field in ("CapInh", "CapAmb") else capabilities),
                "mount process capabilities differ")


def permitted_master_mount(filesystem, mountpoint, readonly, fields):
    if (type(filesystem) is not str or type(mountpoint) is not str or type(readonly) is not bool
            or type(fields) is not list or not 0 < len(fields) <= 4
            or not all(type(field) is str and re.fullmatch("master:[0-9]+", field) for field in fields)):
        return False
    if (filesystem in ("cgroup", "cgroup2") and readonly is True
            and (mountpoint == "/sys/fs/cgroup" or mountpoint.startswith("/sys/fs/cgroup/"))
            and ".." not in mountpoint.split("/")):
        return True
    return filesystem == "ext4" and mountpoint in MASTER_VOLUMES and readonly is MASTER_VOLUMES[mountpoint]


def validate_namespaces(value):
    require(type(value) is dict and set(value) == {"target_euid", "target_mount_namespace", "helper_mount_namespace",
            "target_root_mount_id", "helper_root_mount_id", "target_process", "helper_process", "mount_propagation"},
            "mount namespace fields differ")
    require(type(value["target_euid"]) is int and value["target_euid"] == 65534, "mount target effective UID differs")
    for field in ("target_mount_namespace", "helper_mount_namespace"):
        require(type(value[field]) is str and re.fullmatch(r"mnt:\[[0-9]+\]", value[field]) is not None,
                "mount namespace encoding differs")
    require(value["target_mount_namespace"] != value["helper_mount_namespace"], "mount helper already shares target namespace")
    for field in ("target_root_mount_id", "helper_root_mount_id"):
        require(type(value[field]) is int and value[field] > 0, "mount namespace root evidence differs")
    validate_process(value["target_process"], 65534, 0)
    validate_process(value["helper_process"], 0, sum(1 << bit for bit in CAPABILITIES))
    propagation = value["mount_propagation"]
    require(type(propagation) is dict and set(propagation) == {"helper", "target"}, "mount propagation fields differ")
    for records in propagation.values():
        require(type(records) is list and len(records) <= 32, "mount propagation record count differs")
        for record in records:
            require(type(record) is dict and set(record) == {"type", "mountpoint", "readonly", "optional"},
                    "mount propagation member fields differ")
            require(permitted_master_mount(record["type"], record["mountpoint"], record["readonly"], record["optional"]),
                    "unsupported mount propagation was refused")
    return value


def validate_ack(value, request):
    validate_request(request)
    require(type(value) is dict and set(value) == {"version", "operation", "check", "token", "status", "before",
            "after", "namespaces", "setns_verified", "private_mount_verified"}, "mount acknowledgment fields differ")
    require(type(value["version"]) is int and value["version"] == 1 and value["status"] == "observed"
            and value["setns_verified"] is True and value["private_mount_verified"] is True,
            "mount acknowledgment status differs")
    require(all(value[field] == request[field] for field in ("operation", "check", "token")),
            "mount acknowledgment selection differs")
    for field in ("before", "after"):
        validate_snapshot(value[field], request["kind"])
    original = request["original"]
    if request["operation"] == "mount":
        require(value["before"] == original and same_object(value["after"], original)
                and value["after"]["mount_id"] != original["mount_id"], "mount acknowledgment did not cover the original object")
    else:
        require(value["after"] == original and same_object(value["before"], original)
                and value["before"]["mount_id"] != original["mount_id"], "unmount acknowledgment did not restore the original object")
    validate_namespaces(value["namespaces"])
    return value


def open_object(target, kind):
    return os.open(target, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
                   | (os.O_DIRECTORY if kind == "directory" else 0))


def leaf_snapshot(descriptor):
    info = os.fstat(descriptor)
    require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534 and info.st_nlink == 1
            and stat.S_IMODE(info.st_mode) == 0o600 and 0 < info.st_size <= LIMIT, "mount member object differs")
    raw = os.pread(descriptor, LIMIT + 1, 0)
    require(len(raw) == info.st_size and not os.listxattr(descriptor), "mount member contents or attributes differ")
    after = os.fstat(descriptor)
    require((info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns) ==
            (after.st_dev, after.st_ino, after.st_mode, after.st_size, after.st_mtime_ns, after.st_ctime_ns),
            "mount member changed during observation")
    return dict(device=info.st_dev, inode=info.st_ino, mode=stat.S_IMODE(info.st_mode), size=info.st_size,
                sha256=hashlib.sha256(raw).hexdigest())


def snapshot(target, kind, held_fd=None):
    descriptor = open_object(target, kind) if held_fd is None else held_fd
    try:
        if kind == "file":
            value = dict(kind=kind, mount_id=fd_mount_id(descriptor), **leaf_snapshot(descriptor))
        else:
            info = os.fstat(descriptor)
            require(stat.S_ISDIR(info.st_mode) and info.st_uid == info.st_gid == 65534
                    and stat.S_IMODE(info.st_mode) == 0o700 and not os.listxattr(descriptor)
                    and set(os.listdir(descriptor)) == {"key.openssh", "manifest.json"}, "mount directory object differs")
            members = {}
            for name in ("key.openssh", "manifest.json"):
                member = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=descriptor)
                try:
                    members[name] = leaf_snapshot(member)
                finally:
                    os.close(member)
            value = dict(kind=kind, mount_id=fd_mount_id(descriptor), device=info.st_dev, inode=info.st_ino,
                         mode=stat.S_IMODE(info.st_mode), size=info.st_size, members=members)
            after = os.fstat(descriptor)
            require((info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns) ==
                    (after.st_dev, after.st_ino, after.st_mode, after.st_size, after.st_mtime_ns, after.st_ctime_ns),
                    "mount directory changed during observation")
        return validate_snapshot(value, kind)
    finally:
        if held_fd is None:
            os.close(descriptor)


def read_status(pid):
    path = "/proc/{}/status".format(pid)
    result = {}
    with open(path, "rt", encoding="ascii") as stream:
        for line in stream:
            key, separator, value = line.partition(":")
            if separator:
                result[key] = value.strip()
    return result


def status_number(status, key):
    try:
        return int(status[key], 16) if key.startswith("Cap") else int(status[key])
    except (KeyError, ValueError) as error:
        raise ProbeError("process status field unavailable: " + key) from error


def capability_mask():
    return sum(1 << bit for bit in CAPABILITIES)


def process_evidence(status):
    result = {"Uid": status.get("Uid"), "Gid": status.get("Gid"),
              "NoNewPrivs": status.get("NoNewPrivs"), "Seccomp": status.get("Seccomp"),
              "Seccomp_filters": status.get("Seccomp_filters")}
    for key in ("CapEff", "CapPrm", "CapBnd", "CapAmb", "CapInh"):
        result[key] = status.get(key)
    return result


def validate_target_process():
    target = read_status(1)
    try:
        uid = [int(item) for item in target["Uid"].split()]
        gid = [int(item) for item in target["Gid"].split()]
    except (KeyError, ValueError) as error:
        raise ProbeError("target credentials unavailable") from error
    require(len(uid) == 4 and uid[1] == 65534, "target effective UID differs")
    require(len(gid) == 4 and gid[1] == 65534, "target effective GID differs")
    for key in ("CapEff", "CapPrm", "CapBnd", "CapAmb", "CapInh"):
        require(status_number(target, key) == 0, "target retained a capability")
    require(status_number(target, "NoNewPrivs") == 1, "target no-new-privileges is absent")
    require(status_number(target, "Seccomp") == 2, "target seccomp filter is absent")
    validate_seccomp_filters(target, "target")
    return target


def validate_helper_process():
    helper = read_status("self")
    try:
        uid = [int(item) for item in helper["Uid"].split()]
        gid = [int(item) for item in helper["Gid"].split()]
    except (KeyError, ValueError) as error:
        raise ProbeError("helper credentials unavailable") from error
    require(uid[1] == 0 and gid[1] == 0, "helper is not root")
    expected = capability_mask()
    for key in ("CapEff", "CapPrm", "CapBnd"):
        require(status_number(helper, key) == expected, "helper capability set differs: " + key)
    for key in ("CapAmb", "CapInh"):
        require(status_number(helper, key) == 0, "helper inherited a capability: " + key)
    require(status_number(helper, "NoNewPrivs") == 1, "helper no-new-privileges is absent")
    require(status_number(helper, "Seccomp") == 2, "helper seccomp filter is absent")
    validate_seccomp_filters(helper, "helper")
    return helper


def validate_seccomp_filters(status, label):
    filters = status.get("Seccomp_filters")
    try:
        require(filters is None or int(filters) >= 1, label + " seccomp filter count is zero")
    except ValueError as error:
        raise ProbeError(label + " seccomp filter count is malformed") from error


def decode_mount_field(value):
    result = bytearray()
    index = 0
    raw = value.encode("ascii")
    while index < len(raw):
        if raw[index:index + 1] == b"\\" and index + 3 < len(raw):
            digits = raw[index + 1:index + 4]
            if all(b"0" <= item <= b"7" for item in digits):
                result.append(int(digits, 8))
                index += 4
                continue
        result.append(raw[index])
        index += 1
    return result.decode("utf-8", errors="strict")


def mount_records(path):
    records = []
    with open(path, "rt", encoding="ascii") as stream:
        for line in stream:
            fields = line.rstrip("\n").split(" ")
            try:
                split = fields.index("-")
                require(split >= 6 and len(fields) >= split + 4, "mountinfo record is malformed")
                records.append({
                    "id": int(fields[0]),
                    "mountpoint": decode_mount_field(fields[4]),
                    "options": fields[5].split(","),
                    "optional": fields[6:split],
                    "filesystem": fields[split + 1],
                    "source": fields[split + 2],
                    "super_options": fields[split + 3].split(","),
                })
            except (ValueError, IndexError) as error:
                raise ProbeError("mountinfo record could not be parsed") from error
    require(bool(records), "mountinfo contained no records")
    return records


def validate_private_readonly_root(records, label):
    roots = [record for record in records if record["mountpoint"] == "/"]
    require(len(roots) == 1 and "ro" in roots[0]["options"], label + " root is not read-only")
    permitted = []
    unsupported = []
    for record in records:
        fields = record["optional"]
        if not fields:
            continue
        mountpoint = record["mountpoint"]
        readonly = "ro" in record["options"]
        if permitted_master_mount(record["filesystem"], mountpoint, readonly, fields):
            permitted.append({"type": record["filesystem"], "mountpoint": mountpoint,
                              "readonly": readonly, "optional": list(fields)})
            continue
        unsupported.append({"mountpoint": mountpoint, "filesystem": record["filesystem"],
                            "readonly": "ro" in record["options"], "optional": list(fields)})
    if unsupported:
        raise ProbeError(label + " has unsupported mount propagation: " + canonical(unsupported).decode("ascii"))
    return roots[0], permitted


def fd_mount_id(fd):
    path = "/proc/self/fdinfo/{}".format(fd)
    with open(path, "rt", encoding="ascii") as stream:
        for line in stream:
            key, separator, value = line.partition(":")
            if separator and key == "mnt_id":
                try:
                    mount_id = int(value.strip())
                except ValueError as error:
                    raise ProbeError("descriptor mount ID is malformed") from error
                require(mount_id > 0, "descriptor mount ID is invalid")
                return mount_id
    raise ProbeError("descriptor mount ID is unavailable")


def open_mount_namespace_inputs():
    root_fd = os.open("/proc/1/root", os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        mount_fd = os.open("/proc/1/ns/mnt", os.O_RDONLY | os.O_CLOEXEC)
    except BaseException:
        os.close(root_fd)
        raise
    return root_fd, mount_fd


def join_target_mount_namespace(root_fd, mount_fd, expected_namespace):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.setns.argtypes = (ctypes.c_int, ctypes.c_int)
    libc.setns.restype = ctypes.c_int
    if libc.setns(mount_fd, CLONE_NEWNS) != 0:
        code = ctypes.get_errno()
        raise ProbeError("setns mount namespace failed", code)
    os.fchdir(root_fd)
    os.chroot(".")
    os.chdir("/")
    require(os.readlink("/proc/self/ns/mnt") == expected_namespace,
            "helper did not enter target mount namespace")
    validate_private_readonly_root(mount_records("/proc/self/mountinfo"), "target after setns")


def libc_mount(source, target):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.mount.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
                           ctypes.c_ulong, ctypes.c_void_p)
    libc.mount.restype = ctypes.c_int
    if libc.mount(os.fsencode(source), os.fsencode(target), None, MS_BIND, None) != 0:
        code = ctypes.get_errno()
        raise ProbeError("bind mount failed", code)


def libc_umount(target):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.umount2.argtypes = (ctypes.c_char_p, ctypes.c_int)
    libc.umount2.restype = ctypes.c_int
    if libc.umount2(os.fsencode(target), 0) != 0:
        code = ctypes.get_errno()
        raise ProbeError("umount2 failed", code)


def make_mount_private(target):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.mount.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
                           ctypes.c_ulong, ctypes.c_void_p)
    libc.mount.restype = ctypes.c_int
    if libc.mount(None, os.fsencode(target), None, MS_PRIVATE, None) != 0:
        code = ctypes.get_errno()
        raise ProbeError("selected mount could not be made private", code)


def require_private_mount(target, mount_id):
    require(type(target) is str and type(mount_id) is int and mount_id > 0,
            "selected private mount identity differs")
    matches = [record for record in mount_records("/proc/self/mountinfo") if record["mountpoint"] == target]
    require(len(matches) == 1 and matches[0]["id"] == mount_id and matches[0]["optional"] == [],
            "selected mount is not the exact private mount")
    return matches[0]


def helper_same_namespace_checks():
    helper_status = validate_helper_process()
    target_status = validate_target_process()
    helper_mnt = os.readlink("/proc/self/ns/mnt")
    target_mnt = os.readlink("/proc/1/ns/mnt")
    require(helper_mnt != target_mnt, "helper already shares target mount namespace")
    require(os.readlink("/proc/self/ns/pid") == os.readlink("/proc/1/ns/pid"),
            "helper does not share target PID namespace")
    require(os.readlink("/proc/self/ns/user") == os.readlink("/proc/1/ns/user"),
            "helper and target user namespaces differ")
    helper_root, helper_propagation = validate_private_readonly_root(
        mount_records("/proc/self/mountinfo"), "helper")
    target_root, target_propagation = validate_private_readonly_root(
        mount_records("/proc/1/mountinfo"), "target")
    return {"target_euid": int(target_status["Uid"].split()[1]),
            "target_mount_namespace": target_mnt, "helper_mount_namespace": helper_mnt,
            "target_root_mount_id": target_root["id"], "helper_root_mount_id": helper_root["id"],
            "target_process": process_evidence(target_status),
            "helper_process": process_evidence(helper_status),
            "mount_propagation": {"helper": helper_propagation, "target": target_propagation}}

