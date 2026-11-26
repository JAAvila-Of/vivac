#!/usr/bin/env python3
"""Observe an attended draft in a disposable fixed Linux custody namespace."""
import argparse
import base64
import copy
import errno
import hashlib
import json
import os
from pathlib import Path
import selectors
import socket
import stat
import subprocess
import sys
import time

import generated_probe as generated
import heap_probe as heap
import lifecycle_probe as life
import openssh_probe as native
import rust_fixed_root_probe as fixed
import session_probe as session

require = generated.require
REAL = Path("/var/lib/vivac")
DECOY = Path("/decoy/var/lib/vivac")
USER = REAL / "custody-v1/users/65534"
PACKAGES = USER / "state/packages"
ADMISSIONS = USER / "state/admissions"
CASES = ("valid", "environment-decoy", "cwd-decoy", "mutate-key", "mutate-manifest",
         "replace-key", "extra-member", "missing-root", "mount-packages",
         "occupied-package", "occupied-admission")
MUTATIONS = {"mutate-key", "mutate-manifest", "replace-key", "extra-member"}
OCCUPIED = {"occupied-package", "occupied-admission"}
REFUSALS = {"missing-root", "mount-packages", *OCCUPIED}
OCCUPANT_BYTES = b"Synthetic existing custody slot.\n"
FALSE_FLAGS = {"package_published", "creation_admission", "custody_accredited",
               "producer_origin_checked", "native_binding_verified"}
RECORD_FIELDS = ("version", "installation_id", "creation_id", "device_id", "credential_id",
                 "public_key", "ciphertext_sha256", "manifest_sha256", "ciphertext_len",
                 "manifest_len", "creation_profile_revision", "secret_policy_revision")
PACKET_FIELDS = {"version", "stage", "pid", "record_canonical", "attended_confirmation_completed", *FALSE_FLAGS}
UNCERTAIN = []


def canonical(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=True).encode("ascii")


def decode(payload, bound):
    require(type(payload) is bytes and 0 < len(payload) <= bound, "public JSON size differs")
    return json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)


def parse_record(text):
    require(type(text) is str, "record type differs")
    raw = text.encode("ascii")
    value = decode(raw, 2048)
    require(type(value) is dict and tuple(value) == RECORD_FIELDS, "record fields or order differ")
    for field, expected in (("version", 2), ("creation_profile_revision", 1), ("secret_policy_revision", 1)):
        require(type(value[field]) is int and value[field] == expected, "record revision differs")
    for field in ("installation_id", "creation_id", "device_id", "credential_id"):
        require(fixed.installation(value[field]), "record identifier differs")
    for field in ("public_key", "ciphertext_sha256", "manifest_sha256"):
        require(fixed.hexadecimal(value[field], 64), "record public encoding differs")
    for field, bound in (("ciphertext_len", 65536), ("manifest_len", 2048)):
        require(type(value[field]) is int and 0 < value[field] <= bound, "record member size differs")
    require(raw == canonical(value), "record is not canonical")
    return value


def parse_packet(payload, pid, *, final=False):
    value = decode(payload, 8192)
    fields = PACKET_FIELDS | ({"draft_revalidated", "preparation_dropped"} if final else set())
    require(type(value) is dict and set(value) == fields, "draft packet fields differ")
    require(type(value["version"]) is int and value["version"] == 1
            and value["stage"] == ("final" if final else "draft"), "draft stage differs")
    require(type(value["pid"]) is int and value["pid"] == pid and pid > 0, "draft process differs")
    require(value["attended_confirmation_completed"] is True
            and all(value[field] is False for field in FALSE_FLAGS), "draft claims authority")
    parse_record(value["record_canonical"])
    if final:
        require(type(value["draft_revalidated"]) is bool and value["preparation_dropped"] is True,
                "draft final closure differs")
    return value


