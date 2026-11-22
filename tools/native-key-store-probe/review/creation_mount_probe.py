"""Retain original objects while an isolated auxiliary covers their mount paths."""
import os
from pathlib import Path
import stat
import time

import creation_mount_common as common

MOUNT_CHECKS = common.MOUNT_CHECKS
UNCERTAIN = []
FIELDS = {"version", "check", "phase", "target", "kind", "token", "original", "covered", "restored",
          "evidence", "native_outcome_verified"}


def case_for(check):
    common.require(check in MOUNT_CHECKS, "unknown live mount experiment")
    return "publish-invalidate-before-package" if check.startswith("writer-") else "publish-complete"


def request_for(value, operation):
    return dict(version=1, operation=operation, check=value["check"], target=value["target"], kind=value["kind"],
                token=value["token"], original=value["original"])


def validate_covered(report):
    value = report.get("live_mount")
    common.require(type(value) is dict and set(value) == FIELDS and type(value["version"]) is int
                   and value["version"] == 1 and value["phase"] in ("covered", "restored")
                   and value["covered"] is True and type(value["restored"]) is bool
                   and type(value["native_outcome_verified"]) is bool, "live mount evidence fields differ")
    common.require(report.get("case") == case_for(value["check"]), "live mount producer case differs")
    request = common.validate_request(request_for(value, "mount"))
    evidence = value["evidence"]
    required = {"mount_ack", "held_covered", "named_covered", "private_mount_verified"}
    restored_fields = {"unmount_ack", "held_restored", "named_restored"}
    common.require(type(evidence) is dict and set(evidence) == required | (restored_fields if value["restored"] else set()),
                   "live mount snapshot evidence differs")
    acknowledgment = common.validate_ack(evidence["mount_ack"], request)
    common.require(evidence["private_mount_verified"] is True, "independent private mount observation is missing")
    common.require(evidence["held_covered"] == value["original"] and evidence["named_covered"] == acknowledgment["after"],
                   "live mount did not preserve the held original object")
    common.require(value["restored"] is (value["phase"] == "restored"), "live mount restoration phase differs")
    return value


def validate_native_outcome(report, value):
    common.require(report.get("process_exit_observed") is True and report.get("independent_exits_observed") is True
                   and report.get("independent_closed_before_publication") is True,
                   "live mount native process closure was not observed")
    common.require(all(report.get(flag) is False for flag in
                   ("ready_published", "producer_origin_checked", "creation_admission", "custody_accredited")),
                   "live mount experiment claims authority")
    reader = report.get("fresh_reader")
    common.require(type(reader) is dict and reader.get("process_exit_observed") is True
                   and reader.get("durable_ack_inferred") is False, "live mount fresh reader evidence missing")
    if value["check"].startswith("writer-"):
        common.require(report.get("publication_outcome") == "unpublished" and report.get("package_published") is False
                       and report.get("record_published") is False and report.get("experimental_durable_publication_ack") is False
                       and reader.get("accepted") is False and reader.get("validation_error") == "unavailable",
                       "covered preparation was published or acknowledged")
    else:
        common.require(report.get("publication_outcome") == "durable" and report.get("package_published") is True
                       and report.get("record_published") is True and report.get("experimental_durable_publication_ack") is True,
                       "reader mount experiment lacks its prior durable publication")
        common.require(reader.get("check") == "mount-" + value["check"][7:]
                       and reader.get("accepted") is True and reader.get("revalidated") is False
                       and reader.get("validation_error") == "crosses-devices" and reader.get("mutation_observed") is True,
                       "retained reader did not reject the live mount crossing")


def validate_observation(report):
    value = validate_covered(report)
    common.require(value["phase"] == "restored" and value["restored"] is True
                   and value["native_outcome_verified"] is True, "live mount experiment did not finish")
    evidence = value["evidence"]
    acknowledgment = common.validate_ack(evidence["unmount_ack"], request_for(value, "unmount"))
    common.require(evidence["named_restored"] == evidence["held_restored"] == value["original"]
                   and acknowledgment["before"] == evidence["named_covered"],
                   "live mount experiment did not restore its exact covered object")
    common.require(acknowledgment["namespaces"]["target_mount_namespace"]
                   == evidence["mount_ack"]["namespaces"]["target_mount_namespace"],
                   "live mount target namespace changed before restoration")
    validate_native_outcome(report, value)
    return value


