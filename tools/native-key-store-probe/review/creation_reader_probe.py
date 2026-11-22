"""Observe a fresh, non-writing Rust reader after real disposable publication."""
import hashlib
import os
from pathlib import Path
import selectors
import socket
import stat
import subprocess
import time

import creation_draft_probe as draft
import generated_probe as generated
import lifecycle_probe as life

RETAINED_MUTATIONS = ("publish-invalidate-retained-record", "publish-invalidate-retained-package")
MUTATIONS = ("record-in-place", "record-replacement", "key-in-place",
             "key-replacement", "manifest-replacement", "package-replacement")
MOUNT_CHECKS = ("mount-package", "mount-key", "mount-manifest", "mount-record")
RESTORE_CHECKS = ("restore-record-replacement", "restore-key-replacement", "restore-package-replacement")
BASE_CHECKS = ("stable", *MUTATIONS, "unrelated-publication", *MOUNT_CHECKS, *RESTORE_CHECKS)
LAUNCH_CHECKS = ("launch-validate", *("launch-" + check for check in (*MUTATIONS, *RESTORE_CHECKS)))
CHECKS = (*BASE_CHECKS, *LAUNCH_CHECKS)
LAUNCH_FLAGS = {"launch_prepared", "launch_snapshot_revalidated", "launch_failure_sticky",
                "launch_reservation_contention_rejected", "launch_reservation_reacquired"}
LAUNCH_ROLE_FLAGS = {"admitted_openssh_roles_verified", "keeper_restricted_successor_verified"}
LAUNCH_ERRORS = {"launch_validation_error"}

def semantic_check(check):
    return "stable" if check == "launch-validate" else check.removeprefix("launch-")

FLAGS = {"creation_admission", "custody_accredited", "producer_origin_checked", "durable_ack_inferred"}
MATERIAL_FLAGS = {"admitted_material_prepared", "admitted_material_validated",
                  "admitted_reservation_validated", "admitted_reservation_contention_rejected",
                  "admitted_reservation_reacquired"}
LIFECYCLE_FLAGS = {"material_snapshot_revalidated", "reserved_snapshot_revalidated",
                   "material_failure_sticky", "reserved_failure_sticky", "current_namespace_revalidated"}
LIFECYCLE_ERRORS = {"material_validation_error", "reserved_validation_error"}
BASE = {"version", "stage", "pid", "credential_id", "initial_opened", "initial_error",
        "record_canonical", "snapshot_revalidated", "validation_error",
        "repeat_snapshot_revalidated", "repeat_validation_error", "read_only", *FLAGS, *MATERIAL_FLAGS,
        *LIFECYCLE_FLAGS, *LIFECYCLE_ERRORS, *LAUNCH_FLAGS, *LAUNCH_ERRORS}
ERRORS = {"none", "not-found", "invalid-data", "crosses-devices", "symlink", "permission-denied", "other"}
REPORT_FIELDS = {"check", "reader_pid", "reader_starttime", "reader_identity", "accepted", "initial_error",
                 "record_canonical", "revalidated", "validation_error", "mutation_observed",
                 "repeat_snapshot_revalidated", "repeat_validation_error",
                 "reader_state_unchanged", "filesystem_observation", "process_exit_observed", "helper_unchanged", "source_unchanged",
                 "snapshot_released", "unrelated_publication_observed", "reader_sha256", "source_sha256", "state_before_sha256",
                 "state_after_sha256", "retained_original_verified", *FLAGS, *MATERIAL_FLAGS,
                 *LIFECYCLE_FLAGS, *LIFECYCLE_ERRORS, *LAUNCH_FLAGS, *LAUNCH_ERRORS, "restoration_observed", "restoration_evidence"}
READER = Path("/launcher-build/launcher189/debug/rust-launcher-probe")
UNCERTAIN = []
require = generated.require


def validate_lifecycle(value, opened, revalidated, error, stage, launch=False):
    require(all(type(value[field]) is bool for field in LIFECYCLE_FLAGS),
            "reader material lifecycle types differ")
    expected = stage != "opened" and revalidated
    require(value["material_snapshot_revalidated"] is expected
            and value["reserved_snapshot_revalidated"] is (expected and not launch),
            "reader independent material revalidation differs")
    expected_error = "not-run" if stage == "opened" else error
    require(all(type(value[field]) is str and value[field] == ("not-required" if launch and field == "reserved_validation_error" else expected_error) for field in LIFECYCLE_ERRORS),
            "reader independent material rejection differs")
    sticky = stage == "final" and opened and not revalidated
    require(value["material_failure_sticky"] is sticky and value["reserved_failure_sticky"] is (sticky and not launch),
            "reader material failure was not retained")
    if stage != "final" or not opened:
        require(value["current_namespace_revalidated"] is False, "reader namespace check preceded its barrier")
    elif revalidated:
        require(value["current_namespace_revalidated"] is True, "reader stable namespace cannot be reopened")