def check_hashes(record, key, manifest):
    require(record["ciphertext_len"] == len(key) and record["manifest_len"] == len(manifest)
            and record["ciphertext_sha256"] == hashlib.sha256(key).hexdigest()
            and record["manifest_sha256"] == hashlib.sha256(manifest).hexdigest(), "draft bytes differ")


def validate_observation(value, case):
    require(type(value) is dict and case in CASES and value.get("case") == case
            and type(value.get("version")) is int and value["version"] == 1
            and value.get("experiment") == "rooted-creation-draft" and value.get("status") == "observed",
            "draft observation identity differs")
    require(all(value.get(field) is False for field in FALSE_FLAGS), "observation claims authority")
    for field in ("process_exit_observed", "terminal_restored", "standard_streams_clean",
                  "helper_unchanged", "sources_unchanged", "root_binding_unchanged"):
        require(value.get(field) is True, "draft observation evidence missing")
    for field in ("helper_sha256", "observer_sha256", "decoy_binding_sha256"):
        require(fixed.hexadecimal(value.get(field), 64), "observation provenance differs")
    if case == "missing-root":
        require(value.get("missing_root_observed") is True and value.get("real_binding_sha256") is None,
                "missing fixed root evidence differs")
    else:
        require(fixed.hexadecimal(value.get("real_binding_sha256"), 64), "fixed binding digest differs")
    if case in REFUSALS:
        require("record_canonical" not in value and value.get("refusal_before_disclosure") is True,
                "refusal observation differs")
        if case == "mount-packages":
            require(value.get("mount_boundary_observed") is True, "mount boundary unobserved")
        if case in OCCUPIED:
            require(value.get("occupied_slot_observed") is True and value.get("occupied_slot_preserved") is True,
                    "occupied slot evidence differs")
    else:
        record = parse_record(value.get("record_canonical"))
        require(record["installation_id"] == value.get("selected_installation_id")
                and value.get("draft_revalidated") is (case not in MUTATIONS), "draft result differs")
        for field in ("process_identity_verified", "private_terminal_observed", "secret_boundaries_checked",
                      "original_package_verified", "preparation_dropped", "admissions_empty", "final_slot_absent"):
            require(value.get(field) is True, "draft evidence missing")
        require(value.get("mutation_observed") is (case in MUTATIONS)
                and value.get("invalidated_stage_preserved") is (case in MUTATIONS), "mutation outcome differs")
    return value


def metadata(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid,
            info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def directory(path, uid, mode):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    info = os.fstat(fd)
    require(info.st_uid == info.st_gid == uid and stat.S_IMODE(info.st_mode) == mode,
            "directory ownership or mode differs")
    require(not os.listxattr(fd), "directory extended attributes differ")
    return fd


def retain_binding(root):
    # Check every ancestor independently; the user-owned state is below this chain.
    current = Path("/")
    for part in (root / "custody-v1/users/65534").parts[1:]:
        current /= part
        info = current.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == info.st_gid == 0
                and stat.S_IMODE(info.st_mode) & 0o022 == 0, "administrative root chain differs")
    parent = fixed.binding_parent(root)
    fd, data, iid, evidence = fixed.read_binding_at(parent)
    return parent, fd, data, iid, evidence


def revalidate_binding(held):
    parent, fd, raw, iid, evidence = held
    require(os.pread(fd, 513, 0) == raw, "retained binding bytes changed")
    fresh, data, selected, current = fixed.read_binding_at(parent)
    os.close(fresh)
    require(data == raw and selected == iid and current == evidence, "binding snapshot changed")


def mount_boundary(raw):
    target = PACKAGES.as_posix()
    entries = {}
    for line in raw.splitlines():
        fields = line.split()
        require(len(fields) >= 10, "mount metadata malformed")
        entries[fields[4]] = fields
    child = entries.get(target)
    require(child is not None, "packages mount boundary absent")
    parents = [fields for path, fields in entries.items()
               if target.startswith(path.rstrip("/") + "/") and path != target]
    require(parents, "parent mount absent")
    parent = max(parents, key=lambda fields: len(fields[4]))
    require(child[0] != parent[0] and child[2] == parent[2], "same-device mount boundary differs")
    return True


