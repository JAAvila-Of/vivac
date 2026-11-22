"""Observe owned producer death at a publication barrier, without a durable reply."""
import os

import creation_draft_probe as draft
import creation_reader_probe as joint
import lifecycle_probe as life

DEATH_CASES = (
    "death-package-before-rename", "death-package-renamed", "death-package-parent-synced",
    "death-record-file-synced", "death-record-staging-synced", "death-record-before-rename",
    "death-record-renamed", "death-record-before-parent-sync", "death-record-parent-synced",
    "death-before-durable-reply",
)
LINUX_SIGKILL = 9
require = draft.require
PACKAGE_CASES = DEATH_CASES[:3]
PENDING_CASES = DEATH_CASES[3:6]
VISIBLE_CASES = DEATH_CASES[6:]
UNCERTAIN = []


def parse_cut(payload, case, pid, identity=None):
    import creation_native_probe as producer
    require(case in DEATH_CASES, "unknown producer death cut")
    value = draft.decode(payload, 8192)
    expected = producer.BASE | {"challenge", "signature"} | (producer.FINAL - {"preparation_dropped"})
    expected |= {"cut_point", "durable_reply_sent"}
    require(type(value) is dict and set(value) == expected and value["stage"] == "publication-cut",
            "producer death cut packet fields differ")
    require(type(value["cut_point"]) is str and value["cut_point"] == case.removeprefix("death-")
            and value["durable_reply_sent"] is False, "producer death cut claims a reply or another point")
    normalized = {key: item for key, item in value.items() if key not in ("cut_point", "durable_reply_sent")}
    normalized.update(stage="closed-before-publication", preparation_dropped=False)
    producer.parse_packet(draft.canonical(normalized), case, pid, "closed-before-publication", identity)
    return value


def expected_state(case):
    require(case in DEATH_CASES, "unknown producer death state")
    package = case != "death-package-before-rename"
    record = case in VISIBLE_CASES
    return dict(package_published=package, record_published=record,
                pending_creation_record_observed=case in PENDING_CASES,
                publication_outcome="interrupted-record-visible" if record else
                    "interrupted-package-only" if package else "interrupted-unpublished",
                package_durable_observed=case not in PACKAGE_CASES,
                package_parent_sync_observed=case != "death-package-before-rename" and case != "death-package-renamed",
                record_parent_sync_observed=case in ("death-record-parent-synced", "death-before-durable-reply"),
                joint_validation_completed=case == "death-before-durable-reply")


def validate_death_receipt(returncode, sent_by_pidfd, exit_ready, normal_final_received):
    require(type(returncode) is int and returncode == -LINUX_SIGKILL
            and sent_by_pidfd is True and exit_ready is True and normal_final_received is False,
            "producer exit is not the owned pidfd SIGKILL receipt")