def validate_launch(value, opened, revalidated, error, stage, launch):
    expected = dict(launch_prepared=launch and opened,
        launch_snapshot_revalidated=launch and stage != "opened" and revalidated,
        launch_failure_sticky=launch and stage == "final" and opened and not revalidated,
        launch_reservation_contention_rejected=launch and stage == "final" and revalidated,
        launch_reservation_reacquired=launch and stage == "final" and revalidated)
    require(all(type(value[field]) is bool and value[field] is expected[field] for field in LAUNCH_FLAGS),
            "reader launch evidence differs")
    if launch:
        require(all(value[field] is opened for field in LAUNCH_ROLE_FLAGS),
                "reader admitted OpenSSH roles or keeper successor differ")
    expected_error = "not-required" if not launch else "not-run" if stage == "opened" else error
    require(value["launch_validation_error"] == expected_error, "reader launch rejection differs")


def validate_restoration(value, check):
    restored = semantic_check(check) in RESTORE_CHECKS
    require(value["restoration_observed"] is restored, "reader restoration evidence differs")
    evidence = value["restoration_evidence"]
    if not restored:
        require(evidence is None, "reader claims an unrequested restoration")
        return
    identities = ("original_identity", "replacement_identity", "restored_identity", "retained_identity")
    hashes = ("original_sha256", "replacement_sha256", "restored_sha256", "retained_sha256")
    require(type(evidence) is dict and set(evidence) == {"check", *identities, *hashes}
            and evidence["check"] == check, "reader restoration fields differ")
    for field in identities:
        require(type(evidence[field]) is list and len(evidence[field]) == 2
                and all(type(item) is int and item > 0 for item in evidence[field]),
                "reader restoration identity differs")
    require(evidence["original_identity"] == evidence["restored_identity"] == evidence["retained_identity"]
            and evidence["replacement_identity"] != evidence["original_identity"]
            and evidence["replacement_identity"][0] == evidence["original_identity"][0],
            "reader restoration did not retain the original object")
    require(all(draft.fixed.hexadecimal(evidence[field], 64) for field in hashes)
            and len({evidence[field] for field in hashes}) == 1,
            "reader restoration changed retained content")
    require(value["current_namespace_revalidated"] is True,
            "restored reader namespace could not be freshly admitted")


def parse_packet(payload, pid, credential, stage, identity=None, *, check="stable"):
    require(check in CHECKS, "reader packet check differs")
    launch = check in LAUNCH_CHECKS
    require(stage in ("opened", "validated", "final"), "reader protocol stage differs")
    value = draft.decode(payload, 8192)
    expected_fields = BASE | (LAUNCH_ROLE_FLAGS if launch else set())
    require(type(value) is dict and set(value) == expected_fields | ({"snapshot_released"} if stage == "final" else set()),
            "reader packet fields differ")
    require(type(pid) is int and pid > 0 and type(value["pid"]) is int and value["pid"] == pid
            and type(value["version"]) is int and value["version"] == 1 and value["stage"] == stage,
            "reader packet identity differs")
    require(draft.fixed.installation(credential) and value["credential_id"] == credential,
            "reader credential selector differs")
    require(value["read_only"] is True and all(value[field] is False for field in FLAGS),
            "reader claims authority or historical acknowledgment")
    require(type(value["initial_opened"]) is bool and type(value["snapshot_revalidated"]) is bool
            and type(value["initial_error"]) is str and value["initial_error"] in ERRORS
            and type(value["validation_error"]) is str
            and type(value["repeat_snapshot_revalidated"]) is bool
            and type(value["repeat_validation_error"]) is str, "reader result types differ")
    require(not launch or value["initial_opened"], "launch requires a published admission")
    if value["initial_opened"]:
        require(value["initial_error"] == "none", "opened reader also claims an error")
        draft.parse_record(value["record_canonical"])
    else:
        require(value["initial_error"] != "none" and value["record_canonical"] is None,
                "unavailable reader claims a resident record")
    if stage == "opened":
        require(value["snapshot_revalidated"] is False and value["validation_error"] == "not-run",
                "reader revalidation preceded its barrier")
    elif value["initial_opened"]:
        require(value["validation_error"] in ERRORS
                and value["snapshot_revalidated"] is (value["validation_error"] == "none"),
                "reader revalidation outcome differs")
    else:
        require(value["snapshot_revalidated"] is False and value["validation_error"] == "unavailable",
                "unavailable reader claims revalidation")
    if stage != "final":
        require(value["repeat_snapshot_revalidated"] is False and value["repeat_validation_error"] == "not-run",
                "reader repeated validation preceded its barrier")
    else:
        repeated = value["initial_opened"] and value["snapshot_revalidated"]
        repeat_error = "none" if repeated else "invalid-data" if value["initial_opened"] else "unavailable"
        require(value["repeat_snapshot_revalidated"] is repeated and value["repeat_validation_error"] == repeat_error,
                "reader repeated validation outcome differs")
    material_prepared = not launch and stage == "final" and value["initial_opened"] and value["snapshot_revalidated"]
    require(all(value[field] is material_prepared for field in MATERIAL_FLAGS),
            "reader admitted material evidence differs")
    validate_lifecycle(value, value["initial_opened"], value["snapshot_revalidated"],
                       value["validation_error"], stage, launch)
    validate_launch(value, value["initial_opened"], value["snapshot_revalidated"], value["validation_error"], stage, launch)
    if identity is not None:
        require(all(value[field] == identity[field] for field in
                    ("pid", "credential_id", "initial_opened", "initial_error", "record_canonical")),
                "reader refreshed its retained identity")
    if stage == "final":
        if identity is not None and identity.get("stage") == "validated":
            require(all(value[field] == identity[field] for field in ("snapshot_revalidated", "validation_error")),
                    "reader replaced its first validation outcome")
        require(value["snapshot_released"] is True, "reader did not release its retained snapshot")
    return value