def require_missing_installation(root=REAL):
    info = root.lstat()
    require(stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode),
            "fixed root mountpoint differs")
    # The administrator leaves the real volume mounted, without installing custody.
    anchor = root / "custody-v1"
    try:
        anchor.lstat()
    except FileNotFoundError:
        require(os.listdir(root) == [], "uninstalled root contains unexpected entries")
        return True
    raise generated.ProbeError("fixed installation anchor unexpectedly exists")


def snapshot_package(packages, record, secret, *, preserved_names=()):
    from creation_namespace_probe import select_stage
    members = os.listdir(packages)
    stage_name = select_stage(members, preserved_names)
    stage = os.open(stage_name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=packages)
    info = os.fstat(stage)
    require(info.st_uid == info.st_gid == 65534 and stat.S_IMODE(info.st_mode) == 0o700
            and os.fstat(packages).st_dev == info.st_dev and info.st_nlink == 2, "stage metadata differs")
    require(set(os.listdir(stage)) == {"key.openssh", "manifest.json"}, "draft members differ")
    held = {}
    for name, limit in (("key.openssh", 65536), ("manifest.json", 2048)):
        fd = os.open(name, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=stage)
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534
                and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
                and info.st_dev == os.fstat(stage).st_dev and 0 < info.st_size <= limit,
                "draft file metadata differs")
        raw = os.pread(fd, limit + 1, 0)
        require(len(raw) == info.st_size and secret not in raw and not os.listxattr(fd), "draft file boundary differs")
        require(metadata(os.stat(name, dir_fd=stage, follow_symlinks=False)) == metadata(info), "draft file changed")
        held[name] = (fd, raw, metadata(info))
    key, manifest = held["key.openssh"][1], held["manifest.json"][1]
    check_hashes(record, key, manifest)
    parsed = native.parse_key_bytes(key)
    manifest_value = decode(manifest, 2048)
    require(set(manifest_value) == {"version", "profile", "device_id", "credential_id", "public_key", "key_sha256", "key_size"},
            "manifest fields differ")
    require(type(manifest_value["version"]) is int and manifest_value["version"] == 1
            and manifest_value["profile"] == "openssh-ed25519-aes256gcm-bcrypt-v1"
            and type(manifest_value["key_size"]) is int and manifest_value["key_size"] == len(key)
            and manifest_value["key_sha256"] == record["ciphertext_sha256"], "manifest profile differs")
    require(parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["kdf"] == "bcrypt"
            and parsed["rounds"] == 64 and parsed["public_key"].hex() == record["public_key"], "native header differs")
    require(all(manifest_value[field] == record[field] for field in ("device_id", "credential_id", "public_key")),
            "manifest identity differs")
    return stage_name, stage, metadata(os.fstat(stage)), held


def mutate(snapshot, case):
    name, stage, _, held = snapshot
    if case == "extra-member":
        fd = os.open("foreign", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=stage)
        try:
            require(os.write(fd, b"foreign") == 7, "foreign write differs")
        finally:
            os.close(fd)
        require(set(os.listdir(stage)) == {"key.openssh", "manifest.json", "foreign"}, "foreign member absent")
        return
    member = "manifest.json" if case == "mutate-manifest" else "key.openssh"
    fd, raw, before = held[member]
    if case == "replace-key":
        replacement = os.open("replacement", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=stage)
        try:
            require(os.write(replacement, raw) == len(raw), "replacement write differs")
            os.fsync(replacement)
        finally:
            os.close(replacement)
        os.rename("replacement", member, src_dir_fd=stage, dst_dir_fd=stage)
        current = os.stat(member, dir_fd=stage, follow_symlinks=False)
        require(current.st_ino != before[1] and os.fstat(fd).st_nlink == 0, "replacement inode unchanged")
        fresh = os.open(member, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=stage)
        try:
            require(os.read(fresh, 65537) == raw and stat.S_IMODE(current.st_mode) == 0o600, "replacement bytes differ")
        finally:
            os.close(fresh)
    else:
        if case == "mutate-manifest":
            offset = raw.index(b'"public_key":"') + len(b'"public_key":"')
            changed = b"0" if raw[offset] != ord("0") else b"1"
            expected = raw[:offset] + changed + raw[offset + 1:]
        else:
            expected = changed_ciphertext(raw)
        require(len(expected) == len(raw) and os.pwrite(fd, expected, 0) == len(raw), "mutation write differs")
        os.fsync(fd)
        after = os.pread(fd, len(raw) + 1, 0)
        require(after == expected and after != raw
                and os.fstat(fd).st_ino == before[1], "in-place mutation differs")