def validate_observation(value, case, *, reader_required=True):
    import creation_native_probe as producer
    require(type(value) is dict and case in DEATH_CASES and value.get("case") == case
            and type(value.get("version")) is int and value["version"] == 1
            and value.get("experiment") == "rooted-native-creation" and value.get("status") == "observed",
            "producer death observation identity differs")
    require(all(value.get(field) is False for field in producer.FLAGS), "producer death observation claims authority")
    require("final" not in value and "preparation_dropped" not in value
            and value.get("preparation_release_acknowledged") is False
            and value.get("normal_final_received") is False and value.get("durable_reply_sent") is False
            and value.get("experimental_durable_publication_ack") is False
            and value.get("physical_power_loss_simulated") is False,
            "producer death observation claims an unobserved release or reply")
    for field in ("process_exit_observed", "producer_exit_observed", "terminal_restored", "standard_streams_clean",
                  "standard_stream_eof_observed",
                  "helper_unchanged", "sources_unchanged", "root_binding_unchanged", "process_identity_verified",
                  "private_terminal_observed", "secret_boundaries_checked", "original_package_verified",
                  "independent_exits_observed", "private_loader_terminal_observed", "native_identity_observed",
                  "provider_revalidated", "retained_connection_refused", "new_connection_refused",
                  "native_binding_verified", "independent_signature_verified", "altered_message_refused",
                  "closure_acknowledged", "independent_closed_before_publication", "publication_filesystem_verified",
                  "cut_witness_verified", "producer_killed_by_pidfd", "producer_kill_identity_verified",
                  "namespace_preserved_verified"):
        require(value.get(field) is True, "producer death observation evidence missing")
    for field in ("helper_sha256", "observer_sha256", "real_binding_sha256", "decoy_binding_sha256", "openssl_sha256",
                  "death_source_sha256", "controller_source_sha256", "state_at_cut_sha256", "state_after_death_sha256"):
        require(draft.fixed.hexadecimal(value.get(field), 64), "producer death observation provenance differs")
    require(value["state_at_cut_sha256"] == value["state_after_death_sha256"], "producer death changed the observed cut state")
    require(type(value.get("producer_starttime")) is int and value["producer_starttime"] > 0,
            "producer death start time differs")
    identity = value.get("producer_identity")
    credentials = identity.get("credentials") if type(identity) is dict else None
    require(type(credentials) is dict and type(credentials.get("PPid")) is list
            and len(credentials["PPid"]) == 1 and type(credentials["PPid"][0]) is int
            and credentials["PPid"][0] > 0, "producer death observer identity missing")
    joint.validate_identity(identity, credentials["PPid"][0])
    require(value.get("producer_exit_code") is None and type(value.get("producer_exit_signal")) is int
            and value["producer_exit_signal"] == LINUX_SIGKILL, "producer death exact cause differs")
    validate_death_receipt(-value["producer_exit_signal"], value["producer_killed_by_pidfd"],
                           value["producer_exit_observed"], value["normal_final_received"])
    cut = parse_cut(draft.canonical(value.get("publication_cut")), case, value.get("helper_pid"))
    require(cut["record_canonical"] == value.get("record_canonical")
            and value.get("cut_point") == cut["cut_point"], "producer death changed its original record or cut")
    record = draft.parse_record(value["record_canonical"])
    require(record["installation_id"] == value.get("selected_installation_id")
            and record["device_id"] == value.get("device_id") and record["credential_id"] == value.get("credential_id"),
            "producer death selected another resident identity")
    environment = producer.validate_process_metadata(value.get("native_process_metadata"))
    require(value.get("native_environment_absence_verified") is environment, "producer death environment claim differs")
    producer.validate_process_identity(value.get("native_process_identity"), value.get("helper_pid"))
    producer.validate_abandonment(value, False)
    require(value.get("preserved_namespace") == {"packages": [], "admissions": []},
            "producer death experiment adopted prior entries")
    state = expected_state(case)
    for field, expected in state.items():
        require(value.get(field) == expected if type(expected) is str else value.get(field) is expected,
                "producer death visible state differs")
    require(value.get("creation_record_verified") is state["record_published"]
            and value.get("final_slot_absent") is (not state["package_published"])
            and value.get("admissions_empty") is (not state["record_published"] and not state["pending_creation_record_observed"])
            and value.get("mutation_observed") is False and value.get("invalidated_stage_preserved") is False,
            "producer death filesystem or mutation evidence differs")
    if reader_required:
        joint.validate_observation(value)
    return value


def retain_record(producer, admissions, name, text, secret):
    producer.creation_record_snapshot(admissions, name, text, secret)
    directory = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=admissions)
    member = None
    try:
        member = os.open("record.json", os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=directory)
        result = (name, directory, member, draft.metadata(os.fstat(directory)), draft.metadata(os.fstat(member)), text.encode("ascii"))
        validate_retained_record(producer, admissions, result, secret)
        return result
    except BaseException:
        if member is not None:
            os.close(member)
        os.close(directory)
        raise


def validate_retained_record(producer, admissions, retained, secret):
    name, directory, member, before_directory, before_member, raw = retained
    producer.creation_record_snapshot(admissions, name, raw.decode("ascii"), secret)
    require(draft.metadata(os.fstat(directory)) == before_directory
            == draft.metadata(os.stat(name, dir_fd=admissions, follow_symlinks=False))
            and draft.metadata(os.fstat(member)) == before_member
            == draft.metadata(os.stat("record.json", dir_fd=directory, follow_symlinks=False))
            and os.pread(member, 2049, 0) == raw, "producer death lost the retained record object")


def inspect_state(producer, case, packages, admissions, snapshot, evidence, secret, identity, preserved):
    state = expected_state(case)
    credential = draft.parse_record(identity["record_canonical"])["credential_id"]
    if state["package_published"]:
        producer.published_package(packages, snapshot, credential, evidence, secret, preserved)
    else:
        producer.unchanged_stage(packages, snapshot, evidence, preserved)
    names = os.listdir(admissions)
    selected = None
    if state["record_published"]:
        require(names == [credential], "producer death final admission membership differs")
        selected = credential
    elif state["pending_creation_record_observed"]:
        require(len(names) == 1 and names[0].startswith(".pending-")
                and draft.fixed.hexadecimal(names[0][9:], 64) and credential not in names,
                "producer death private record stage differs")
        selected = names[0]
    else:
        require(not names, "producer death exposed an unexpected admission")
    if selected is not None:
        producer.creation_record_snapshot(admissions, selected, identity["record_canonical"], secret)
    return selected