def validate_identity(value, parent):
    require(type(value) is dict and set(value) == {"credentials", "executable_observation"}
            and value["executable_observation"] == "verified", "fresh reader executable differs")
    credentials = value["credentials"]
    require(type(credentials) is dict and set(credentials) == {"Uid", "Gid", "NoNewPrivs", "PPid"},
            "fresh reader credential fields differ")
    for field, expected in (("Uid", [65534] * 4), ("Gid", [65534] * 4),
                            ("NoNewPrivs", [1]), ("PPid", [parent])):
        require(type(credentials[field]) is list and credentials[field] == expected
                and all(type(item) is int for item in credentials[field]), "fresh reader credentials differ")


def validate_observation(report):
    value = report.get("fresh_reader")
    require(type(value) is dict and "check" in value, "fresh reader report is missing its check")
    selected_check = value["check"]
    require(selected_check in CHECKS and selected_check == report.get("reader_check", "stable"), "fresh reader check differs")
    launch = selected_check in LAUNCH_CHECKS
    require(type(value) is dict
            and set(value) == REPORT_FIELDS | (LAUNCH_ROLE_FLAGS if launch else set()),
            "fresh reader report fields differ")
    check = semantic_check(selected_check)
    require(selected_check == "stable" or report.get("case") == "publish-complete", "reader mutation needs a complete publication")
    published = report.get("record_published", False)
    require(type(published) is bool, "fresh reader publication type differs")
    retained_mutation = report.get("case") in RETAINED_MUTATIONS
    require(not retained_mutation or (published and report.get("mutation_observed") is True
            and report.get("retained_publication_mutation_preserved") is True
            and report.get("publication_outcome") == "record-durability-unknown"
            and report.get("experimental_durable_publication_ack") is False),
            "fresh reader retained mutation evidence differs")
    opened = published and not retained_mutation
    require(type(opened) is bool and value["accepted"] is opened, "fresh reader availability differs")
    require(check not in MOUNT_CHECKS or opened, "fresh reader mount requires an opened snapshot")
    require(type(value["reader_pid"]) is int and value["reader_pid"] > 0
            and type(value["reader_starttime"]) is int and value["reader_starttime"] > 0,
            "fresh reader process identity differs")
    require(type(report.get("helper_pid")) is int and report["helper_pid"] > 0
            and value["reader_pid"] != report["helper_pid"], "fresh reader reused the producer process")
    credentials = value["reader_identity"].get("credentials") if type(value["reader_identity"]) is dict else None
    require(type(credentials) is dict and type(credentials.get("PPid")) is list
            and len(credentials["PPid"]) == 1 and type(credentials["PPid"][0]) is int
            and credentials["PPid"][0] > 0, "fresh reader observer parent missing")
    validate_identity(value["reader_identity"], credentials["PPid"][0])
    require(all(value[field] is False for field in FLAGS), "fresh reader claims authority or historical acknowledgment")
    require(value["filesystem_observation"] == "persistent-state-comparison", "fresh reader observation scope differs")
    require(value["retained_original_verified"] is opened, "fresh reader retained-original evidence differs")
    for field in ("reader_state_unchanged", "process_exit_observed", "helper_unchanged", "source_unchanged",
                  "snapshot_released"):
        require(value[field] is True, "fresh reader evidence missing")
    for field in ("reader_sha256", "source_sha256", "state_before_sha256", "state_after_sha256"):
        require(draft.fixed.hexadecimal(value[field], 64), "fresh reader evidence digest differs")
    require(value["state_before_sha256"] == value["state_after_sha256"], "fresh reader changed state")
    invalidated = opened and check in (*MUTATIONS, *MOUNT_CHECKS, *RESTORE_CHECKS)
    require(value["unrelated_publication_observed"] is (check == "unrelated-publication"),
            "unrelated publication evidence differs")
    require(value["mutation_observed"] is invalidated and value["revalidated"] is (opened and not invalidated),
            "fresh reader mutation or revalidation differs")
    expected_error = "crosses-devices" if check in MOUNT_CHECKS else "invalid-data" if invalidated else "none" if opened else "unavailable"
    require(value["initial_error"] == ("none" if opened else "invalid-data" if retained_mutation else "not-found")
            and value["validation_error"] == expected_error,
            "fresh reader did not reject the expected state")
    repeated = opened and not invalidated
    repeat_error = "none" if repeated else "invalid-data" if opened else "unavailable"
    require(value["repeat_snapshot_revalidated"] is repeated and value["repeat_validation_error"] == repeat_error,
            "fresh reader repeated validation evidence differs")
    require(all(value[field] is (repeated and not launch) for field in MATERIAL_FLAGS),
            "fresh reader admitted material evidence differs")
    validate_lifecycle(value, opened, value["revalidated"], expected_error, "final", launch)
    validate_launch(value, opened, value["revalidated"], expected_error, "final", launch)
    validate_restoration(value, selected_check)
    require(value["record_canonical"] == (report.get("record_canonical") if opened else None),
            "fresh reader did not independently retain the producer record")
    if check == "unrelated-publication":
        validate_unrelated_publication(report)
    elif check in MOUNT_CHECKS:
        import creation_mount_probe as mounts
        mounts.validate_covered(report)
    return value