def changed_ciphertext(raw):
    parsed = native.parse_key_bytes(raw)
    decoded = bytearray(parsed["decoded"])
    reader = native.Reader(decoded)
    reader.take(15)
    for _ in range(3):
        reader.string()
    require(reader.number() == 1, "mutation key count differs")
    reader.string()
    size = reader.number()
    require(size > 0, "mutation ciphertext absent")
    decoded[reader.position] ^= 1
    encoded = base64.b64encode(decoded)
    lines = raw.splitlines(keepends=True)
    position = 0
    result = [lines[0]]
    for line in lines[1:-1]:
        content = line.rstrip(b"\r\n")
        result.append(encoded[position:position + len(content)] + line[len(content):])
        position += len(content)
    require(position == len(encoded), "mutation envelope differs")
    result.append(lines[-1])
    changed = b"".join(result)
    check = native.parse_key_bytes(changed)
    require(check["public_key"] == parsed["public_key"] and check["cipher"] == parsed["cipher"],
            "ciphertext mutation changed header")
    return changed


def stage_evidence(stage):
    result = {}
    for member in os.listdir(stage):
        fd = os.open(member, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=stage)
        try:
            info = os.fstat(fd)
            require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534
                    and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
                    and 0 < info.st_size <= 65536, "stage member evidence differs")
            result[member] = (metadata(info), os.pread(fd, 65537, 0))
        finally:
            os.close(fd)
    return metadata(os.fstat(stage)), result