def publish_request(path, value):
    raw = common.canonical(common.validate_request(value))
    common.require(len(raw) <= 8192 and path.parent == Path("/reports/run"), "mount request destination differs")
    temporary = path.with_name("." + path.name + "-" + value["token"] + ".tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        offset = 0
        while offset < len(raw):
            count = os.write(descriptor, raw[offset:])
            common.require(count > 0, "mount request write was incomplete")
            offset += count
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    os.link(temporary, path, follow_symlinks=False)
    os.unlink(temporary)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def receive_ack(path, request):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK)
        except FileNotFoundError:
            time.sleep(0.025)
            continue
        try:
            info = os.fstat(descriptor)
            common.require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534 and info.st_nlink == 1
                           and stat.S_IMODE(info.st_mode) == 0o600 and 0 < info.st_size <= 32768,
                           "mount acknowledgment file differs")
            raw = os.read(descriptor, 32769)
            common.require(len(raw) == info.st_size, "mount acknowledgment was incompletely read")
            after = os.fstat(descriptor)
            common.require((after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
                           == (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns),
                           "mount acknowledgment changed while read")
            return common.validate_ack(common.decode_json(raw, 32768), request)
        finally:
            os.close(descriptor)
    raise common.ProbeError("mount acknowledgment deadline expired")


class MountExperiment:
    def __init__(self, args, report):
        common.require(args.mount_check in MOUNT_CHECKS and args.case == case_for(args.mount_check),
                       "live mount experiment case differs")
        self.args, self.report = args, report
        self.directory = args.report.parent
        common.require(self.directory == Path("/reports/run") and "live_mount" not in report,
                       "live mount experiment report destination differs")
        self.held = None

    def exchange(self, operation):
        request = request_for(self.report["live_mount"], operation)
        publish_request(self.directory / (operation + "-request.json"), request)
        return receive_ack(self.directory / (operation + "-ack.json"), request)

    def cover(self, selector, original_fd=None):
        common.require(self.held is None and "live_mount" not in self.report, "live mount was requested twice")
        target, kind = common.selected_target(self.args.mount_check, selector)
        self.held = common.open_object(target, kind) if original_fd is None else os.dup(original_fd)
        original = common.snapshot(target, kind, self.held)
        common.require(common.snapshot(target, kind) == original, "live mount path differs from its held original")
        value = dict(version=1, check=self.args.mount_check, phase="preparing", target=target, kind=kind,
            token=os.urandom(32).hex(), original=original, covered=False, restored=False,
            evidence={}, native_outcome_verified=False)
        self.report["live_mount"] = value
        acknowledgment = self.exchange("mount")
        held = common.snapshot(target, kind, self.held)
        named = common.snapshot(target, kind)
        common.require(held == original and named == acknowledgment["after"], "live mount witness differs from helper receipt")
        common.require_private_mount(target, named["mount_id"])
        value.update(phase="covered", covered=True,
                     evidence=dict(mount_ack=acknowledgment, held_covered=held, named_covered=named,
                                   private_mount_verified=True))
        validate_covered(self.report)

    def package_mutator(self, snapshot, mutation_name):
        common.require(self.args.mount_check.startswith("writer-") and mutation_name == "mutate-key",
                       "live mount package mutation selection differs")
        original_fd = snapshot[1] if self.args.mount_check == "writer-stage" else snapshot[3][
            "key.openssh" if self.args.mount_check == "writer-key" else "manifest.json"][0]
        self.cover(snapshot[0], original_fd)

    def reader_cover(self, credential):
        common.require(self.args.mount_check.startswith("reader-") and credential == self.report.get("credential_id"),
                       "live mount reader credential differs")
        self.cover(credential)

    def release(self):
        value = validate_covered(self.report)
        common.require(self.held is not None and common.snapshot(value["target"], value["kind"], self.held) == value["original"]
                       and common.snapshot(value["target"], value["kind"]) == value["evidence"]["named_covered"],
                       "live mount changed before restoration")
        common.require_private_mount(value["target"], value["evidence"]["named_covered"]["mount_id"])
        acknowledgment = self.exchange("unmount")
        held = common.snapshot(value["target"], value["kind"], self.held)
        named = common.snapshot(value["target"], value["kind"])
        common.require(held == named == value["original"], "live mount restoration changed its original object")
        value["evidence"].update(unmount_ack=acknowledgment, held_restored=held, named_restored=named)
        value.update(phase="restored", restored=True)
        os.close(self.held)
        self.held = None


def run(args, report, bindings, provider, verifier, runtime):
    import creation_native_probe as native
    import creation_reader_probe as joint
    experiment = MountExperiment(args, report)
    try:
        if args.mount_check.startswith("writer-"):
            common.require(args.reader_check == "stable", "writer live mount requires a stable fresh reader")
            native.observe(args, report, bindings, provider, verifier, runtime, package_mutator=experiment.package_mutator)
            experiment.release()
            joint.observe(args, report, runtime)
        else:
            common.require(args.reader_check == "mount-" + args.mount_check[7:], "reader live mount selection differs")
            native.observe(args, report, bindings, provider, verifier, runtime)
            joint.observe(args, report, runtime, mount_cover=experiment.reader_cover)
            experiment.release()
        value = report["live_mount"]
        validate_native_outcome(report, value)
        value["native_outcome_verified"] = True
        validate_observation(report)
    except BaseException:
        UNCERTAIN.append(experiment)
        raise