def validate_unrelated_publication(report):
    secondary = report.get("unrelated_producer")
    require(type(secondary) is dict and secondary.get("case") == "publish-complete"
            and secondary.get("reader_check") == "stable", "unrelated producer identity differs")
    first = draft.parse_record(report["record_canonical"])
    second = draft.parse_record(secondary.get("record_canonical"))
    require(first["installation_id"] == second["installation_id"]
            and all(first[field] != second[field] for field in
                    ("device_id", "credential_id", "creation_id", "public_key")),
            "unrelated producer reused the original credential")
    for field in ("process_exit_observed", "native_binding_verified", "independent_signature_verified",
                  "closure_acknowledged", "package_published", "record_published",
                  "experimental_durable_publication_ack", "namespace_preserved_verified"):
        require(secondary.get(field) is True, "unrelated producer evidence missing")
    require(secondary.get("publication_outcome") == "durable"
            and all(secondary.get(field) is False for field in
                    ("creation_admission", "custody_accredited", "producer_origin_checked")),
            "unrelated producer claims authority or incomplete publication")
    nested = validate_observation(secondary)
    require(nested["accepted"] is True and nested["revalidated"] is True,
            "unrelated publication could not be independently read")
    if "fresh_reader" in report:
        reader = report["fresh_reader"]
        require(secondary["helper_pid"] not in (report["helper_pid"], reader["reader_pid"])
                and nested["reader_pid"] not in (report["helper_pid"], reader["reader_pid"]),
                "unrelated processes were not independent")
    return secondary