def occupy_slot(parent, credential):
    os.mkdir(credential, mode=0o700, dir_fd=parent)
    slot = os.open(credential, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    member = os.open("existing", os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o600, dir_fd=slot)
    require(os.write(member, OCCUPANT_BYTES) == len(OCCUPANT_BYTES), "occupied member write differs")
    os.fsync(member)
    evidence = stage_evidence(slot)
    require(os.listdir(parent) == [credential] and set(evidence[1]) == {"existing"}
            and evidence[1]["existing"][1] == OCCUPANT_BYTES
            and os.fstat(slot).st_uid == os.fstat(slot).st_gid == 65534
            and stat.S_IMODE(os.fstat(slot).st_mode) == 0o700 and os.fstat(slot).st_nlink == 2,
            "occupied slot preparation differs")
    return slot, member, evidence


def preserve_occupied_slot(parent, other, credential, occupant):
    slot, member, evidence = occupant
    require(os.listdir(parent) == [credential] and os.listdir(other) == [], "occupied parent membership changed")
    linked = os.stat(credential, dir_fd=parent, follow_symlinks=False)
    require(metadata(linked) == evidence[0] and metadata(os.fstat(slot)) == evidence[0]
            and stage_evidence(slot) == evidence, "occupied slot changed")
    require(metadata(os.fstat(member)) == evidence[1]["existing"][0]
            and os.pread(member, len(OCCUPANT_BYTES) + 1, 0) == OCCUPANT_BYTES,
            "retained occupied member changed")


def observe(args, report, bindings):
    import pty
    import termios
    master, slave = pty.openpty()
    os.fchmod(slave, 0o600)
    initial = copy.deepcopy(termios.tcgetattr(slave))
    require(os.fstat(slave).st_uid == 65534 and stat.S_IMODE(os.fstat(slave).st_mode) == 0o600, "private PTY differs")
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    env = life.private_env(Path("/tmp"))
    cwd = "/"
    if args.case == "environment-decoy":
        env.update(HOME="/decoy", XDG_DATA_HOME="/decoy/var/lib", VIVAC_HOME=str(DECOY), VIVAC_ROOT=str(DECOY))
    if args.case == "cwd-decoy":
        cwd = str(DECOY)
    device, credential = native.identifier(), native.identifier()
    command = [str(args.writer), "--fixed-root-record-draft", device, credential, "--observer-fd", str(peer.fileno())]
    process = None
    pidfd = None
    transcript, stdout, stderr = bytearray(), bytearray(), bytearray()
    secret = None
    snapshot = None
    changed_evidence = None
    packages = admissions = None
    occupant = None
    completed = False
    try:
        if args.case not in REFUSALS or args.case in OCCUPIED:
            packages = directory(PACKAGES, 65534, 0o700)
            admissions = directory(ADMISSIONS, 65534, 0o700)
            require(os.listdir(packages) == [] and os.listdir(admissions) == [], "initial store is not empty")
            if args.case in OCCUPIED:
                occupant = occupy_slot(packages if args.case == "occupied-package" else admissions, credential)
                report["occupied_slot_observed"] = True
        process = subprocess.Popen(command, stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            preexec_fn=lambda: generated.child_setup(slave), pass_fds=(peer.fileno(),), env=env, cwd=cwd, close_fds=True)
        peer.close()
        pidfd = os.pidfd_open(process.pid)
        start = life.process_start(process.pid)
        deadline = time.monotonic() + 25
        ack = confirmed = False
        draft = final = None
        with selectors.DefaultSelector() as selector:
            for stream, label in ((master, "terminal"), (process.stdout, "stdout"), (process.stderr, "stderr"), (channel, "packet")):
                os.set_blocking(stream if type(stream) is int else stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, label)
            while True:
                require(time.monotonic() < deadline, "draft observation deadline expired")
                events = selector.select(0.02)
                for key, _ in events:
                    if key.data == "packet":
                        payload, ancillary, flags, _ = channel.recvmsg(8193)
                        require(not ancillary and flags == 0 and len(payload) <= 8192, "packet boundary differs")
                        if not payload:
                            selector.unregister(channel)
                            continue
                        require(args.case not in REFUSALS and confirmed, "packet preceded confirmation")
                        if draft is None:
                            require(life.process_start(process.pid) == start and not life.pidfd_ready(pidfd, 0), "draft process changed")
                            draft = parse_packet(payload, process.pid)
                            record = parse_record(draft["record_canonical"])
                            require(record["installation_id"] == bindings[0][3]
                                    and record["installation_id"] != bindings[1][3]
                                    and record["device_id"] == device and record["credential_id"] == credential,
                                    "fixed draft origin differs")
                            require(not stdout and not stderr, "standard output preceded final")
                            generated.proc_secret_absent(process.pid, secret)
                            snapshot = snapshot_package(packages, record, secret)
                            require(not os.listdir(admissions) and credential not in os.listdir(packages), "draft was published")
                            report.update(original_package_verified=True, record_canonical=draft["record_canonical"],
                                          selected_installation_id=record["installation_id"])
                            if args.case in MUTATIONS:
                                mutate(snapshot, args.case)
                                changed_evidence = stage_evidence(snapshot[1])
                            report["mutation_observed"] = args.case in MUTATIONS
                            for held in bindings:
                                revalidate_binding(held)
                            channel.sendall(b"V")
                        else:
                            require(final is None, "extra final packet")
                            final = parse_packet(payload, process.pid, final=True)
                            require(all(final[field] == draft[field] for field in PACKET_FIELDS - {"stage"})
                                    and final["draft_revalidated"] is (args.case not in MUTATIONS), "final changed draft")
                        continue
                    try:
                        chunk = os.read(key.fd, 4096)
                    except OSError as error:
                        if key.data != "terminal" or error.errno != errno.EIO:
                            raise
                        chunk = b""
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    target, limit = {"terminal": (transcript, 32768), "stdout": (stdout, 8192), "stderr": (stderr, 1024)}[key.data]
                    target.extend(chunk)
                    require(len(target) <= limit, "observer output bound exceeded")
                    if args.case not in REFUSALS:
                        require(not stderr and (not stdout or draft is not None), "premature standard stream output")
                if not ack and generated.ACK_PROMPT in transcript:
                    require(args.case not in REFUSALS, "refusal unexpectedly prompted")
                    credentials = life.agent_credentials(process.pid)
                    status = Path("/proc", str(process.pid), "status").read_text(encoding="ascii")
                    require(credentials["Uid"] == credentials["Gid"] == [65534] * 4
                            and credentials["PPid"] == [os.getpid()] and credentials["NoNewPrivs"] == [1]
                            and all(int(line.split()[1], 16) == 0 for line in status.splitlines() if line.startswith("CapEff:"))
                            and life.executable(process.pid) == str(args.writer)
                            and os.getsid(process.pid) == process.pid, "retained process identity differs")
                    report["process_identity_verified"] = True
                    require(not termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ECHONL)
                            and os.tcgetpgrp(master) == process.pid, "acknowledgment terminal differs")
                    generated.send(master, generated.ACK + b"\n", deadline)
                    ack = True
                if secret is None:
                    secret = generated.parse_disclosure(transcript)
                if secret is not None and not confirmed:
                    require(ack and not termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ECHONL)
                            and os.tcgetpgrp(master) == process.pid, "confirmation terminal differs")
                    generated.proc_secret_absent(process.pid, secret)
                    generated.send(master, secret + b"\n", deadline)
                    confirmed = True
                    report["private_terminal_observed"] = True
                if process.poll() is not None and not events:
                    break
        code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        require(life.pidfd_ready(pidfd, 0), "helper exit unobserved")
        require(termios.tcgetattr(slave) == initial, "terminal not restored")
        report.update(process_exit_observed=True, terminal_restored=True)
        if args.case in REFUSALS:
            require(code == 1 and not stdout and stderr == generated.REFUSAL_MESSAGE and not transcript
                    and draft is None and final is None and secret is None, "early refusal differs")
            report["refusal_before_disclosure"] = True
            if args.case in OCCUPIED:
                parent, other = (packages, admissions) if args.case == "occupied-package" else (admissions, packages)
                preserve_occupied_slot(parent, other, credential, occupant)
                report["occupied_slot_preserved"] = True
        else:
            require(code == 0 and final is not None and not stderr and stdout == canonical(final) + b"\n",
                    "final standard output or exit differs")
            require(secret is not None and secret not in stdout and secret not in stderr
                    and transcript.count(secret) == 1, "secret output boundary differs")
            require(not os.listdir(admissions) and credential not in os.listdir(packages), "final publication appeared")
            name, stage, _, held = snapshot
            mutated = args.case in MUTATIONS
            require(os.listdir(packages) == ([name] if mutated else []), "final stage disposition differs")
            if mutated:
                require(os.stat(name, dir_fd=packages, follow_symlinks=False).st_ino == os.fstat(stage).st_ino,
                        "invalidated stage replaced")
                require(stage_evidence(stage) == changed_evidence, "invalidated stage changed after revalidation")
                for member in os.listdir(stage):
                    fd = os.open(member, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=stage)
                    try:
                        require(secret not in os.read(fd, 65537), "secret appeared in retained stage")
                    finally:
                        os.close(fd)
            else:
                require(os.listdir(stage) == [] and os.fstat(stage).st_nlink == 0,
                        "dropped stage remains linked")
                for fd, raw, _ in held.values():
                    require(os.fstat(fd).st_nlink == 0 and os.pread(fd, 65537, 0) == raw,
                            "original dropped member changed")
            report.update(draft_revalidated=final["draft_revalidated"], preparation_dropped=True,
                          secret_boundaries_checked=True, admissions_empty=True, final_slot_absent=True,
                          invalidated_stage_preserved=mutated)
        report["standard_streams_clean"] = True
        completed = True
    finally:
        generated.wipe(secret)
        generated.wipe(transcript)
        if not completed:
            # Only the directly retained child can be stopped. Keep uncertain handles and stage names.
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            UNCERTAIN.append((process, pidfd, master, slave, channel, peer, packages, admissions, snapshot, occupant))
        else:
            for fd in (pidfd, master, slave, packages, admissions):
                if fd is not None:
                    os.close(fd)
            if snapshot is not None:
                os.close(snapshot[1])
                for fd, _, _ in snapshot[3].values():
                    os.close(fd)
            if occupant is not None:
                os.close(occupant[0])
                os.close(occupant[1])
            for stream in (channel, peer, process.stdout, process.stderr):
                stream.close()


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "report", "expected-setup"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--case", choices=CASES, required=True)
    args = parser.parse_args()
    report = dict(version=1, experiment="rooted-creation-draft", case=args.case, status="incomplete",
                  **dict.fromkeys(FALSE_FLAGS, False), process_exit_observed=False)
    bindings = []
    try:
        require(all(path.is_absolute() for path in (args.writer, args.report, args.expected_setup)), "absolute paths required")
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1"
                and os.getuid() == os.geteuid() == os.getgid() == os.getegid() == 65534, "disposable UID required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        report["resource_limits"] = life.require_resources(expected_script="creation_draft_probe.py")
        session.require_report_directory(args.report)
        setup = decode(args.expected_setup.read_bytes(), 32768)
        require(type(setup) is dict and type(setup.get("version")) is int and setup["version"] == 1
                and setup.get("status") == "observed" and type(setup.get("result")) is dict,
                "setup observation differs")
        expected = setup["result"]
        require(expected["status"] == "observed", "setup was not observed")
        decoy = retain_binding(DECOY)
        if args.case == "missing-root":
            require_missing_installation()
            report.update(missing_root_observed=True, real_binding_sha256=None)
            bindings = [decoy]
        else:
            real = retain_binding(REAL)
            require(real[3] == expected["real_installation_id"] and real[4]["sha256"] == expected["real_binding_sha256"],
                    "fixed binding differs from setup")
            report["real_binding_sha256"] = real[4]["sha256"]
            bindings = [real, decoy]
        require(decoy[3] == expected["decoy_installation_id"] and decoy[4]["sha256"] == expected["decoy_binding_sha256"],
                "decoy binding differs from setup")
        report["decoy_binding_sha256"] = decoy[4]["sha256"]
        if args.case == "mount-packages":
            report["mount_boundary_observed"] = mount_boundary(Path("/proc/self/mountinfo").read_text(encoding="ascii"))
        sources = {"helper": args.writer, "observer": Path(__file__), "generated": Path(generated.__file__),
                   "fixed": Path(fixed.__file__), "lifecycle": Path(life.__file__), "openssh": Path(native.__file__)}
        hashes = {name + "_sha256": generated.file_sha256(path) for name, path in sources.items()}
        report.update(hashes)
        observe(args, report, bindings)
        for held in bindings:
            revalidate_binding(held)
        require(all(generated.file_sha256(path) == hashes[name + "_sha256"] for name, path in sources.items()),
                "probe sources changed")
        report.update(helper_unchanged=True, sources_unchanged=True, root_binding_unchanged=True, status="observed")
        validate_observation(report, args.case)
        for held in bindings:
            os.close(held[0])
            os.close(held[1])
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", failure={"kind": type(error).__name__})
        UNCERTAIN.append(bindings)
        try:
            life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        except OSError:
            pass
        print("Rooted creation draft experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