def publication_protocol(args, report, process, helper_pidfd, helper_start, channel, packages, admissions, snapshot,
                         original_evidence, secret, pidfds, identity, bound, preserved):
    import creation_native_probe as producer
    import signal
    require(args.case in DEATH_CASES and not preserved.package_names and not preserved.admission_names,
            "producer death needs an empty initial namespace")
    closed = producer.parse_packet(producer.receive(channel), args.case, process.pid, "closed-before-publication", identity)
    require(all(closed[field] == bound[field] for field in bound if field != "stage"), "producer death binding changed before closure")
    require(all(life.pidfd_ready(descriptor, 5) for descriptor in pidfds.values()), "producer death preceded native exits")
    producer.unchanged_stage(packages, snapshot, original_evidence, preserved)
    require(not os.listdir(admissions), "producer death record preceded package publication")
    report["independent_closed_before_publication"] = True
    preserved.validate()
    channel.sendall(b"C")
    if args.case not in PACKAGE_CASES:
        package = producer.parse_packet(producer.receive(channel), args.case, process.pid, "package-durable", identity)
        require(all(package[field] == closed[field] for field in closed if field != "stage"), "producer death package checkpoint changed")
        credential = draft.parse_record(identity["record_canonical"])["credential_id"]
        producer.published_package(packages, snapshot, credential, original_evidence, secret, preserved)
        require(not os.listdir(admissions), "producer death record preceded durable package")
        preserved.validate()
        channel.sendall(b"D")
    cut = parse_cut(producer.receive(channel), args.case, process.pid, identity)
    require(all(cut[field] == closed[field] for field in closed if field not in ("stage", "preparation_dropped")),
            "producer death cut changed its native closure evidence")
    selected = inspect_state(producer, args.case, packages, admissions, snapshot, original_evidence, secret, identity, preserved)
    retained = None
    completed = False
    try:
        if selected is not None:
            retained = retain_record(producer, admissions, selected, identity["record_canonical"], secret)
        at_cut = joint.state_snapshot()
        start = life.process_start(process.pid)
        process_identity = life.inspect_agent(process.pid, start, str(args.writer), os.getpid())
        joint.validate_identity(process_identity, os.getpid())
        require(start == helper_start and process.poll() is None and not life.pidfd_ready(helper_pidfd, 0)
                and life.process_start(process.pid) == start, "producer death target was not the owned live process")
        signal.pidfd_send_signal(helper_pidfd, LINUX_SIGKILL, None, 0)
        require(life.pidfd_ready(helper_pidfd, 5), "producer death remained unobserved")
        code = process.wait(timeout=5)
        validate_death_receipt(code, True, life.pidfd_ready(helper_pidfd, 0), False)
        channel.settimeout(5)
        remainder, ancillary, flags, _ = channel.recvmsg(8193)
        require(not remainder and not ancillary and flags == 0, "producer death returned another packet after the cut")
        require(all(life.pidfd_ready(descriptor, 0) for descriptor in pidfds.values()), "producer death revived a native child")
        inspect_state(producer, args.case, packages, admissions, snapshot, original_evidence, secret, identity, preserved)
        if retained is not None:
            validate_retained_record(producer, admissions, retained, secret)
        after = joint.state_snapshot()
        require(at_cut == after, "producer death changed the cut namespace")
        preserved.validate()
        state = expected_state(args.case)
        report.update(state)
        report.update(publication_cut=cut, cut_point=cut["cut_point"], cut_witness_verified=True,
            producer_starttime=start, producer_identity=process_identity, producer_killed_by_pidfd=True,
            producer_kill_identity_verified=True, producer_exit_observed=True, producer_exit_code=None,
            producer_exit_signal=LINUX_SIGKILL, normal_final_received=False, durable_reply_sent=False,
            experimental_durable_publication_ack=False, preparation_release_acknowledged=False,
            physical_power_loss_simulated=False, publication_filesystem_verified=True,
            state_at_cut_sha256=joint.evidence_digest(at_cut), state_after_death_sha256=joint.evidence_digest(after),
            creation_record_verified=state["record_published"], final_slot_absent=not state["package_published"],
            admissions_empty=not state["record_published"] and not state["pending_creation_record_observed"])
        completed = True
        return None
    finally:
        if retained is not None:
            if completed:
                os.close(retained[2])
                os.close(retained[1])
            else:
                UNCERTAIN.append(retained)