def state_snapshot():
    """Observe content, permissions and timestamps, excluding access-time effects."""
    result = []
    count = 0
    for root in (draft.USER / "state", draft.DECOY / "custody-v1/users/65534/state"):
        def visit(path, relative, depth):
            nonlocal count
            count += 1
            require(count <= 64 and depth <= 4, "reader state evidence exceeded its bound")
            before = path.lstat()
            require(before.st_uid == before.st_gid == 65534 and not stat.S_ISLNK(before.st_mode),
                    "reader state object ownership differs")
            directory = stat.S_ISDIR(before.st_mode)
            require(directory or stat.S_ISREG(before.st_mode), "reader state object kind differs")
            require(stat.S_IMODE(before.st_mode) == (0o700 if directory else 0o600),
                    "reader state object mode differs")
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
                                 | (os.O_DIRECTORY if directory else 0))
            try:
                require(draft.metadata(os.fstat(descriptor)) == draft.metadata(before), "reader state object changed")
                attributes = tuple(sorted(os.listxattr(descriptor)))
                require(not attributes, "reader state has unsupported extended attributes")
                if directory:
                    names = sorted(os.listdir(descriptor))
                    require(len(names) <= 16, "reader directory count exceeded its bound")
                    content = tuple(names)
                    for name in names:
                        visit(path / name, relative + "/" + name, depth + 1)
                else:
                    require(before.st_nlink == 1 and 0 < before.st_size <= 65536,
                            "reader file shape differs")
                    raw = os.pread(descriptor, 65537, 0)
                    require(len(raw) == before.st_size, "reader file size changed")
                    content = hashlib.sha256(raw).hexdigest()
                require(draft.metadata(os.fstat(descriptor)) == draft.metadata(before)
                        == draft.metadata(path.lstat()), "reader state changed during observation")
                result.append((str(root), relative, draft.metadata(before), attributes, content))
            finally:
                os.close(descriptor)
        visit(root, ".", 0)
    return tuple(result)


def evidence_digest(value):
    return hashlib.sha256(draft.canonical(value)).hexdigest()


def write_new(path, raw):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        require(os.write(descriptor, raw) == len(raw), "reader mutation write was incomplete")
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def mutate(check, credential):
    require(check in MUTATIONS, "unknown fresh reader mutation")
    package = draft.PACKAGES / credential
    admission = draft.ADMISSIONS / credential
    token = os.urandom(16).hex()
    retained = []
    originals = []
    if check == "package-replacement":
        directory = os.open(package, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        retained.append(directory)
        before = os.fstat(directory)
        temporary = package.parent / (".reader-new-" + token)
        previous = package.parent / (".reader-old-" + token)
        os.mkdir(temporary, 0o700)
        for name in ("key.openssh", "manifest.json"):
            descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
            retained.append(descriptor)
            raw = os.pread(descriptor, 65537, 0)
            require(0 < len(raw) <= 65536, "reader package mutation member exceeded its bound")
            originals.append((descriptor, raw))
            write_new(temporary / name, raw)
        os.rename(package, previous)
        os.rename(temporary, package)
        require(package.stat().st_ino != before.st_ino and previous.stat().st_ino == before.st_ino,
                "reader package replacement identity did not change")
    else:
        target = admission / "record.json" if check.startswith("record-") else package / (
            "manifest.json" if check == "manifest-replacement" else "key.openssh")
        in_place = check.endswith("in-place")
        descriptor = os.open(target, (os.O_RDWR if in_place else os.O_RDONLY) | os.O_NOFOLLOW | os.O_CLOEXEC)
        retained.append(descriptor)
        before = os.fstat(descriptor)
        raw = os.pread(descriptor, 65537, 0)
        require(0 < len(raw) <= 65536, "reader mutation input exceeded its bound")
        if in_place:
            if check == "record-in-place":
                record = draft.parse_record(raw.decode("ascii"))
                creation = record["creation_id"]
                record["creation_id"] = creation[:-1] + ("0" if creation[-1] != "0" else "1")
                changed = draft.canonical(record)
                draft.parse_record(changed.decode("ascii"))
            else:
                changed = draft.changed_ciphertext(raw)
            require(changed != raw and len(changed) == len(raw), "reader in-place mutation did not preserve length")
            require(os.pwrite(descriptor, changed, 0) == len(changed), "reader in-place mutation was incomplete")
            os.fsync(descriptor)
            require(target.stat().st_ino == before.st_ino and os.pread(descriptor, 65537, 0) == changed,
                    "reader in-place mutation differs")
            originals.append((descriptor, changed))
        else:
            temporary = target.parent / (".reader-replacement-" + token)
            write_new(temporary, raw)
            os.replace(temporary, target)
            require(target.stat().st_ino != before.st_ino and os.fstat(descriptor).st_nlink == 0,
                    "reader identical-byte replacement identity did not change")
            require(target.read_bytes() == raw, "reader identical-byte replacement contents differ")
            originals.append((descriptor, raw))
    return retained, originals


def replace_restorable(check, credential):
    require(check in RESTORE_CHECKS and draft.fixed.installation(credential),
            "unknown restorable reader replacement")
    package = draft.PACKAGES / credential
    target = draft.ADMISSIONS / credential / "record.json" if check == "restore-record-replacement" else (
        package / "key.openssh" if check == "restore-key-replacement" else package)
    directory = check == "restore-package-replacement"
    token = os.urandom(16).hex()
    backup = target.parent / (".reader-restore-original-" + token)
    temporary = target.parent / (".reader-restore-copy-" + token)
    retained, originals = [], []

    def identity(metadata):
        return [metadata.st_dev, metadata.st_ino]

    def content(path):
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC
                             | (os.O_DIRECTORY if directory else 0))
        try:
            before = os.fstat(descriptor)
            if directory:
                require(sorted(os.listdir(descriptor)) == ["key.openssh", "manifest.json"],
                        "restorable package members differ")
                members = []
                for name in ("key.openssh", "manifest.json"):
                    member = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=descriptor)
                    try:
                        metadata = os.fstat(member)
                        raw = os.pread(member, 65537, 0)
                        require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
                                and 0 < metadata.st_size == len(raw) <= 65536,
                                "restorable package member shape differs")
                        members.append((name, hashlib.sha256(raw).hexdigest()))
                    finally:
                        os.close(member)
                digest = hashlib.sha256(draft.canonical(members)).hexdigest()
            else:
                raw = os.pread(descriptor, 65537, 0)
                require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1
                        and 0 < before.st_size == len(raw) <= 65536,
                        "restorable reader member shape differs")
                digest = hashlib.sha256(raw).hexdigest()
            require(draft.metadata(os.fstat(descriptor)) == draft.metadata(before)
                    == draft.metadata(path.lstat()), "restorable reader object changed while inspected")
            return identity(before), digest
        finally:
            os.close(descriptor)

    descriptor = os.open(target, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC
                         | (os.O_DIRECTORY if directory else 0))
    retained.append(descriptor)
    original_identity, original_hash = content(target)
    require(identity(os.fstat(descriptor)) == original_identity, "restorable reader original identity differs")
    if directory:
        temporary.mkdir(mode=0o700)
        for name in ("key.openssh", "manifest.json"):
            member = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=descriptor)
            retained.append(member)
            raw = os.pread(member, 65537, 0)
            require(0 < len(raw) <= 65536, "restorable package input exceeded its bound")
            originals.append((member, raw))
            write_new(temporary / name, raw)
    else:
        raw = os.pread(descriptor, 65537, 0)
        require(0 < len(raw) <= 65536, "restorable reader input exceeded its bound")
        originals.append((descriptor, raw))
        write_new(temporary, raw)
    os.rename(target, backup)
    os.rename(temporary, target)
    replacement_identity, replacement_hash = content(target)
    require(replacement_identity != original_identity and replacement_identity[0] == original_identity[0]
            and replacement_hash == original_hash and identity(os.fstat(descriptor)) == original_identity,
            "restorable reader replacement was not independently observed")
    require(all(os.pread(member, 65537, 0) == raw for member, raw in originals),
            "restorable reader lost original retained content")

    def restore():
        require(content(target) == (replacement_identity, replacement_hash)
                and content(backup) == (original_identity, original_hash),
                "restorable reader objects changed before restoration")
        os.rename(target, temporary)
        os.rename(backup, target)
        restored_identity, restored_hash = content(target)
        retained_identity = identity(os.fstat(descriptor))
        require(restored_identity == retained_identity == original_identity
                and restored_hash == original_hash
                and all(os.pread(member, 65537, 0) == raw for member, raw in originals),
                "restorable reader did not restore the original retained object")
        require(content(temporary) == (replacement_identity, replacement_hash),
                "restorable reader copy identity changed before removal")
        if directory:
            for name in ("key.openssh", "manifest.json"):
                (temporary / name).unlink()
            temporary.rmdir()
        else:
            temporary.unlink()
        require(not backup.exists() and not temporary.exists(), "restorable reader copies remain named")
        if directory:
            retained_hash = hashlib.sha256(draft.canonical([
                (name, hashlib.sha256(os.pread(member, 65537, 0)).hexdigest())
                for name, (member, _) in zip(("key.openssh", "manifest.json"), originals)
            ])).hexdigest()
        else:
            retained_hash = hashlib.sha256(os.pread(descriptor, 65537, 0)).hexdigest()
        require(retained_hash == original_hash, "restorable reader retained content changed")
        return dict(check=check, original_identity=original_identity, replacement_identity=replacement_identity,
                    restored_identity=restored_identity, retained_identity=retained_identity,
                    original_sha256=original_hash, replacement_sha256=replacement_hash,
                    restored_sha256=restored_hash, retained_sha256=retained_hash)

    return retained, originals, restore


def receive(channel, deadline):
    require(time.monotonic() < deadline, "fresh reader protocol deadline expired")
    channel.settimeout(max(0.001, deadline - time.monotonic()))
    raw, ancillary, flags, _ = channel.recvmsg(8193)
    require(raw and len(raw) <= 8192 and not ancillary and flags == 0, "fresh reader packet boundary differs")
    return raw


def output(process, deadline):
    values = {"out": bytearray(), "err": bytearray()}
    with selectors.DefaultSelector() as selector:
        for stream, label in ((process.stdout, "out"), (process.stderr, "err")):
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, label)
        while selector.get_map():
            require(time.monotonic() < deadline, "fresh reader output deadline expired")
            for key, _ in selector.select(0.02):
                part = os.read(key.fd, 4096)
                if not part:
                    selector.unregister(key.fileobj)
                    continue
                values[key.data].extend(part)
                require(len(values["out"]) <= 8193 and len(values["err"]) <= 1024,
                        "fresh reader output exceeded its bound")
    require(not values["err"], "fresh reader emitted diagnostics")
    return bytes(values["out"])


def observe(args, report, runtime, *, while_held=None, mount_cover=None):
    selected_check = getattr(args, "reader_check", "stable")
    check = semantic_check(selected_check)
    require(selected_check in CHECKS and (selected_check == "stable" or report.get("case") == "publish-complete"),
            "fresh reader mutation selection differs")
    require(while_held is None or check == "unrelated-publication", "unexpected held-reader operation")
    require(callable(mount_cover) if check in MOUNT_CHECKS else mount_cover is None,
            "fresh reader mount callback differs")
    require(report.get("process_exit_observed") is True, "reader preceded the observed producer exit")
    require(args.reader == READER and args.reader.is_absolute(), "fresh reader executable selection differs")
    with args.reader.open("rb") as source:
        require(source.read(4) == b"\x7fELF", "fresh reader executable is not ELF")
    reader_hash = generated.file_sha256(args.reader)
    source_hash = generated.file_sha256(Path(__file__))
    credential = draft.parse_record(report["record_canonical"])["credential_id"]
    retained_mutation = report.get("case") in RETAINED_MUTATIONS
    expected = report.get("record_published", False) and not retained_mutation
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    process = pidfd = None
    retained, originals = [], []
    restoration = restoration_evidence = None
    completed = False
    try:
        before = state_snapshot()
        deadline = time.monotonic() + 20
        invocation = [str(args.reader), "--creation-read", credential, "--observer-fd", str(peer.fileno())]
        if selected_check in LAUNCH_CHECKS:
            invocation.append("--prepare-launch")
        process = subprocess.Popen(invocation,
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, pass_fds=(peer.fileno(),),
            env=life.private_env(runtime), cwd="/", close_fds=True)
        peer.close()
        pidfd = os.pidfd_open(process.pid)
        start = life.process_start(process.pid)
        identity = life.inspect_agent(process.pid, start, str(args.reader), os.getpid())
        validate_identity(identity, os.getpid())
        opened = parse_packet(receive(channel, deadline), process.pid, credential, "opened", check=selected_check)
        require(opened["initial_opened"] is expected
                and opened["initial_error"] == ("none" if expected else "invalid-data" if retained_mutation else "not-found")
                and opened["record_canonical"] == (report["record_canonical"] if expected else None),
                "fresh resident read differs from the real publication")
        require(before == state_snapshot(), "opening the fresh reader changed state")
        if check in RESTORE_CHECKS:
            require(expected, "reader restoration requires an opened resident snapshot")
            retained, originals, restoration = replace_restorable(check, credential)
        elif check in MUTATIONS:
            require(expected, "reader mutation requires an opened resident snapshot")
            retained, originals = mutate(check, credential)
        elif check in MOUNT_CHECKS:
            require(expected, "reader mount requires an opened resident snapshot")
            mount_cover(credential)
            import creation_mount_probe as mounts
            mounts.validate_covered(report)
        elif check == "unrelated-publication":
            require(expected and callable(while_held), "unrelated publication needs a held reader")
            report["unrelated_producer"] = while_held()
            validate_unrelated_publication(report)
        after_mutation = state_snapshot()
        require((before != after_mutation) is (check not in ("stable", *MOUNT_CHECKS)),
                "fresh reader mutation evidence differs")
        require(process.poll() is None and not life.pidfd_ready(pidfd, 0)
                and life.process_start(process.pid) == start, "fresh reader identity changed before revalidation")
        channel.sendall(b"G")
        validated = parse_packet(receive(channel, deadline), process.pid, credential, "validated", opened, check=selected_check)
        expected_error = "crosses-devices" if check in MOUNT_CHECKS else "invalid-data" if check in (*MUTATIONS, *RESTORE_CHECKS) else "none" if expected else "unavailable"
        require(validated["validation_error"] == expected_error
                and validated["snapshot_revalidated"] is (expected and check not in (*MUTATIONS, *MOUNT_CHECKS, *RESTORE_CHECKS)),
                "fresh reader did not reject the exact retained-snapshot change")
        require(after_mutation == state_snapshot(), "fresh reader revalidation changed state")
        if restoration is not None:
            restoration_evidence = restoration()
            restoration_evidence["check"] = selected_check
        after_restoration = state_snapshot()
        require((after_restoration != after_mutation) is (restoration is not None),
                "fresh reader restoration was not independently observed")
        require(process.poll() is None and not life.pidfd_ready(pidfd, 0)
                and life.process_start(process.pid) == start, "fresh reader identity changed before release")
        channel.sendall(b"G")
        final_raw = receive(channel, deadline)
        final = parse_packet(final_raw, process.pid, credential, "final", validated, check=selected_check)
        require(all(final[field] == validated[field] for field in validated
                    if field not in {"stage", "repeat_snapshot_revalidated", "repeat_validation_error", *MATERIAL_FLAGS,
                                     "material_failure_sticky", "reserved_failure_sticky", "current_namespace_revalidated", *LAUNCH_FLAGS}),
                "fresh reader changed its revalidation result during release")
        stdout = output(process, deadline)
        require(life.pidfd_ready(pidfd, max(0.001, deadline - time.monotonic()))
                and process.wait(timeout=max(0.001, deadline - time.monotonic())) == 0
                and stdout == final_raw + b"\n", "fresh reader exit or output differs")
        require(after_restoration == state_snapshot(), "fresh reader exit changed state")
        require(all(os.pread(descriptor, 65537, 0) == raw for descriptor, raw in originals),
                "fresh reader lost the held mutation evidence")
        require(generated.file_sha256(args.reader) == reader_hash
                and generated.file_sha256(Path(__file__)) == source_hash, "fresh reader helper or observer changed")
        report["reader_check"] = selected_check
        report["fresh_reader"] = dict(check=selected_check, reader_pid=process.pid, reader_starttime=start,
            reader_identity=identity, accepted=expected, initial_error=final["initial_error"],
            record_canonical=final["record_canonical"], revalidated=final["snapshot_revalidated"],
            validation_error=final["validation_error"],
            repeat_snapshot_revalidated=final["repeat_snapshot_revalidated"],
            repeat_validation_error=final["repeat_validation_error"], mutation_observed=check in (*MUTATIONS, *MOUNT_CHECKS, *RESTORE_CHECKS), reader_state_unchanged=True,
            filesystem_observation="persistent-state-comparison",
            process_exit_observed=True, helper_unchanged=True, source_unchanged=True, snapshot_released=True,
            reader_sha256=reader_hash, source_sha256=source_hash, state_before_sha256=evidence_digest(after_restoration),
            state_after_sha256=evidence_digest(state_snapshot()), retained_original_verified=expected,
            unrelated_publication_observed=check == "unrelated-publication",
            restoration_observed=restoration_evidence is not None, restoration_evidence=restoration_evidence,
            **{field: final[field] for field in MATERIAL_FLAGS | LAUNCH_FLAGS | LAUNCH_ERRORS},
            **({field: final[field] for field in LAUNCH_ROLE_FLAGS} if selected_check in LAUNCH_CHECKS else {}),
            **{field: final[field] for field in LIFECYCLE_FLAGS | LIFECYCLE_ERRORS},
            **dict.fromkeys(FLAGS, False))
        validate_observation(report)
        completed = True
    finally:
        if not completed:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            UNCERTAIN.append((process, pidfd, channel, peer, retained, originals))
        else:
            for descriptor in [pidfd, *retained]:
                os.close(descriptor)
            for stream in (channel, peer, process.stdout, process.stderr):
                stream.close()
