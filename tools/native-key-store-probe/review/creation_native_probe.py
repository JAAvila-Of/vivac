#!/usr/bin/env python3
"""Observe a rooted attended draft through native possession and closure."""
import argparse
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
import tempfile
import time

import creation_draft_probe as draft
import creation_reader_probe as joint
import creation_namespace_probe as namespaces
import creation_death_probe as death
import generated_probe as generated
import lifecycle_probe as life
import openssh_probe as native
import provider_probe as providers
import rust_creation_binding_probe as binding
import rust_launcher_probe as spawn
import rust_loader_probe as loader
import session_probe as session

RETAINED_CASES = ("publish-invalidate-retained-record", "publish-invalidate-retained-package")
PUBLICATION_CASES = ("publish-complete", "publish-package-failure", "publish-record-before-rename-failure",
                     "publish-record-after-rename-failure", "publish-invalidate-before-package", "publish-invalidate-package", *RETAINED_CASES)
CONTENDED_CASE = "publish-contended-credential"
MULTI_CASES = ("multi-unrelated", "multi-contended")
DEATH_CASES = death.DEATH_CASES
MOUNT_CHECKS = ("writer-stage", "writer-key", "writer-manifest", "reader-package",
                "reader-key", "reader-manifest", "reader-record")
PRODUCER_CASES = ("complete", "invalidate-before-proof", "abandon-after-load", *PUBLICATION_CASES, *DEATH_CASES, CONTENDED_CASE)
CASES = (*PRODUCER_CASES[:-1], *MULTI_CASES)
PUBLICATION_FIELDS = {"publication_outcome", "package_published", "record_published", "experimental_durable_publication_ack"}
PUBLICATION_OUTCOMES = {
    "publish-complete": ("durable", True, True, True),
    "publish-package-failure": ("package-durability-unknown", True, False, False),
    "publish-record-before-rename-failure": ("package-only", True, False, False),
    "publish-record-after-rename-failure": ("record-durability-unknown", True, True, False),
    "publish-invalidate-before-package": ("unpublished", False, False, False),
    "publish-invalidate-package": ("package-only", True, False, False),
    **dict.fromkeys(RETAINED_CASES, ("record-durability-unknown", True, True, False)),
    CONTENDED_CASE: ("unpublished", False, False, False),
}
REFUSALS = set()
FLAGS = {"ready_published", "producer_origin_checked", "creation_admission", "custody_accredited"}
BASE = {"version", "stage", "case", "helper_pid", "agent_pid", "loader_pid", "creator_tid",
        "record_canonical", "native_binding_verified", *FLAGS}
COUNTS = {"proofs_published", "proofs_verified", "native_requests_sent", "native_replies_received",
          "workers_joined", "lock_callers", "lock_acknowledgments"}
FINAL = {"closure_acknowledged", "resources_closed_after_drain", "preparation_dropped",
         "agent_exit_code", "agent_exit_signal", "loader_exit_code", "loader_exit_signal", *COUNTS}
UNCERTAIN = []
require = generated.require
# This is a Linux wire-protocol value, including when inspected on Windows.
LINUX_SIGKILL = 9
LINUX_SIGHUP = 1
ENDPOINT_UNLOCK_PROMPT = b"Device credential unlock secret: "
ENDPOINT_PROOF_FLAGS = {"signature_verified", "distinct_processes_verified", "process_exits_observed",
                        "session_closed", "standard_streams_clean"}
ENDPOINT_WRONG_SECRET_REFUSED = "attended_endpoint_wrong_secret_refused"
ACTIVE_ENDPOINT_PROOF_FLAGS = {
    "predecessor_signed", "missing_closure_refused", "incomplete_closure_refused",
    "different_generation_closure_refused", "lost_connection_closure_refused", "terminal_restored",
    "distinct_generation_verified", "successor_signature_verified", "sessions_closed",
    "process_exits_observed", "reservation_reacquired_after_keeper_exit",
    "live_keeper_death_observed",
}
OPENING_PERMIT_CASES = {
    **dict.fromkeys(("missing-collection", "symlink-collection", "missing-permit", "symlink-permit",
                    "hardlink-permit", "wrong-mode", "wrong-digest", "wrong-version", "wrong-width",
                    "wrong-kind", "reserved-byte"), "refused"),
    "first-open": "first-open",
    "clean-close": "clean-close",
}
ENDPOINT_FAILURE_PHASES = (
    "accept", "connection", "reserve-attempt", "receive-challenge", "consume-attempt",
    "prepare-context", "prepare-nonce", "prepare-agent-connect", "prepare-agent-peer",
    "prepare-agent-write", "prepare-agent-header", "prepare-agent-body", "prepare-agent-deadline",
    "prepare-agent-identity", "prepare-revalidate", "prepare-final-deadline", "sign-proof",
    "validate-before-send", "send-proof", "validate-after-send", "check-result", "close-session",
    "remove-sockets", "verify-proof", "attended-unlock", "other",
    "keeper-owner", "keeper-launch", "keeper-ready", "keeper-terminal", "keeper-bootstrap",
    "keeper-offer", "keeper-adopt", "keeper-confirm", "keeper-confirm-received",
    "keeper-stop", "keeper-collect", "keeper-advance",
    "keeper-predecessor-material", "keeper-predecessor-openssh", "keeper-predecessor-terminal",
    "active-old-ready", "active-old-closed", "active-close-frame", "active-old-exit", "active-advance",
    "active-close-refusal",
)


def is_publication(case):
    return case in PUBLICATION_OUTCOMES or case in DEATH_CASES


def failure_frames(error):
    frames = []
    current = error.__traceback__
    while current is not None:
        frames.append({"file": Path(current.tb_frame.f_code.co_filename).name, "line": current.tb_lineno})
        current = current.tb_next
    return frames[-12:]


def read_process_metadata(path):
    with path.open("rb") as source:
        value = source.read(65537)
    require(len(value) <= 65536, "native process metadata exceeded its bound")
    return value


def process_metadata(pid, secret, *, read=None):
    read = read_process_metadata if read is None else read
    root = Path("/proc", str(pid))
    command = read(root / "cmdline")
    require(type(command) is bytes and len(command) <= 65536 and secret not in command,
            "native command metadata boundary differs")
    result = {"cmdline_secret_absence_verified": True}
    try:
        environment = read(root / "environ")
    except PermissionError:
        result.update(environment_observation="permission-denied", environment_secret_absence_verified=False)
    else:
        require(type(environment) is bytes and len(environment) <= 65536 and secret not in environment,
                "native environment metadata boundary differs")
        result.update(environment_observation="verified", environment_secret_absence_verified=True)
    return result


def validate_process_metadata(value):
    require(type(value) is dict and set(value) == {"agent", "loader"}, "native metadata process set differs")
    verified = []
    for item in value.values():
        require(type(item) is dict and set(item) == {"cmdline_secret_absence_verified", "environment_observation",
                                                   "environment_secret_absence_verified"}, "native metadata fields differ")
        require(item["cmdline_secret_absence_verified"] is True
                and type(item["environment_observation"]) is str
                and item["environment_observation"] in ("verified", "permission-denied"),
                "native metadata observation differs")
        expected = item["environment_observation"] == "verified"
        require(item["environment_secret_absence_verified"] is expected, "native environment absence claim differs")
        verified.append(expected)
    return all(verified)


def validate_process_identity(value, helper_pid):
    require(type(value) is dict and set(value) == {"agent", "loader"}, "native identity process set differs")
    require(type(helper_pid) is int and helper_pid > 0, "native identity parent differs")
    for item in value.values():
        require(type(item) is dict and set(item) == {"credentials", "executable_observation"},
                "native identity fields differ")
        require(type(item["executable_observation"]) is str and item["executable_observation"] in
                ("verified", "exe_link_denied_cmdline_matches"), "native executable observation differs")
        credentials = item["credentials"]
        require(type(credentials) is dict and set(credentials) == {"Uid", "Gid", "NoNewPrivs", "PPid"},
                "native credential fields differ")
        for field, expected in (("Uid", [65534] * 4), ("Gid", [65534] * 4),
                                ("NoNewPrivs", [1]), ("PPid", [helper_pid])):
            require(type(credentials[field]) is list and credentials[field] == expected
                    and all(type(number) is int for number in credentials[field]), "native credential observation differs")


def validate_abandonment(value, abandoned):
    require("abandoned_stage_retained_while_agent_alive" not in value, "retired abandonment claim present")
    for field in ("abandoned_stage_retained_without_closure", "native_agent_alive_before_abandonment",
                  "abandoned_stage_retained_while_producer_alive"):
        require(value.get(field) is abandoned, "abandonment retention evidence differs")


def parse_packet(payload, case, pid, stage, identity=None):
    require(case in PRODUCER_CASES and stage in ("loader-ready", "loader-handoff", "before-proof", "bound",
                                       "binding-refused", "loaded", "abandoned", "final",
                                       "closed-before-publication", "package-durable", "opening-permit-durable", "publication-retained"), "native stage differs")
    publication = is_publication(case)
    checkpoint = stage in ("closed-before-publication", "package-durable", "opening-permit-durable", "publication-retained")
    require(stage != "opening-permit-durable" or case in ("publish-complete", "publish-record-before-rename-failure"),
            "unexpected opening permit checkpoint")
    require(stage != "publication-retained" or case in RETAINED_CASES, "unexpected retained publication checkpoint")
    require(not checkpoint or publication, "unexpected publication checkpoint")
    require(stage != "package-durable" or case not in ("publish-package-failure", "publish-invalidate-before-package", CONTENDED_CASE),
            "unexpected durable package checkpoint")
    require(stage not in ("loaded", "abandoned") or case == "abandon-after-load", "unexpected abandonment stage")
    require(stage not in ("before-proof", "binding-refused") or case == "invalidate-before-proof", "unexpected refusal stage")
    require(stage != "bound" or case == "complete" or publication, "unexpected proof stage")
    value = draft.decode(payload, 8192)
    fields = BASE.copy()
    if stage in ("bound", "binding-refused", "final") or checkpoint:
        fields |= {"challenge", "signature"}
    if stage == "abandoned":
        fields |= {"producer_context_dropped", "closure_acknowledged", "admission_stopped"}
    if stage == "final" or checkpoint:
        fields |= FINAL
    if stage == "final" and case in DEATH_CASES:
        raise generated.ProbeError("killed producer cannot return a normal final result")
    if stage == "final" and publication:
        fields |= PUBLICATION_FIELDS
    require(type(value) is dict and set(value) == fields, "native packet fields differ")
    require(type(value["version"]) is int and value["version"] == 1 and value["case"] == case
            and value["stage"] == stage, "native packet identity differs")
    for field in ("helper_pid", "agent_pid", "loader_pid", "creator_tid"):
        require(type(value[field]) is int and value[field] > 0, "native process type differs")
    require(value["helper_pid"] == value["creator_tid"] == pid
            and len({value["agent_pid"], value["loader_pid"], pid}) == 3, "native process identity differs")
    record = draft.parse_record(value["record_canonical"])
    if identity is not None:
        require(all(value[field] == identity[field] for field in
                    ("helper_pid", "agent_pid", "loader_pid", "creator_tid", "record_canonical")),
                "native ownership or record changed")
    require(all(value[field] is False for field in FLAGS), "native packet claims authority")
    complete = (case == "complete" or publication) and (stage in ("bound", "final") or checkpoint)
    require(value["native_binding_verified"] is complete, "native binding outcome differs")
    if stage in ("bound", "binding-refused", "final") or checkpoint:
        require(case != "abandon-after-load" and stage != ("binding-refused" if complete else "bound"),
                "native result stage differs")
        if complete:
            challenge = binding.public_hex(value["challenge"], len(binding.DOMAIN) + 64)
            require(challenge[:len(binding.DOMAIN)] == binding.DOMAIN
                    and challenge[len(binding.DOMAIN):len(binding.DOMAIN) + 32]
                    == hashlib.sha256(draft.canonical(record)).digest(), "native challenge purpose differs")
            binding.public_hex(value["signature"], 64)
        else:
            require(value["challenge"] is None and value["signature"] is None, "refusal disclosed proof")
    if stage == "abandoned":
        require(case == "abandon-after-load" and value["producer_context_dropped"] is True
                and value["closure_acknowledged"] is False and value["admission_stopped"] is True,
                "abandonment retention differs")
    if stage == "final" or checkpoint:
        require(all(value[field] is True for field in
                    ("closure_acknowledged", "resources_closed_after_drain"))
                and value["preparation_dropped"] is (not checkpoint),
                "native closure differs")
        for field in COUNTS:
            expected = int(complete) if field == "proofs_published" else 1
            require(type(value[field]) is int and value[field] == expected, "native closure count differs")
        require(value["agent_exit_code"] is None and value["loader_exit_signal"] is None
                and type(value["agent_exit_signal"]) is int and value["agent_exit_signal"] == LINUX_SIGKILL
                and type(value["loader_exit_code"]) is int and value["loader_exit_code"] == 0,
                "native exact exit cause differs")
    if stage == "final" and publication:
        validate_publication_outcome(value, case)
    return value


def validate_publication_outcome(value, case):
    require(case in PUBLICATION_OUTCOMES, "publication case differs")
    outcome, package, record, acknowledged = PUBLICATION_OUTCOMES[case]
    require(type(value.get("publication_outcome")) is str and value["publication_outcome"] == outcome,
            "publication uncertainty differs")
    for field, expected in (("package_published", package), ("record_published", record),
                            ("experimental_durable_publication_ack", acknowledged)):
        require(value.get(field) is expected, "publication acknowledgment differs")


def validate_observation(value, case, *, reader_required=True):
    if type(value) is dict and "operational_measurement" in value:
        require(case == "publish-complete" and type(value.get("keeper_ssh_release")) is dict
                and value["keeper_ssh_release"].get("mode") == "normal",
                "operational measurement selection differs")
        validate_operational_measurement(value["operational_measurement"])
    if type(value) is dict and "keeper_ssh_release" in value:
        require(case == "publish-complete" and "initial_opening_permit" in value
                and not any(field in value for field in
                            ("keeper_opening_release", "opening_permit_consumption", "opening_permit_check")),
                "keeper SSH release selection differs")
        validate_keeper_ssh_release(value["keeper_ssh_release"])
    if type(value) is dict and "keeper_opening_release" in value:
        require(case == "publish-complete" and "initial_opening_permit" in value
                and "opening_permit_consumption" not in value and "opening_permit_check" not in value,
                "keeper opening release selection differs")
        validate_keeper_opening_release(value["keeper_opening_release"])
    if type(value) is dict and "opening_permit_consumption" in value:
        require(case == "publish-complete" and "initial_opening_permit" in value
                and "opening_permit_check" not in value, "consumed opening selection differs")
        validate_consumed_opening(value["opening_permit_consumption"])
    if type(value) is dict and "initial_opening_permit" in value:
        require(value.get("reader_check", "stable") == "stable" and value.get("mount_check") is None
                and "live_mount" not in value and "opening_permit_check" not in value
                and not any(key in value for key in ("endpoint_proof_requested",
                    "attended_endpoint_proof_requested", "active_endpoint_proof_requested")),
                "initial opening permit selection differs")
        validate_initial_opening_permit(value["initial_opening_permit"], case)
    if type(value) is dict and "opening_permit_check" in value:
        require(case == "publish-complete" and value.get("reader_check", "stable") == "stable"
                and value.get("mount_check") is None and "live_mount" not in value
                and not any(key in value for key in (
                    "endpoint_proof_requested", "attended_endpoint_proof_requested",
                    "active_endpoint_proof_requested")), "opening permit selection differs")
        validate_opening_permit_check(value["opening_permit_check"])
    if type(value) is dict and (value.get("mount_check") is not None or "live_mount" in value):
        require(case in ("publish-invalidate-before-package", "publish-complete"),
                "mount observation uses an unrelated native case")
    if type(value) is dict:
        validate_endpoint_observation(value, case)
    if case in MULTI_CASES:
        import creation_multi_probe as multi
        return multi.validate_observation(value, case)
    if case in DEATH_CASES:
        return death.validate_observation(value, case, reader_required=reader_required)
    require(type(value) is dict and case in PRODUCER_CASES and value.get("case") == case
            and type(value.get("version")) is int and value["version"] == 1
            and value.get("experiment") == "rooted-native-creation" and value.get("status") == "observed",
            "native observation identity differs")
    require(all(value.get(field) is False for field in FLAGS), "observation claims authority")
    for field in ("process_exit_observed", "terminal_restored", "standard_streams_clean", "helper_unchanged",
                  "sources_unchanged", "root_binding_unchanged", "process_identity_verified",
                  "private_terminal_observed", "secret_boundaries_checked", "original_package_verified",
                  "independent_exits_observed",
                  "private_loader_terminal_observed", "native_identity_observed", "provider_revalidated",
                  "retained_connection_refused", "new_connection_refused"):
        require(value.get(field) is True, "native observation evidence missing")
    for field in ("helper_sha256", "observer_sha256", "real_binding_sha256", "decoy_binding_sha256", "openssl_sha256"):
        require(draft.fixed.hexadecimal(value.get(field), 64), "native observation provenance differs")
    record = draft.parse_record(value.get("record_canonical"))
    require(record["installation_id"] == value.get("selected_installation_id"), "native selected root differs")
    environment_verified = validate_process_metadata(value.get("native_process_metadata"))
    require(value.get("native_environment_absence_verified") is environment_verified,
            "native environment aggregate claim differs")
    validate_process_identity(value.get("native_process_identity"), value.get("helper_pid"))
    publication = case in PUBLICATION_OUTCOMES
    complete, abandoned = case == "complete" or publication, case == "abandon-after-load"
    validate_abandonment(value, abandoned)
    for field, expected in (("native_binding_verified", complete), ("independent_signature_verified", complete),
                            ("altered_message_refused", complete), ("closure_acknowledged", not abandoned),
                            ("preparation_dropped", not abandoned),
                            ("mutation_observed", case in ("invalidate-before-proof", "publish-invalidate-before-package", "publish-invalidate-package", *RETAINED_CASES)),
                            ("invalidated_stage_preserved", case in ("invalidate-before-proof", "publish-invalidate-before-package"))):
        require(value.get(field) is expected, "native outcome evidence differs")
    if not abandoned:
        final = parse_packet(draft.canonical(value.get("final")), case, value.get("helper_pid"), "final")
        require(final["record_canonical"] == value["record_canonical"], "observation final record changed")
    preserved = namespaces.validate_summary(value.get("preserved_namespace"))
    require(value.get("namespace_preserved_verified") is True, "preserved namespace evidence missing")
    prior_packages = {item["name"] for item in preserved["packages"]}
    prior_admissions = {item["name"] for item in preserved["admissions"]}
    if publication:
        validate_publication_outcome(value, case)
        _, package, record_published, _ = PUBLICATION_OUTCOMES[case]
        for field, expected in (("publication_filesystem_verified", True), ("independent_closed_before_publication", True),
                                ("package_durable_observed", case not in ("publish-package-failure", "publish-invalidate-before-package", CONTENDED_CASE)),
                                ("creation_record_verified", record_published and case != "publish-invalidate-retained-record"),
                                ("pending_creation_record_observed", case == "publish-record-before-rename-failure"),
                                ("final_package_mutation_preserved", case in ("publish-invalidate-package", "publish-invalidate-retained-package")),
                                ("final_slot_absent", not package and record["credential_id"] not in prior_packages),
                                ("admissions_empty", not prior_admissions and not record_published and case != "publish-record-before-rename-failure")):
            require(value.get(field) is expected, "publication filesystem evidence differs")
        require(value.get("retained_publication_mutation_preserved", False) is (case in RETAINED_CASES),
                "retained publication mutation evidence differs")
        require(value.get("contended_stage_preserved", False) is (case == CONTENDED_CASE),
                "contended private stage evidence differs")
    else:
        require(value.get("admissions_empty") is (not prior_admissions) and value.get("final_slot_absent") is True,
                "unpublished native fixture membership differs")
    if reader_required:
        joint.validate_observation(value)
    if value.get("mount_check") is not None or "live_mount" in value:
        import creation_mount_probe as mounts
        mounts.validate_observation(value)
    return value


def endpoint_packet(payload, role, secret):
    require(type(payload) is bytes and secret not in payload and payload.endswith(b"\n")
            and payload.count(b"\n") == 1 and len(payload) <= 1024,
            "endpoint process output boundary differs")
    value = draft.decode(payload, 1024)
    require(type(value) is dict
            and set(value) == {"endpoint_role", "generation", "signature_verified", "session_closed"}
            and value.get("endpoint_role") == role and value.get("signature_verified") is True
            and value.get("session_closed") is (role == "custodian")
            and draft.fixed.hexadecimal(value.get("generation"), 64)
            and value["generation"] != "0" * 64,
            "endpoint process evidence differs")
    return value


def validate_endpoint_evidence(value, input_flags):
    flags = ENDPOINT_PROOF_FLAGS | input_flags
    require(type(value) is dict and set(value) == {"generation", *flags}
            and draft.fixed.hexadecimal(value.get("generation"), 64)
            and value["generation"] != "0" * 64
            and all(value.get(field) is True for field in flags),
            "endpoint proof evidence missing")
    return value


def validate_endpoint_proof(value):
    return validate_endpoint_evidence(value, {"private_secret_pipe_verified"})


def validate_attended_endpoint_proof(value):
    return validate_endpoint_evidence(value, {"attended_terminal_verified", "terminal_restored"})


def validate_active_endpoint_proof(value):
    fields = ACTIVE_ENDPOINT_PROOF_FLAGS | {"predecessor_generation", "successor_generation"}
    require(type(value) is dict and set(value) == fields
            and all(value.get(field) is True for field in ACTIVE_ENDPOINT_PROOF_FLAGS)
            and draft.fixed.hexadecimal(value.get("predecessor_generation"), 64)
            and draft.fixed.hexadecimal(value.get("successor_generation"), 64)
            and value["predecessor_generation"] != "0" * 64
            and value["successor_generation"] != "0" * 64
            and value["predecessor_generation"] != value["successor_generation"],
            "active endpoint proof evidence missing")
    return value


def validate_endpoint_observation(value, case):
    pipe = "endpoint_proof_requested" in value or "endpoint_proof" in value
    attended = any(field in value for field in (
        "attended_endpoint_proof_requested", "attended_endpoint_proof", ENDPOINT_WRONG_SECRET_REFUSED))
    active = "active_endpoint_proof_requested" in value or "active_endpoint_proof" in value
    if pipe or attended or active:
        require(sum((pipe, attended, active)) == 1 and case == "publish-complete"
                and value.get("reader_check", "stable") == "stable"
                and value.get("mount_check") is None and "live_mount" not in value,
                "endpoint proof selection differs")
        if active:
            require(value.get("active_endpoint_proof_requested") is True,
                    "active endpoint proof selection differs")
            validate_active_endpoint_proof(value.get("active_endpoint_proof"))
        elif attended:
            require(value.get("attended_endpoint_proof_requested") is True
                    and value.get(ENDPOINT_WRONG_SECRET_REFUSED) is True,
                    "attended endpoint proof selection differs")
            validate_attended_endpoint_proof(value.get("attended_endpoint_proof"))
        else:
            require(value.get("endpoint_proof_requested") is True, "endpoint proof selection differs")
            validate_endpoint_proof(value.get("endpoint_proof"))


def validate_endpoint_selection(args):
    pipe = getattr(args, "endpoint_proof", False)
    attended = getattr(args, "endpoint_attended", False)
    active = getattr(args, "endpoint_active", False)
    require(sum((pipe, attended, active)) <= 1, "endpoint proof modes are mutually exclusive")
    if pipe or attended or active:
        require(args.case == "publish-complete" and args.reader_check == "stable"
                and getattr(args, "mount_check", None) is None and args.reader is not None,
                "endpoint proof requires a stable complete publication")
    return "active" if active else "attended" if attended else "pipe" if pipe else None


def validate_opening_permit_selection(args):
    selected = getattr(args, "opening_permit_check", False)
    if selected:
        require(args.case == "publish-complete" and args.reader_check == "stable"
                and args.reader is not None and getattr(args, "mount_check", None) is None
                and validate_endpoint_selection(args) is None
                and not getattr(args, "publish_initial_opening_permit", False),
                "opening permit requires a stable complete publication without endpoints")
    return selected


def validate_initial_opening_selection(args):
    selected = getattr(args, "publish_initial_opening_permit", False)
    if selected:
        require(args.case in ("publish-complete", "publish-record-before-rename-failure")
                and args.reader_check == "stable" and args.reader is not None
                and getattr(args, "mount_check", None) is None
                and not getattr(args, "opening_permit_check", False)
                and validate_endpoint_selection(args) is None,
                "initial opening permit requires a supported stable publication")
    return selected


def validate_consumed_opening_selection(args):
    mode = getattr(args, "consume_initial_opening_permit", None)
    if mode is not None:
        require(mode in ("normal", "sync-error") and validate_initial_opening_selection(args)
                and getattr(args, "keeper_opening_release", None) is None
                and getattr(args, "keeper_ssh_release", None) is None
                and args.case == "publish-complete", "consumed opening selection differs")
    return mode


def validate_keeper_opening_release_selection(args):
    mode = getattr(args, "keeper_opening_release", None)
    if mode is not None:
        require(mode in ("normal", "missing", "keeper-lost") and validate_initial_opening_selection(args)
                and args.case == "publish-complete"
                and getattr(args, "consume_initial_opening_permit", None) is None
                and getattr(args, "keeper_ssh_release", None) is None,
                "keeper opening release selection differs")
    return mode


def validate_keeper_ssh_release_selection(args):
    mode = getattr(args, "keeper_ssh_release", None)
    if mode is not None:
        require(mode in ("normal", "disconnect", "unconfirmed") and validate_initial_opening_selection(args)
                and args.case == "publish-complete"
                and getattr(args, "consume_initial_opening_permit", None) is None
                and getattr(args, "keeper_opening_release", None) is None,
                "keeper SSH release selection differs")
    return mode


CREATION_MEASUREMENT_FIELDS = {
    "spawn_to_ack_prompt_ns", "ack_prompt_to_confirmation_ns",
    "confirmation_to_verified_publication_ns", "publication_to_independent_admission_ns",
}
SSH_MEASUREMENT_FIELDS = {
    "transport_to_predecessor_channel_ns", "predecessor_channel_to_prompt_ns",
    "predecessor_prompt_to_input_ns", "predecessor_input_to_endpoint_ns",
    "predecessor_endpoint_to_verified_signature_ns", "predecessor_release_to_permit_ns",
    "permit_observed_to_predecessor_exit_ns", "successor_transport_to_channel_ns",
    "successor_channel_to_prompt_ns", "successor_prompt_to_input_ns",
    "successor_input_to_endpoint_ns", "successor_endpoint_to_verified_signature_ns",
    "successor_release_to_exit_ns", "refusal_transport_to_channel_ns", "refusal_channel_to_exit_ns",
}


def validate_operational_measurement_selection(args):
    selected = getattr(args, "measure_operational_stages", False)
    require(not selected or getattr(args, "keeper_ssh_release", None) == "normal",
            "operational measurement requires normal keeper SSH release")
    return selected


def validate_operational_measurement(value):
    require(type(value) is dict and set(value) == {"clock", "creation", "ssh"}
            and value["clock"] == "monotonic-ns", "operational measurement clock differs")
    for section, fields in (("creation", CREATION_MEASUREMENT_FIELDS), ("ssh", SSH_MEASUREMENT_FIELDS)):
        durations = value[section]
        require(type(durations) is dict and set(durations) == fields
                and all(type(duration) is int and 0 <= duration <= 240_000_000_000
                        for duration in durations.values()), "operational measurement stages differ")
    return value


def validate_keeper_ssh_release(value):
    fields = {"mode", "ssh_pty_verified", "no_pty_permit_preserved", "supervisor_start_refused_foreign_inputs",
              "predecessor_signed", "initial_permit_consumed",
              "reconnect_refused_before_prompt", "descendant_exits_observed", "clean_close_published",
              "successor_signature_verified", "disconnect_observed", "unconfirmed_closure_refused", "authority"}
    require(type(value) is dict and set(value) == fields and value.get("mode") in ("normal", "disconnect", "unconfirmed")
            and all(value.get(field) is True for field in
                    ("ssh_pty_verified", "no_pty_permit_preserved", "supervisor_start_refused_foreign_inputs",
                     "predecessor_signed", "initial_permit_consumed",
                     "reconnect_refused_before_prompt", "descendant_exits_observed"))
            and all(value.get(field) is (value["mode"] == "normal") for field in
                    ("clean_close_published", "successor_signature_verified"))
            and value.get("disconnect_observed") is (value["mode"] == "disconnect")
            and value.get("unconfirmed_closure_refused") is (value["mode"] == "unconfirmed")
            and value.get("authority") is False, "keeper SSH release evidence differs")
    return value


def validate_keeper_opening_release(value):
    fields = {"mode", "predecessor_signed", "consumed_before_input", "terminal_restored",
              "missing_permit_refused", "clean_close_published", "successor_signature_verified",
              "distinct_generation_verified", "keeper_death_observed", "authority"}
    require(type(value) is dict and set(value) == fields
            and value.get("mode") in ("normal", "missing", "keeper-lost")
            and all(value.get(field) is True for field in
                    ("predecessor_signed", "consumed_before_input", "terminal_restored", "missing_permit_refused"))
            and all(value.get(field) is (value["mode"] == "normal") for field in
                    ("clean_close_published", "successor_signature_verified", "distinct_generation_verified"))
            and value.get("keeper_death_observed") is (value["mode"] == "keeper-lost")
            and value.get("authority") is False, "keeper opening release evidence differs")
    return value


def validate_consumed_opening(value):
    fields = {"mode", "invalid_permit_refused", "consumed_before_input", "missing_permit_refused",
              "terminal_restored", "signature_verified", "sync_error_refused", "authority"}
    require(type(value) is dict and set(value) == fields and value.get("mode") in ("normal", "sync-error")
            and all(value.get(field) is True for field in
                    ("consumed_before_input", "missing_permit_refused", "terminal_restored"))
            and value.get("invalid_permit_refused") is (value["mode"] == "normal")
            and value.get("authority") is False
            and value.get("signature_verified") is (value["mode"] == "normal")
            and value.get("sync_error_refused") is (value["mode"] == "sync-error"),
            "consumed opening evidence differs")
    return value


def validate_initial_opening_permit(value, case):
    fields = {"permit_durable_before_admission", "package_lock_contended", "orphan_refused",
              "permit_unchanged", "producer_exit_observed", "final_reader", "authority"}
    require(case in ("publish-complete", "publish-record-before-rename-failure")
            and type(value) is dict and set(value) == fields
            and all(value.get(field) is True for field in fields - {"final_reader", "authority"})
            and value.get("authority") is False
            and value.get("final_reader") == ("first-open" if case == "publish-complete" else "refused"),
            "initial opening permit evidence differs")
    return value


def check_initial_opening_reader(args, credential, runtime, expected):
    result = subprocess.run([str(args.reader), "--opening-permit-check", credential],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        cwd="/", env=life.private_env(runtime), timeout=10, check=False)
    require(len(result.stdout) <= 256 and not result.stderr
            and result.stdout.endswith(b"\n") and result.stdout.count(b"\n") == 1,
            "initial opening permit reader streams differ")
    value = draft.decode(result.stdout, 256)
    require(type(value) is dict and value == {"opening_permit": expected, "authority": False}
            and value["authority"] is False and result.returncode == (1 if expected == "refused" else 0),
            "initial opening permit reader outcome differs")


def initial_opening_checkpoint(args, state, credential, canonical, packages, admissions, pidfds, runtime):
    import fcntl

    collection = draft.USER / "state/openings-v1"
    metadata = collection.lstat()
    require((metadata.st_dev, metadata.st_ino) == state["collection_identity"]
            and metadata.st_dev == os.fstat(packages).st_dev
            and stat.S_ISDIR(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o700
            and metadata.st_uid == metadata.st_gid == 65534 and not os.listxattr(collection),
            "initial opening collection changed")
    require(credential not in os.listdir(admissions)
            and all(life.pidfd_ready(descriptor, 0) for descriptor in pidfds.values()),
            "initial opening permit preceded closure or followed admission")
    descriptor = os.open(credential, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC,
                         dir_fd=state["directory"])
    state["permit"] = descriptor
    metadata = os.fstat(descriptor)
    payload = b"VVCOPEN\x00" + bytes((1, 1)) + bytes(6) + hashlib.sha256(canonical).digest()
    require(stat.S_ISREG(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o600
            and metadata.st_uid == metadata.st_gid == 65534 and metadata.st_nlink == 1
            and metadata.st_size == len(payload) and not os.listxattr(descriptor)
            and os.pread(descriptor, 49, 0) == payload,
            "initial opening permit differs from the committed creation")
    state["permit_identity"] = (metadata.st_dev, metadata.st_ino)
    state["payload"] = payload
    contender = os.open(credential, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                        dir_fd=packages)
    try:
        try:
            fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            require(error.errno == errno.EWOULDBLOCK, "initial opening contention errno differs")
        else:
            require(False, "initial opening package reservation was not retained")
    finally:
        os.close(contender)
    check_initial_opening_reader(args, credential, runtime, "refused")


def finish_initial_opening(args, state, credential, runtime):
    require(state.get("permit") is not None, "initial opening checkpoint missing")
    collection = draft.USER / "state/openings-v1"
    current = collection.lstat()
    require((current.st_dev, current.st_ino) == state["collection_identity"],
            "initial opening collection changed after publication")
    retained = os.fstat(state["permit"])
    named = os.stat(credential, dir_fd=state["directory"], follow_symlinks=False)
    require((retained.st_dev, retained.st_ino) == state["permit_identity"]
            == (named.st_dev, named.st_ino)
            and stat.S_ISREG(named.st_mode) and stat.S_IMODE(named.st_mode) == 0o600
            and named.st_uid == named.st_gid == 65534 and named.st_nlink == 1
            and named.st_size == 48 and not os.listxattr(state["permit"])
            and os.pread(state["permit"], 49, 0) == state["payload"],
            "initial opening permit changed after producer exit")
    expected = "first-open" if args.case == "publish-complete" else "refused"
    check_initial_opening_reader(args, credential, runtime, expected)
    return validate_initial_opening_permit(dict(permit_durable_before_admission=True,
        package_lock_contended=True, orphan_refused=True, permit_unchanged=True,
        producer_exit_observed=True, final_reader=expected, authority=False), args.case)


def validate_opening_permit_check(value):
    require(type(value) is dict and set(value) == {"cases", "record_unchanged", "authority"}
            and value.get("authority") is False and value.get("record_unchanged") is True
            and type(value.get("cases")) is dict and value["cases"] == OPENING_PERMIT_CASES,
            "opening permit evidence differs")
    return value


def observe_opening_permit_check(args, runtime, report):
    credential = draft.parse_record(report["record_canonical"])["credential_id"]
    record_path = draft.ADMISSIONS / credential / "record.json"
    canonical = record_path.read_bytes()
    require(canonical == report["record_canonical"].encode("ascii"),
            "opening permit canonical record differs")
    collection = draft.USER / "state/openings-v1"
    require(not collection.exists() and not collection.is_symlink(),
            "opening permit fixture collection already exists")
    target = Path(tempfile.mkdtemp(prefix=".opening-target-", dir=collection.parent))
    created = {}
    collection_identity = None
    results = {}

    def verify_record():
        require(record_path.read_bytes() == canonical, "opening permit changed the committed record")

    def check(case):
        verify_record()
        result = subprocess.run(
            [str(args.reader), "--opening-permit-check", credential],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            cwd="/", env=life.private_env(runtime), timeout=10, check=False)
        require(len(result.stdout) <= 256 and not result.stderr
                and result.stdout.endswith(b"\n") and result.stdout.count(b"\n") == 1,
                "opening permit helper streams differ")
        value = draft.decode(result.stdout, 256)
        expected = OPENING_PERMIT_CASES[case]
        require(type(value) is dict and set(value) == {"opening_permit", "authority"}
                and value.get("opening_permit") == expected and value.get("authority") is False
                and result.returncode == (1 if expected == "refused" else 0),
                "opening permit helper outcome differs")
        verify_record()
        results[case] = expected

    def track(path):
        metadata = path.lstat()
        created[path] = (metadata.st_dev, metadata.st_ino)

    def remove(path):
        metadata = path.lstat()
        require((metadata.st_dev, metadata.st_ino) == created.pop(path),
                "opening permit fixture object changed")
        path.unlink()

    def write(path, payload, mode=0o600):
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, mode)
        try:
            track(path)
            require(os.write(descriptor, payload) == len(payload), "opening permit fixture write was short")
            os.fchmod(descriptor, mode)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    permit = collection / credential
    payload = b"VVCOPEN\x00" + bytes((1, 1)) + bytes(6) + hashlib.sha256(canonical).digest()
    try:
        check("missing-collection")
        write(target / credential, payload)
        collection.symlink_to(target.name, target_is_directory=True)
        track(collection)
        check("symlink-collection")
        remove(collection)
        collection.mkdir(mode=0o700)
        metadata = collection.lstat()
        collection_identity = (metadata.st_dev, metadata.st_ino)
        check("missing-permit")
        file_target = collection / "permit-target"
        write(file_target, payload)
        permit.symlink_to(file_target.name)
        track(permit)
        check("symlink-permit")
        remove(permit)
        remove(file_target)
        write(permit, payload)
        linked = collection / "hardlink"
        os.link(permit, linked)
        track(linked)
        check("hardlink-permit")
        remove(linked)
        remove(permit)
        for case, contents, mode in (
            ("wrong-mode", payload, 0o640),
            ("wrong-digest", payload[:-1] + bytes((payload[-1] ^ 1,)), 0o600),
            ("wrong-version", payload[:8] + b"\x02" + payload[9:], 0o600),
            ("wrong-width", payload[:-1], 0o600),
            ("wrong-kind", payload[:9] + b"\x03" + payload[10:], 0o600),
            ("reserved-byte", payload[:10] + b"\x01" + payload[11:], 0o600),
            ("first-open", payload, 0o600),
            ("clean-close", payload[:9] + b"\x02" + payload[10:], 0o600),
        ):
            write(permit, contents, mode)
            check(case)
            require(permit.read_bytes() == contents, "opening permit reader modified its input")
            remove(permit)
        return validate_opening_permit_check(dict(cases=results, record_unchanged=True, authority=False))
    finally:
        for path in list(created):
            remove(path)
        if collection_identity is not None:
            metadata = collection.lstat()
            require((metadata.st_dev, metadata.st_ino) == collection_identity,
                    "opening permit fixture collection changed")
            collection.rmdir()
        target.rmdir()
        verify_record()


def endpoint_failure(payload):
    value = draft.decode(payload, 1024)
    if type(value) is dict and set(value) == {"endpoint_failure", "kind"}:
        phase, kind = value["endpoint_failure"], value["kind"]
        require(phase in ENDPOINT_FAILURE_PHASES and type(kind) is str and kind.isascii()
                and kind.isalpha() and kind[:1].isupper() and len(kind) <= 40,
                "unrecognized endpoint failure diagnostic")
        return {"phase": phase, "kind": kind}
    return None


def record_endpoint_failure(report, role, payload):
    diagnostic = endpoint_failure(payload)
    if diagnostic is not None:
        report.setdefault("endpoint_failures", {})[role] = diagnostic


def validate_wrong_endpoint_secret_refusal(code, payload, secret, wrong_secret):
    require(type(code) is int and code == 1 and type(payload) is bytes
            and secret not in payload and wrong_secret not in payload
            and payload.endswith(b"\n") and payload.count(b"\n") == 1,
            "wrong endpoint secret refusal stream or exit differs")
    failure = endpoint_failure(payload)
    require(failure is not None and failure["phase"] == "attended-unlock"
            and failure["kind"] in ("InvalidData", "Other", "TimedOut", "Uncategorized"),
            "wrong endpoint secret refusal diagnostic differs")
    return True


def endpoint_process_pending(process, deadline, report, missing):
    if process.poll() is not None:
        payload = spawn.bounded_output(process, deadline)
        if payload:
            record_endpoint_failure(report, "custodian", payload)
        require(False, missing)
    require(time.monotonic() < deadline, missing)


def setup_endpoint_terminal(slave):
    generated.child_setup(slave)
    os.close(slave)


def setup_fsync_failure_terminal(slave):
    import ctypes
    import platform

    require(platform.machine() == "x86_64", "fsync filter requires x86_64")
    setup_endpoint_terminal(slave)

    class Filter(ctypes.Structure):
        _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte),
                    ("jf", ctypes.c_ubyte), ("value", ctypes.c_uint)]

    class Program(ctypes.Structure):
        _fields_ = [("length", ctypes.c_ushort), ("filters", ctypes.POINTER(Filter))]

    instructions = (Filter * 7)(
        Filter(0x20, 0, 0, 4), Filter(0x15, 1, 0, 0xC000003E),
        Filter(0x06, 0, 0, 0x80000000), Filter(0x20, 0, 0, 0),
        Filter(0x15, 0, 1, 74), Filter(0x06, 0, 0, 0x00050000 | errno.EIO),
        Filter(0x06, 0, 0, 0x7FFF0000))
    program = Program(len(instructions), instructions)
    libc = ctypes.CDLL(None, use_errno=True)
    require(libc.prctl(38, 1, 0, 0, 0) == 0
            and libc.prctl(22, 2, ctypes.byref(program), 0, 0) == 0,
            "fsync failure filter was not installed")


def observe_opening_refusal(args, credential, runtime, expected_kind, *, sync_error=False):
    import pty
    import termios

    home = Path(tempfile.mkdtemp(prefix="opening-refusal-", dir=runtime))
    home.chmod(0o700)
    master, slave = pty.openpty()
    os.fchmod(slave, 0o600)
    os.set_blocking(master, False)
    original = termios.tcgetattr(slave)
    process = None
    completed = False
    deadline = time.monotonic() + 20
    try:
        setup = setup_fsync_failure_terminal if sync_error else setup_endpoint_terminal
        process = subprocess.Popen(
            [str(args.reader), "--endpoint-permitted-custodian", credential, str(home)],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=life.private_env(home), cwd="/", close_fds=True,
            preexec_fn=lambda: setup(slave), pass_fds=(slave,))
        with selectors.DefaultSelector() as selector:
            selector.register(master, selectors.EVENT_READ)
            while process.poll() is None:
                require(time.monotonic() < deadline and not list(home.iterdir()),
                        "opening refusal created runtime resources or timed out")
                for _, _ in selector.select(0.02):
                    try:
                        chunk = os.read(master, 4096)
                    except BlockingIOError:
                        continue
                    require(not chunk, "opening refusal emitted a terminal prompt")
        output = spawn.bounded_output(process, deadline)
        expected = json.dumps({"endpoint_failure": "opening-permit", "kind": expected_kind},
                              sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
        require(process.returncode == 1 and output == expected and not list(home.iterdir()),
                "opening refusal output or exit differs")
        verify_attended_endpoint_terminal(master, slave, original)
        home.rmdir()
        completed = True
    finally:
        os.close(master)
        os.close(slave)
        if process is not None:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            for stream in (process.stdout, process.stderr):
                stream.close()
        if not completed:
            UNCERTAIN.append(home)


def observe_consumed_opening(args, credential, secret, runtime, report):
    mode = validate_consumed_opening_selection(args)
    permit = draft.USER / "state/openings-v1" / credential
    original = permit.read_bytes()
    record = (draft.ADMISSIONS / credential / "record.json").read_bytes()
    expected = b"VVCOPEN\x00" + bytes((1, 1)) + bytes(6) + hashlib.sha256(record).digest()
    metadata = permit.lstat()
    require(original == expected and stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1,
            "consumption did not receive the producer's first permit")
    if mode == "normal":
        descriptor = os.open(permit, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            damaged = original[:-1] + bytes((original[-1] ^ 1,))
            require(os.pwrite(descriptor, damaged, 0) == len(damaged), "permit mutation was short")
            os.fsync(descriptor)
            observe_opening_refusal(args, credential, runtime, "InvalidData")
            require(permit.read_bytes() == damaged, "invalid permit refusal changed its bytes")
            require(os.pwrite(descriptor, original, 0) == len(original), "permit restoration was short")
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

        def consumed():
            require(not permit.exists() and not permit.is_symlink(),
                    "opening prompt preceded permit consumption")

        proof = observe_endpoint_proof(args, credential, secret, runtime, report,
                                       attended=True, permitted=True, before_input=consumed)
        require(proof["signature_verified"] is True and proof["terminal_restored"] is True,
                "permitted opening proof is incomplete")
    else:
        observe_opening_refusal(args, credential, runtime, "Uncategorized", sync_error=True)
    require(not permit.exists() and not permit.is_symlink(), "opening attempt retained its consumed permit")
    observe_opening_refusal(args, credential, runtime, "NotFound")
    require(not permit.exists() and not permit.is_symlink()
            and (draft.ADMISSIONS / credential / "record.json").read_bytes() == record,
            "missing permit refusal changed the admission")
    return validate_consumed_opening(dict(mode=mode, invalid_permit_refused=mode == "normal",
        consumed_before_input=True, missing_permit_refused=True, terminal_restored=True,
        signature_verified=mode == "normal", sync_error_refused=mode == "sync-error", authority=False))


def send_attended_endpoint_input(process, master, slave, secret, deadline, report, *, replacement=None,
                                 before_input=None):
    import termios

    transcript = bytearray()
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(master, selectors.EVENT_READ)
            while transcript != ENDPOINT_UNLOCK_PROMPT:
                endpoint_process_pending(process, deadline, report, "endpoint unlock prompt was not observed")
                for _, _ in selector.select(0.02):
                    try:
                        chunk = os.read(master, len(ENDPOINT_UNLOCK_PROMPT) + 1)
                    except BlockingIOError:
                        continue
                    transcript.extend(chunk)
                    require(chunk and ENDPOINT_UNLOCK_PROMPT.startswith(transcript),
                            "endpoint terminal emitted unexpected output")
        echo_bits = sum(getattr(termios, name, 0) for name in
                        ("ECHO", "ECHONL", "ECHOE", "ECHOK", "ECHOKE", "ECHOCTL", "ECHOPRT"))
        require(os.getsid(process.pid) == os.tcgetpgrp(master) == process.pid
                and not termios.tcgetattr(slave)[3] & echo_bits,
                "endpoint unlock prompt is outside the private foreground terminal")
        generated.proc_secret_absent(process.pid, secret)
        if replacement is not None:
            generated.proc_secret_absent(process.pid, replacement)
        if before_input is not None:
            before_input()
        payload = bytearray(secret if replacement is None else replacement)
        payload.append(10)
        try:
            generated.send(master, payload, deadline)
        finally:
            generated.wipe(payload)
    finally:
        generated.wipe(transcript)


def verify_attended_endpoint_terminal(master, slave, original):
    import termios

    require(termios.tcgetattr(slave) == original, "endpoint unlock terminal was not restored")
    with selectors.DefaultSelector() as selector:
        selector.register(master, selectors.EVENT_READ)
        require(not selector.select(0), "endpoint terminal emitted output after the prompt")


def observe_endpoint_proof(args, credential, secret, runtime, report, *, attended=False, reject_wrong_secret=False,
                           permitted=False, before_input=None):
    require(not reject_wrong_secret or attended, "wrong endpoint secret requires a terminal")
    require(not permitted or attended and not reject_wrong_secret, "permitted endpoint selection differs")
    home = Path(tempfile.mkdtemp(prefix="endpoint-", dir=runtime))
    home.chmod(0o700)
    process = verifier = None
    secret_read = secret_write = None
    master = slave = None
    wrong_secret = None
    completed = False
    deadline = time.monotonic() + (60 if attended else 45)
    try:
        env = life.private_env(home)
        if attended:
            import pty
            import termios

            require(len(secret) == 64, "endpoint unlock input length differs")
            master, slave = pty.openpty()
            os.fchmod(slave, 0o600)
            os.set_blocking(master, False)
            original = termios.tcgetattr(slave)

            mode = "--endpoint-permitted-custodian" if permitted else "--endpoint-attended-custodian"
            command = [str(args.reader), mode, credential, str(home)]
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, env=env, cwd="/", close_fds=True,
                                       preexec_fn=lambda: setup_endpoint_terminal(slave), pass_fds=(slave,))
            if reject_wrong_secret:
                wrong_secret = bytearray(secret)
                wrong_secret[0] = ord("1") if wrong_secret[0] == ord("0") else ord("0")
            send_attended_endpoint_input(process, master, slave, secret, deadline, report,
                                         replacement=wrong_secret, before_input=before_input)
        else:
            secret_read, secret_write = os.pipe2(os.O_CLOEXEC)
            require(0 < len(secret) <= 512 and os.write(secret_write, secret) == len(secret),
                    "endpoint secret pipe boundary differs")
            os.close(secret_write)
            secret_write = None
            command = [str(args.reader), "--endpoint-native-custodian", credential,
                       str(home), str(secret_read)]
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, env=env, close_fds=True,
                                       pass_fds=(secret_read,))
            os.close(secret_read)
            secret_read = None
        endpoint = home / "endpoint.sock"
        if reject_wrong_secret:
            while process.poll() is None:
                require(time.monotonic() < deadline and not endpoint.exists(),
                        "wrong endpoint secret opened a socket or failed to exit")
                time.sleep(0.01)
            payload = spawn.bounded_output(process, deadline)
            refused = validate_wrong_endpoint_secret_refusal(process.returncode, payload, secret, wrong_secret)
            verify_attended_endpoint_terminal(master, slave, original)
            require(not list(home.iterdir()), "wrong endpoint secret retained runtime resources")
            home.rmdir()
            completed = True
            return refused
        while not endpoint.exists():
            endpoint_process_pending(process, deadline, report, "endpoint custodian did not open its socket")
            time.sleep(0.01)
        metadata = endpoint.lstat()
        require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid(),
                "endpoint socket ownership differs")
        verifier = subprocess.Popen(
            [str(args.reader), "--endpoint-native-verifier", credential, str(home)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env, close_fds=True)
        require(process.pid != verifier.pid, "endpoint roles share one process")
        messages = [(role, spawn.bounded_output(child, deadline, first_line=True))
                    for role, child in (("custodian", process), ("verifier", verifier))]
        for role, payload in messages:
            record_endpoint_failure(report, role, payload)
        custodian_report, verifier_report = (endpoint_packet(payload, role, secret)
                                             for role, payload in messages)
        require(custodian_report["generation"] == verifier_report["generation"]
                and process.poll() is None and verifier.poll() is None,
                "endpoint generation or retained process differs")
        if attended:
            verify_attended_endpoint_terminal(master, slave, original)
        for child in (process, verifier):
            child.stdin.write(b"G")
            child.stdin.flush()
        for child in (process, verifier):
            require(spawn.bounded_output(child, deadline) == b""
                    and child.wait(timeout=max(0.001, deadline - time.monotonic())) == 0,
                    "endpoint process exit or trailing stream differs")
        require(not list(home.iterdir()), "endpoint runtime retained resources")
        if attended:
            verify_attended_endpoint_terminal(master, slave, original)
        home.rmdir()
        completed = True
        proof = dict(
            generation=custodian_report["generation"], signature_verified=True,
            distinct_processes_verified=True, process_exits_observed=True, session_closed=True,
            standard_streams_clean=True)
        if attended:
            return validate_attended_endpoint_proof(dict(proof, attended_terminal_verified=True, terminal_restored=True))
        return validate_endpoint_proof(dict(proof, private_secret_pipe_verified=True))
    finally:
        generated.wipe(wrong_secret)
        for descriptor in (secret_read, secret_write, master, slave):
            if descriptor is not None:
                os.close(descriptor)
        for child in (process, verifier):
            if child is not None:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=3)
                for stream in (child.stdin, child.stdout, child.stderr):
                    stream.close()
        if not completed:
            UNCERTAIN.append(home)


def active_endpoint_socket(path, process, deadline, report, missing):
    while not path.exists():
        endpoint_process_pending(process, deadline, report, missing)
        time.sleep(0.01)
    metadata = path.lstat()
    require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid(),
            "active endpoint socket ownership differs")


def release_endpoint_process(process, deadline):
    process.stdin.write(b"G")
    process.stdin.flush()
    require(spawn.bounded_output(process, deadline) == b""
            and process.wait(timeout=max(0.001, deadline - time.monotonic())) == 0,
            "active endpoint process exit or trailing stream differs")


def stop_disconnected_keeper(process, credential, deadline):
    import fcntl

    package = draft.directory(draft.PACKAGES / credential, os.geteuid(), 0o700)
    try:
        try:
            fcntl.flock(package, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            require(error.errno == errno.EWOULDBLOCK, "keeper reservation contention differs")
        else:
            require(False, "keeper lost its reservation before process exit")
        require(process.poll() is None, "keeper exited before the signal")
        process.kill()
        require(process.wait(timeout=max(0.001, deadline - time.monotonic())) == -LINUX_SIGKILL,
                "keeper exit signal differs")
        fcntl.flock(package, fcntl.LOCK_EX | fcntl.LOCK_NB)
    finally:
        os.close(package)


def parse_active_keeper_cut(payload, keeper_pid):
    require(type(payload) is bytes and len(payload) <= 256 and payload.endswith(b"\n")
            and payload.count(b"\n") == 1, "active keeper cut packet differs")
    value = json.loads(payload)
    require(type(value) is dict and set(value) == {"active_keeper_cut", "supervisor_pid"}
            and value["active_keeper_cut"] is True
            and type(value["supervisor_pid"]) is int
            and 1 < value["supervisor_pid"] <= 2147483647
            and value["supervisor_pid"] not in (keeper_pid, os.getpid()),
            "active keeper cut identity differs")
    return value["supervisor_pid"]


def stop_active_keeper(process, reader, credential, previous_home, successor_socket,
                       master, deadline, report):
    import fcntl
    import struct

    supervisor_pid = parse_active_keeper_cut(
        spawn.bounded_output(process, deadline, first_line=True), process.pid)
    paths = (previous_home / "endpoint.sock", previous_home / "agent.sock")
    identities = []
    for path in paths:
        metadata = path.lstat()
        require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid(),
                "active keeper socket identity differs")
        identities.append((metadata.st_dev, metadata.st_ino))
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(1)
        connection.connect(str(paths[1]))
        agent_pid, uid, gid = struct.unpack("3i", connection.getsockopt(
            socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
    require(uid == os.geteuid() and gid == os.getegid()
            and agent_pid not in (supervisor_pid, process.pid, os.getpid()),
            "active keeper agent peer differs")
    descriptors = []
    package = None
    try:
        roles = ((process.pid, reader, str(reader), os.getpid()),
                 (supervisor_pid, reader, "rust-launcher-probe", process.pid),
                 (agent_pid, "/usr/bin/ssh-agent", "/usr/bin/ssh-agent", supervisor_pid))
        for pid, executable, arg0, parent in roles:
            start = life.process_start(pid)
            descriptor = os.pidfd_open(pid)
            descriptors.append(descriptor)
            try:
                require(Path(life.executable(pid)).resolve(strict=True)
                        == Path(executable).resolve(strict=True),
                        "active keeper executable identity differs")
            except PermissionError:
                actual_arg0 = Path("/proc", str(pid), "cmdline").read_bytes().split(b"\0", 1)[0]
                require(actual_arg0 == os.fsencode(arg0), "active keeper command identity differs")
            credentials = life.agent_credentials(pid)
            require(credentials["PPid"] == [parent] and credentials["NoNewPrivs"] == [1]
                    and credentials["Uid"] == [os.geteuid()] * 4
                    and credentials["Gid"] == [os.getegid()] * 4
                    and life.process_start(pid) == start
                    and not life.pidfd_ready(descriptor, 0),
                    "active keeper process exited before the cut")
        package = draft.directory(draft.PACKAGES / credential, os.geteuid(), 0o700)
        try:
            fcntl.flock(package, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            require(error.errno == errno.EWOULDBLOCK, "active keeper lock contention differs")
        else:
            require(False, "active keeper package was available before the cut")
        require(process.poll() is None and not successor_socket.exists()
                and all(not life.pidfd_ready(descriptor, 0) for descriptor in descriptors),
                "active keeper cut changed before the signal")
        foreground = os.tcgetpgrp(master)
        require(foreground == process.pid == os.getpgid(supervisor_pid)
                and os.getpgid(agent_pid) == agent_pid,
                "active keeper terminal process groups differ")
        life.pidfd_signal(descriptors[0], LINUX_SIGKILL)
        statuses = {"keeper": process.wait(timeout=max(0.001, deadline - time.monotonic()))}
        report["active_keeper_death_statuses"] = statuses
        require(statuses["keeper"] == -LINUX_SIGKILL, "active keeper exit signal differs")
        for role, pid, descriptor in zip(("supervisor", "agent"), (supervisor_pid, agent_pid), descriptors[1:]):
            require(life.pidfd_ready(descriptor, min(5, max(0, deadline - time.monotonic()))),
                    "active keeper descendant remained alive")
            statuses[role] = os.waitstatus_to_exitcode(spawn.observe_exit(pid, descriptor, deadline))
        # The foreground supervisor can receive SIGHUP when its session leader dies.
        require(statuses["supervisor"] in (-LINUX_SIGKILL, -LINUX_SIGHUP)
                and statuses["agent"] == -LINUX_SIGKILL, "active keeper descendant exit differs")
        require(all(life.pidfd_ready(descriptor, 0) for descriptor in descriptors)
                and not successor_socket.exists()
                and spawn.bounded_output(process, deadline) == b"",
                "active keeper produced successor evidence after death")
        fcntl.flock(package, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for path, identity, socket_type in zip(paths, identities, (socket.SOCK_SEQPACKET, socket.SOCK_STREAM)):
            metadata = path.lstat()
            require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid()
                    and (metadata.st_dev, metadata.st_ino) == identity,
                    "active keeper stale socket identity changed")
            with socket.socket(socket.AF_UNIX, socket_type) as connection:
                connection.settimeout(1)
                try:
                    connection.connect(str(path))
                except OSError as error:
                    require(error.errno == errno.ECONNREFUSED,
                            "active keeper stale socket refusal differs")
                else:
                    require(False, "active keeper stale socket still accepted a connection")
            life.unlink_owned_socket(path)
        require(not list(previous_home.iterdir()), "active keeper left other runtime resources")
        previous_home.rmdir()
    finally:
        if package is not None:
            os.close(package)
        for descriptor in descriptors:
            os.close(descriptor)


def wait_active_closure_refusal(process, master, successor_socket, deadline, expected_output,
                                *, disconnected_credential=None):
    killed_keeper = False
    with selectors.DefaultSelector() as selector:
        selector.register(master, selectors.EVENT_READ)
        if disconnected_credential is not None:
            selector.register(process.stdout, selectors.EVENT_READ)
        while process.poll() is None:
            require(time.monotonic() < deadline and not successor_socket.exists(),
                    "active closure refusal opened a successor endpoint or timed out")
            for key, _ in selector.select(0.02):
                if key.fileobj is process.stdout:
                    require(spawn.bounded_output(process, deadline, first_line=True) == expected_output,
                            "disconnected keeper readiness differs")
                    stop_disconnected_keeper(process, disconnected_credential, deadline)
                    killed_keeper = True
                    break
                try:
                    chunk = os.read(master, 4096)
                except BlockingIOError:
                    continue
                require(not chunk, "active closure refusal emitted a successor terminal prompt")
    require(not successor_socket.exists(), "active closure refusal left a successor endpoint")
    payload = spawn.bounded_output(process, deadline)
    if disconnected_credential is not None:
        require(killed_keeper and process.returncode == -LINUX_SIGKILL and not payload,
                "disconnected keeper output or exit differs")
    else:
        require(process.returncode == 0 and payload == expected_output,
                "active closure refusal output or exit differs")


def observe_active_endpoint_case(args, credential, secret, runtime, report, case):
    require(case in ("missing", "incomplete", "different-generation", "connection-lost", "keeper-lost", "normal",
                     "permit-close", "permit-missing", "permit-keeper-lost"),
            "active endpoint case differs")
    import pty
    import termios

    home = Path(tempfile.mkdtemp(prefix="active-endpoint-" + case + "-", dir=runtime))
    home.chmod(0o700)
    process = verifier = None
    master = slave = None
    completed = False
    deadline = time.monotonic() + 90
    try:
        require(len(secret) == 64, "active endpoint unlock input length differs")
        env = life.private_env(home)
        master, slave = pty.openpty()
        os.fchmod(slave, 0o600)
        os.set_blocking(master, False)
        original = termios.tcgetattr(slave)
        process = subprocess.Popen(
            [str(args.reader), "--endpoint-active-custodian", credential, str(home), case],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env, cwd="/", close_fds=True,
            preexec_fn=lambda: setup_endpoint_terminal(slave), pass_fds=(slave,))
        permit = draft.USER / "state/openings-v1" / credential

        def consumed():
            require(not permit.exists() and not permit.is_symlink(),
                    "registered opening prompt preceded permit consumption")

        send_attended_endpoint_input(process, master, slave, secret, deadline, report,
                                     before_input=consumed if case.startswith("permit-") else None)

        previous_home = home / "previous"
        previous_socket = previous_home / "endpoint.sock"
        active_endpoint_socket(previous_socket, process, deadline, report,
                               "active endpoint predecessor did not open its socket")
        verifier = subprocess.Popen(
            [str(args.reader), "--endpoint-native-verifier", credential, str(previous_home)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env, close_fds=True)
        require(process.pid != verifier.pid, "active endpoint roles share one process")
        previous_payload = spawn.bounded_output(verifier, deadline, first_line=True)
        record_endpoint_failure(report, "active-" + case + "-predecessor-verifier", previous_payload)
        previous_report = endpoint_packet(previous_payload, "verifier", secret)
        require(verifier.poll() is None and process.poll() is None,
                "active endpoint predecessor exited before its closure check")
        release_endpoint_process(verifier, deadline)
        verifier = None

        process.stdin.write(b"G")
        process.stdin.flush()
        successor_socket = home / "endpoint.sock"
        if case in ("keeper-lost", "permit-keeper-lost"):
            stop_active_keeper(process, args.reader, credential, previous_home, successor_socket,
                               master, deadline, report)
            verify_attended_endpoint_terminal(master, slave, original)
            require(not list(home.iterdir()), "active keeper death retained runtime resources")
            home.rmdir()
            completed = True
            return previous_report["generation"]
        if case == "permit-close":
            require(spawn.bounded_output(process, deadline, first_line=True)
                    == b'{"clean_close_permit_published":true}\n',
                    "clean close publication reply differs")
            verify_attended_endpoint_terminal(master, slave, original)
            require(process.poll() is None and not successor_socket.exists(),
                    "clean close keeper exited or opened a successor")
            inspect_clean_close_permit(args, credential, runtime, report)
            release_endpoint_process(process, deadline)
            verify_attended_endpoint_terminal(master, slave, original)
            require(not list(home.iterdir()), "clean close keeper retained runtime resources")
            home.rmdir()
            completed = True
            return previous_report["generation"]
        if case != "normal":
            expected_output = {
                "missing": b'{"active_closure_missing_refused":true}\n',
                "incomplete": b'{"active_closure_refusal":"incomplete"}\n',
                "different-generation": b'{"active_closure_refusal":"different-generation"}\n',
                "connection-lost": b'{"active_closure_refusal":"connection-lost"}\n',
                "permit-missing": b'{"clean_close_permit_refused":true}\n',
            }[case]
            wait_active_closure_refusal(
                process, master, successor_socket, deadline, expected_output,
                disconnected_credential=credential if case == "connection-lost" else None)
            verify_attended_endpoint_terminal(master, slave, original)
            require(not list(home.iterdir()), "missing active closure retained runtime resources")
            home.rmdir()
            completed = True
            return previous_report["generation"]

        send_attended_endpoint_input(process, master, slave, secret, deadline, report)
        active_endpoint_socket(successor_socket, process, deadline, report,
                               "active endpoint successor did not open its socket")
        verifier = subprocess.Popen(
            [str(args.reader), "--endpoint-native-verifier", credential, str(home)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env, close_fds=True)
        require(process.pid != verifier.pid, "active successor roles share one process")
        custodian_payload = spawn.bounded_output(process, deadline, first_line=True)
        verifier_payload = spawn.bounded_output(verifier, deadline, first_line=True)
        record_endpoint_failure(report, "active-successor-custodian", custodian_payload)
        record_endpoint_failure(report, "active-successor-verifier", verifier_payload)
        custodian_report = endpoint_packet(custodian_payload, "custodian", secret)
        verifier_report = endpoint_packet(verifier_payload, "verifier", secret)
        require(custodian_report["generation"] == verifier_report["generation"]
                and custodian_report["generation"] != previous_report["generation"]
                and process.poll() is None and verifier.poll() is None,
                "active endpoint generation or retained process differs")
        verify_attended_endpoint_terminal(master, slave, original)
        release_endpoint_process(verifier, deadline)
        verifier = None
        release_endpoint_process(process, deadline)
        verify_attended_endpoint_terminal(master, slave, original)
        require(not list(home.iterdir()), "active successor retained runtime resources")
        home.rmdir()
        completed = True
        return previous_report["generation"], custodian_report["generation"]
    finally:
        for descriptor in (master, slave):
            if descriptor is not None:
                os.close(descriptor)
        for child in (process, verifier):
            if child is not None and child.poll() is None:
                child.kill()
                child.wait(timeout=3)
        for child in (process, verifier):
            if child is not None:
                for stream in (child.stdin, child.stdout, child.stderr):
                    if stream is not None and not stream.closed:
                        stream.close()
        if completed:
            require(not home.exists(), "active endpoint temporary home was not removed")


def observe_active_endpoint_proof(args, credential, secret, runtime, report):
    observe_active_endpoint_case(args, credential, secret, runtime, report, "missing")
    observe_active_endpoint_case(args, credential, secret, runtime, report, "incomplete")
    observe_active_endpoint_case(args, credential, secret, runtime, report, "different-generation")
    observe_active_endpoint_case(args, credential, secret, runtime, report, "connection-lost")
    observe_active_endpoint_case(args, credential, secret, runtime, report, "keeper-lost")
    predecessor_generation, successor_generation = observe_active_endpoint_case(
        args, credential, secret, runtime, report, "normal")
    return validate_active_endpoint_proof(dict(
        predecessor_signed=True,
        missing_closure_refused=True,
        incomplete_closure_refused=True,
        different_generation_closure_refused=True,
        lost_connection_closure_refused=True,
        reservation_reacquired_after_keeper_exit=True,
        live_keeper_death_observed=True,
        terminal_restored=True,
        distinct_generation_verified=True,
        successor_signature_verified=True,
        sessions_closed=True,
        process_exits_observed=True,
        predecessor_generation=predecessor_generation,
        successor_generation=successor_generation,
    ))


def inspect_clean_close_permit(args, credential, runtime, report):
    import fcntl

    canonical = report["record_canonical"].encode("ascii")
    permit = draft.USER / "state/openings-v1" / credential
    descriptor = os.open(permit, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        metadata = os.fstat(descriptor)
        expected = b"VVCOPEN\x00" + bytes((1, 2)) + bytes(6) + hashlib.sha256(canonical).digest()
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == metadata.st_gid == 65534
                and stat.S_IMODE(metadata.st_mode) == 0o600 and metadata.st_nlink == 1
                and metadata.st_size == len(expected) and not os.listxattr(descriptor)
                and os.pread(descriptor, 49, 0) == expected,
                "clean close permit binding or policy differs")
        named = permit.lstat()
        require((named.st_dev, named.st_ino) == (metadata.st_dev, metadata.st_ino),
                "clean close permit name changed")
        check_initial_opening_reader(args, credential, runtime, "clean-close")
        contender = draft.directory(draft.PACKAGES / credential, os.geteuid(), 0o700)
        try:
            try:
                fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                require(error.errno == errno.EWOULDBLOCK, "clean close reservation contention differs")
            else:
                require(False, "clean close keeper released its reservation before exit")
        finally:
            os.close(contender)
        require(os.pread(descriptor, 49, 0) == expected
                and (permit.lstat().st_dev, permit.lstat().st_ino) == (metadata.st_dev, metadata.st_ino),
                "clean close observation changed the permit")
    finally:
        os.close(descriptor)


def observe_keeper_opening_release(args, credential, secret, runtime, report):
    mode = validate_keeper_opening_release_selection(args)
    case = {"normal": "permit-close", "missing": "permit-missing",
            "keeper-lost": "permit-keeper-lost"}[mode]
    previous = observe_active_endpoint_case(args, credential, secret, runtime, report, case)
    permit = draft.USER / "state/openings-v1" / credential
    if mode == "normal":
        def consumed():
            require(not permit.exists() and not permit.is_symlink(),
                    "reopening prompt preceded clean close permit consumption")

        proof = observe_endpoint_proof(args, credential, secret, runtime, report,
                                       attended=True, permitted=True, before_input=consumed)
        require(proof["generation"] != previous and proof["signature_verified"] is True,
                "reopened signature reused the previous generation")
    require(not permit.exists() and not permit.is_symlink(), "keeper release left an available permit")
    observe_opening_refusal(args, credential, runtime, "NotFound")
    require(not permit.exists() and not permit.is_symlink(), "missing permit refusal created state")
    return validate_keeper_opening_release(dict(
        mode=mode, predecessor_signed=True, consumed_before_input=True, terminal_restored=True,
        missing_permit_refused=True, clean_close_published=mode == "normal",
        successor_signature_verified=mode == "normal", distinct_generation_verified=mode == "normal",
        keeper_death_observed=mode == "keeper-lost", authority=False))


def receive(channel):
    payload, ancillary, flags, _ = channel.recvmsg(8193)
    require(payload and len(payload) <= 8192 and not ancillary and flags == 0, "native packet boundary differs")
    return payload


def unchanged_stage(packages, snapshot, evidence, preserved=None):
    name, stage, _, _ = snapshot
    prior = () if preserved is None else preserved.package_names
    require(set(os.listdir(packages)) == {*prior, name}
            and os.stat(name, dir_fd=packages, follow_symlinks=False).st_ino == os.fstat(stage).st_ino
            and draft.stage_evidence(stage) == evidence, "retained stage changed")
    if preserved is not None:
        preserved.validate()


def published_package(packages, snapshot, credential, evidence, secret, preserved=None):
    name, stage, before, held = snapshot
    prior = () if preserved is None else preserved.package_names
    require(credential not in prior and set(os.listdir(packages)) == {*prior, credential}
            and name not in os.listdir(packages), "published package membership differs")
    linked = os.stat(credential, dir_fd=packages, follow_symlinks=False)
    current = os.fstat(stage)
    require(draft.metadata(linked) == draft.metadata(current)
            and (current.st_dev, current.st_ino, current.st_mode, current.st_uid, current.st_gid, current.st_nlink)
            == before[:6], "published package retained directory differs")
    observed = draft.stage_evidence(stage)
    require(observed[1] == evidence[1] and set(observed[1]) == set(held), "published package members changed")
    for member, (fd, _, _) in held.items():
        expected_metadata, expected_bytes = evidence[1][member]
        require(draft.metadata(os.fstat(fd)) == expected_metadata and os.pread(fd, 65537, 0) == expected_bytes
                and secret not in expected_bytes, "published package retained bytes differ")
    if preserved is not None:
        preserved.validate()


def creation_record_snapshot(admissions, name, record_text, secret):
    slot = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=admissions)
    member = None
    try:
        before = os.fstat(slot)
        require(before.st_uid == before.st_gid == 65534 and stat.S_IMODE(before.st_mode) == 0o700
                and before.st_nlink == 2 and before.st_dev == os.fstat(admissions).st_dev
                and not os.listxattr(slot) and os.listdir(slot) == ["record.json"], "creation record directory differs")
        member = os.open("record.json", os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=slot)
        info = os.fstat(member)
        expected = record_text.encode("ascii")
        require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534
                and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1 and info.st_dev == before.st_dev
                and info.st_size == len(expected) and not os.listxattr(member)
                and os.pread(member, 2049, 0) == expected and secret not in expected,
                "creation record retained bytes differ")
        require(draft.parse_record(expected.decode("ascii")) == draft.parse_record(record_text), "creation record codec differs")
        require(draft.metadata(os.stat("record.json", dir_fd=slot, follow_symlinks=False)) == draft.metadata(info)
                == draft.metadata(os.fstat(member)) and os.pread(member, 2049, 0) == expected
                and draft.metadata(os.stat(name, dir_fd=admissions, follow_symlinks=False)) == draft.metadata(before)
                == draft.metadata(os.fstat(slot)), "creation record snapshot changed")
    finally:
        if member is not None:
            os.close(member)
        os.close(slot)


class RetainedPublicationMutation:
    def __init__(self, parent, name, member):
        self.parent = os.dup(parent)
        self.name, self.member = name, member
        self.directory = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
        self.descriptor = os.open(member, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.directory)
        self.directory_metadata = draft.metadata(os.fstat(self.directory))
        self.member_metadata = draft.metadata(os.fstat(self.descriptor))
        self.contents = os.pread(self.descriptor, 65537, 0)
        self.validate()

    def validate(self):
        require(draft.metadata(os.stat(self.name, dir_fd=self.parent, follow_symlinks=False))
                == draft.metadata(os.fstat(self.directory)) == self.directory_metadata
                and draft.metadata(os.stat(self.member, dir_fd=self.directory, follow_symlinks=False))
                == draft.metadata(os.fstat(self.descriptor)) == self.member_metadata
                and os.pread(self.descriptor, 65537, 0) == self.contents,
                "retained publication mutation changed")

    def close(self):
        for descriptor in (self.descriptor, self.directory, self.parent):
            os.close(descriptor)


def mutate_retained_record(admissions, credential):
    directory = os.open(credential, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=admissions)
    descriptor = None
    try:
        descriptor = os.open("record.json", os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
        before = os.fstat(descriptor)
        original = os.pread(descriptor, 2049, 0)
        require(original.startswith(b"{") and len(original) <= 2048, "retained record mutation input differs")
        changed = b"!" + original[1:]
        require(os.pwrite(descriptor, changed, 0) == len(changed), "retained record mutation write differs")
        os.fsync(descriptor)
        after = os.fstat(descriptor)
        require((before.st_dev, before.st_ino, before.st_size) == (after.st_dev, after.st_ino, after.st_size)
                and os.pread(descriptor, 2049, 0) == changed, "retained record mutation was not in place")
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(directory)


def publication_protocol(args, report, process, channel, packages, admissions, snapshot, original_evidence,
                         secret, pidfds, identity, bound, preserved, before_publication, package_mutator=None,
                         retained_mutations=None, initial_opening=None, runtime=None):
    mutate_package = draft.mutate if package_mutator is None else package_mutator
    closed = parse_packet(receive(channel), args.case, process.pid, "closed-before-publication", identity)
    require(all(closed[field] == bound[field] for field in bound if field != "stage"), "closed binding changed")
    require(all(life.pidfd_ready(fd, 5) for fd in pidfds.values()), "publication preceded independent native exits")
    unchanged_stage(packages, snapshot, original_evidence, preserved)
    require(set(os.listdir(admissions)) == set(preserved.admission_names), "record preceded package publication")
    report["independent_closed_before_publication"] = True
    changed_evidence = None
    if args.case == "publish-invalidate-before-package":
        mutate_package(snapshot, "mutate-key")
        changed_evidence = draft.stage_evidence(snapshot[1])
    if before_publication is not None:
        before_publication(snapshot, report, preserved)
        unchanged_stage(packages, snapshot, original_evidence, preserved)
        require(set(os.listdir(admissions)) == set(preserved.admission_names), "unowned admission appeared at the publication barrier")
    preserved.validate()
    channel.sendall(b"C")
    durable = args.case not in ("publish-package-failure", "publish-invalidate-before-package", CONTENDED_CASE)
    credential = draft.parse_record(identity["record_canonical"])["credential_id"]
    if durable:
        package = parse_packet(receive(channel), args.case, process.pid, "package-durable", identity)
        require(all(package[field] == closed[field] for field in closed if field != "stage"), "durable package checkpoint changed")
        published_package(packages, snapshot, credential, original_evidence, secret, preserved)
        require(set(os.listdir(admissions)) == set(preserved.admission_names), "record appeared before durable package acknowledgment")
        if args.case == "publish-invalidate-package":
            mutate_package(snapshot, "mutate-key")
            changed_evidence = draft.stage_evidence(snapshot[1])
        preserved.validate()
        channel.sendall(b"D")
    if initial_opening is not None:
        opening = parse_packet(receive(channel), args.case, process.pid, "opening-permit-durable", identity)
        require(all(opening[field] == closed[field] for field in closed if field != "stage"),
                "opening permit publication changed verified closure")
        initial_opening_checkpoint(args, initial_opening, credential,
            identity["record_canonical"].encode("ascii"), packages, admissions, pidfds, runtime)
        channel.sendall(b"O")
    if args.case in RETAINED_CASES:
        retained = parse_packet(receive(channel), args.case, process.pid, "publication-retained", identity)
        require(all(retained[field] == closed[field] for field in closed if field != "stage"),
                "retained publication checkpoint changed")
        published_package(packages, snapshot, credential, original_evidence, secret, preserved)
        require(credential not in preserved.admission_names
                and set(os.listdir(admissions)) == {*preserved.admission_names, credential},
                "retained publication record membership differs")
        creation_record_snapshot(admissions, credential, identity["record_canonical"], secret)
        if args.case == "publish-invalidate-retained-record":
            mutate_retained_record(admissions, credential)
            mutation = RetainedPublicationMutation(admissions, credential, "record.json")
        else:
            draft.mutate(snapshot, "mutate-key")
            changed_evidence = draft.stage_evidence(snapshot[1])
            mutation = RetainedPublicationMutation(packages, credential, "key.openssh")
        require(retained_mutations is not None, "retained publication evidence owner missing")
        retained_mutations.append(mutation)
        preserved.validate()
        channel.sendall(b"R")
    final_payload = receive(channel)
    final = parse_packet(final_payload, args.case, process.pid, "final", identity)
    require(all(final[field] == closed[field] for field in closed if field not in ("stage", "preparation_dropped")),
            "publication changed verified closure")
    _, package_published, record_published, _ = PUBLICATION_OUTCOMES[args.case]
    if package_published:
        published_package(packages, snapshot, credential, changed_evidence or original_evidence, secret, preserved)
    else:
        unchanged_stage(packages, snapshot, changed_evidence or original_evidence, preserved)
    names = os.listdir(admissions)
    pending = args.case == "publish-record-before-rename-failure"
    if record_published:
        require(credential not in preserved.admission_names
                and set(names) == {*preserved.admission_names, credential}, "published creation record membership differs")
        if args.case == "publish-invalidate-retained-record":
            retained_mutations[-1].validate()
        else:
            creation_record_snapshot(admissions, credential, identity["record_canonical"], secret)
    elif pending:
        additions = set(names) - set(preserved.admission_names)
        require(len(additions) == 1 and set(names) == {*preserved.admission_names, *additions},
                "unpublished creation record membership differs")
        pending_name = next(iter(additions))
        require(pending_name.startswith(".pending-")
                and draft.fixed.hexadecimal(pending_name[9:], 64) and credential not in names,
                "unpublished creation record staging differs")
        creation_record_snapshot(admissions, pending_name, identity["record_canonical"], secret)
    else:
        require(set(names) == set(preserved.admission_names), "unexpected creation record after publication failure")
    preserved.validate()
    report.update({field: final[field] for field in PUBLICATION_FIELDS})
    report.update(final=final, publication_filesystem_verified=True, package_durable_observed=durable,
                  creation_record_verified=record_published and args.case != "publish-invalidate-retained-record",
                  pending_creation_record_observed=pending,
                  final_package_mutation_preserved=args.case in ("publish-invalidate-package", "publish-invalidate-retained-package"),
                  retained_publication_mutation_preserved=args.case in RETAINED_CASES,
                  admissions_empty=not names, final_slot_absent=credential not in os.listdir(packages),
                  contended_stage_preserved=args.case == CONTENDED_CASE)
    return final_payload


def protocol(args, report, bindings, process, helper_pidfd, channel, packages, admissions, secret, runtime, provider, verifier,
             preserved, before_publication, package_mutator=None, retained_mutations=None, initial_opening=None):
    import termios
    helper_start = life.process_start(process.pid)
    master = loader.receive_master(channel)
    pidfds = {}
    connection = snapshot = None
    observed = False
    try:
        identity = parse_packet(receive(channel), args.case, process.pid, "loader-ready")
        record = draft.parse_record(identity["record_canonical"])
        require(record["installation_id"] == bindings[0][3] and record["installation_id"] != bindings[1][3]
                and record["device_id"] == report["device_id"] and record["credential_id"] == report["credential_id"],
                "native fixed draft origin differs")
        preserved.validate()
        snapshot = draft.snapshot_package(packages, record, secret, preserved_names=preserved.package_names)
        original_evidence = draft.stage_evidence(snapshot[1])
        metadata_observations = {}
        identity_observations = {}
        for name in ("agent", "loader"):
            pid = identity[name + "_pid"]
            pidfds[name] = os.pidfd_open(pid)
            identity_observations[name] = life.inspect_agent(pid, life.process_start(pid), str(provider.files[
                "ssh-agent" if name == "agent" else "ssh-add"].path), process.pid)
            metadata_observations[name] = process_metadata(pid, secret)
        report["native_process_metadata"] = metadata_observations
        report["native_environment_absence_verified"] = validate_process_metadata(metadata_observations)
        validate_process_identity(identity_observations, process.pid)
        report["native_process_identity"] = identity_observations
        terminal = Path("/proc", str(identity["loader_pid"]), "fd/0").stat()
        require(stat.S_ISCHR(terminal.st_mode) and terminal.st_uid == os.geteuid()
                and stat.S_IMODE(terminal.st_mode) == 0o600
                and not termios.tcgetattr(master)[3] & (termios.ECHO | termios.ECHONL), "native private PTY differs")
        report.update(helper_pid=process.pid, record_canonical=identity["record_canonical"],
                      selected_installation_id=record["installation_id"], original_package_verified=True,
                      private_loader_terminal_observed=True)
        preserved.validate()
        channel.sendall(b"H")
        parse_packet(receive(channel), args.case, process.pid, "loader-handoff", identity)
        status = Path("/proc", str(identity["loader_pid"]), "status").read_text(encoding="ascii")
        require(any(line.startswith("State:") and line.split()[1] == "T" for line in status.splitlines()),
                "native loader stopped state differs")
        preserved.validate()
        channel.sendall(b"L")
        stage = "loaded" if args.case == "abandon-after-load" else (
            "before-proof" if args.case == "invalidate-before-proof" else "bound")
        value = parse_packet(receive(channel), args.case, process.pid, stage, identity)
        parsed = native.parse_key_bytes(snapshot[3]["key.openssh"][1])
        comment = ("vivac.device-key.v1:" + record["device_id"] + ":" + record["credential_id"]).encode("ascii")
        require(native.identities(runtime / "agent.sock") == [(parsed["public_blob"], comment)],
                "native original identity differs")
        require(not life.pidfd_ready(pidfds["agent"], 0) and life.pidfd_ready(pidfds["loader"], 0),
                "native loading exit observation differs")
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(3)
        connection.connect(str(runtime / "agent.sock"))
        report["native_identity_observed"] = True
        changed_evidence = None
        if args.case == "invalidate-before-proof":
            draft.mutate(snapshot, "mutate-key")
            changed_evidence = draft.stage_evidence(snapshot[1])
            preserved.validate()
            channel.sendall(b"P")
            value = parse_packet(receive(channel), args.case, process.pid, "binding-refused", identity)
        elif args.case == "complete" or is_publication(args.case):
            binding.openssl_verify(verifier, runtime, life.private_env(runtime), parsed["public_key"],
                                   bytes.fromhex(value["challenge"]), bytes.fromhex(value["signature"]), "rooted")
        final_payload = None
        preserved.validate()
        if args.case == "abandon-after-load":
            report["native_agent_alive_before_abandonment"] = True
            channel.sendall(b"A")
            parse_packet(receive(channel), args.case, process.pid, "abandoned", identity)
            require(process.poll() is None and life.process_start(process.pid) == helper_start
                    and not life.pidfd_ready(helper_pidfd, 0), "abandoned producer was not alive")
            unchanged_stage(packages, snapshot, original_evidence, preserved)
            report.update(abandoned_stage_retained_without_closure=True,
                          abandoned_stage_retained_while_producer_alive=True)
            require(life.pidfd_ready(pidfds["agent"], 5), "abandoned native exit remained unobserved")
            channel.sendall(b"X")
        else:
            channel.sendall(b"V")
            if args.case in DEATH_CASES:
                final_payload = death.publication_protocol(args, report, process, helper_pidfd, helper_start, channel,
                    packages, admissions, snapshot, original_evidence, secret, pidfds, identity, value, preserved)
            elif args.case in PUBLICATION_OUTCOMES:
                final_payload = publication_protocol(args, report, process, channel, packages, admissions, snapshot,
                                                     original_evidence, secret, pidfds, identity, value, preserved, before_publication,
                                                     package_mutator, retained_mutations, initial_opening, runtime)
            else:
                final_payload = receive(channel)
                final = parse_packet(final_payload, args.case, process.pid, "final", identity)
                require(all(final[field] == value[field] for field in value if field != "stage"),
                        "native result changed during closure")
                report["final"] = final
        for fd in pidfds.values():
            require(life.pidfd_ready(fd, 5), "native exit remains unobserved")
        if args.case == "abandon-after-load":
            require(life.pidfd_ready(helper_pidfd, 5), "abandoned helper exit remained unobserved")
            status = life.reap_owned(identity["agent_pid"])
            require(status is not None and os.WIFSIGNALED(status) and os.WTERMSIG(status) == LINUX_SIGKILL,
                    "abandoned agent exact exit differs")
            unchanged_stage(packages, snapshot, original_evidence, preserved)
        elif args.case == "invalidate-before-proof":
            unchanged_stage(packages, snapshot, changed_evidence, preserved)
        elif not is_publication(args.case):
            require(set(os.listdir(packages)) == set(preserved.package_names)
                    and os.listdir(snapshot[1]) == [] and os.fstat(snapshot[1]).st_nlink == 0,
                    "closed draft stage remains linked")
            for fd, raw, _ in snapshot[3].values():
                require(os.fstat(fd).st_nlink == 0 and os.pread(fd, 65537, 0) == raw, "closed original member changed")
        require(os.isatty(master) and spawn.connection_refused(connection, runtime / "agent.sock"),
                "native capability usable after exit")
        if not is_publication(args.case):
            require(set(os.listdir(admissions)) == set(preserved.admission_names)
                    and record["credential_id"] not in os.listdir(packages),
                    "native probe published package or admission")
            report.update(admissions_empty=not preserved.admission_names, final_slot_absent=True)
        preserved.validate()
        provider.revalidate()
        verifier.revalidate()
        for held in bindings:
            draft.revalidate_binding(held)
        report.update(independent_exits_observed=True, retained_connection_refused=True,
                      new_connection_refused=True, provider_revalidated=True,
                      native_binding_verified=args.case == "complete" or is_publication(args.case),
                      independent_signature_verified=args.case == "complete" or is_publication(args.case),
                      altered_message_refused=args.case == "complete" or is_publication(args.case),
                      closure_acknowledged=args.case != "abandon-after-load",
                      mutation_observed=args.case in ("invalidate-before-proof", "publish-invalidate-before-package", "publish-invalidate-package", *RETAINED_CASES),
                      invalidated_stage_preserved=args.case in ("invalidate-before-proof", "publish-invalidate-before-package"))
        if args.case in DEATH_CASES:
            report.pop("preparation_dropped", None)
            report["preparation_release_acknowledged"] = False
        else:
            report["preparation_dropped"] = args.case != "abandon-after-load"
        for field in ("abandoned_stage_retained_without_closure", "native_agent_alive_before_abandonment",
                      "abandoned_stage_retained_while_producer_alive"):
            report.setdefault(field, False)
        observed = True
        return final_payload
    finally:
        if not observed:
            UNCERTAIN.append((master, pidfds, connection, snapshot))
        else:
            os.close(master)
            for fd in pidfds.values():
                os.close(fd)
            if connection is not None:
                connection.close()
            if snapshot is not None:
                os.close(snapshot[1])
                for fd, _, _ in snapshot[3].values():
                    os.close(fd)


def observe(args, report, bindings, provider, verifier, runtime, *, preserved=None, identifiers=None,
            before_publication=None, package_mutator=None):
    require(package_mutator is None or (callable(package_mutator)
            and args.case in ("publish-invalidate-before-package", "publish-invalidate-package")),
            "native package mutation callback differs")
    import pty
    import termios
    master, slave = pty.openpty()
    os.fchmod(slave, 0o600)
    initial = copy.deepcopy(termios.tcgetattr(slave))
    channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    env = life.private_env(runtime)
    device, credential = (native.identifier(), native.identifier()) if identifiers is None else identifiers
    require(draft.fixed.installation(device) and draft.fixed.installation(credential), "producer identifiers differ")
    report.update(device_id=device, credential_id=credential)
    command = [str(args.writer), "--fixed-root-native-bind", args.case, device, credential,
               "--observer-fd", str(peer.fileno()), "--runtime-dir", str(runtime)]
    initial_opening = None
    if validate_initial_opening_selection(args):
        collection = draft.USER / "state/openings-v1"
        require(not collection.is_symlink(), "initial opening fixture collection is a symlink")
        if not collection.exists():
            collection.mkdir(mode=0o700)
        descriptor = draft.directory(collection, 65534, 0o700)
        require(not os.listdir(descriptor), "initial opening fixture collection is occupied")
        metadata = os.fstat(descriptor)
        initial_opening = {"directory": descriptor, "collection_identity": (metadata.st_dev, metadata.st_ino)}
        command.append("--initial-opening-permit")
    packages = draft.directory(draft.PACKAGES, 65534, 0o700)
    admissions = draft.directory(draft.ADMISSIONS, 65534, 0o700)
    owns_preserved = preserved is None
    preserved = namespaces.PreservedNamespace() if preserved is None else preserved
    preserved.validate()
    require(set(os.listdir(packages)) == set(preserved.package_names)
            and set(os.listdir(admissions)) == set(preserved.admission_names), "initial native store membership differs")
    process = pidfd = secret = None
    transcript, stdout, stderr = bytearray(), bytearray(), bytearray()
    completed = False
    retained_mutations = []
    measurement = None
    if validate_operational_measurement_selection(args):
        report["operational_measurement"] = {"clock": "monotonic-ns", "creation": {}, "ssh": {}}
        measurement = report["operational_measurement"]["creation"]
    try:
        provider.revalidate()
        spawn_started = time.monotonic_ns() if measurement is not None else None
        process = subprocess.Popen(command, stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            preexec_fn=lambda: generated.child_setup(slave),
            pass_fds=(peer.fileno(),),
            env=env, cwd="/", close_fds=True)
        peer.close()
        pidfd = os.pidfd_open(process.pid)
        start = life.process_start(process.pid)
        deadline = time.monotonic() + 90
        ack = confirmed = protocol_done = False
        final_payload = None
        with selectors.DefaultSelector() as selector:
            for stream, label in ((master, "terminal"), (process.stdout, "stdout"), (process.stderr, "stderr"), (channel, "packet")):
                os.set_blocking(stream if type(stream) is int else stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, label)
            while True:
                require(time.monotonic() < deadline, "native observation deadline expired")
                events = selector.select(0.02)
                for key, _ in events:
                    if key.data == "packet":
                        require(confirmed and not protocol_done and not stdout and not stderr
                                and life.process_start(process.pid) == start and not life.pidfd_ready(pidfd, 0),
                                "native protocol preceded confirmation or changed identity")
                        selector.unregister(channel)
                        channel.settimeout(20)
                        final_payload = protocol(args, report, bindings, process, pidfd, channel, packages, admissions,
                                                 secret, runtime, provider, verifier, preserved, before_publication,
                                                 package_mutator, retained_mutations, initial_opening)
                        protocol_done = True
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
                    require(len(target) <= limit, "native observer output exceeded its bound")
                    require(protocol_done or (not stdout and not stderr), "premature standard stream output")
                if not ack and generated.ACK_PROMPT in transcript:
                    ack_observed = time.monotonic_ns() if measurement is not None else None
                    if measurement is not None:
                        measurement["spawn_to_ack_prompt_ns"] = ack_observed - spawn_started
                    credentials = life.agent_credentials(process.pid)
                    require(credentials["Uid"] == credentials["Gid"] == [65534] * 4
                            and credentials["PPid"] == [os.getpid()] and credentials["NoNewPrivs"] == [1]
                            and life.executable(process.pid) == str(args.writer) and os.getsid(process.pid) == process.pid,
                            "attended helper identity differs")
                    require(not termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ECHONL)
                            and os.tcgetpgrp(master) == process.pid, "attended acknowledgment terminal differs")
                    generated.send(master, generated.ACK + b"\n", deadline)
                    ack = True
                    report["process_identity_verified"] = True
                if secret is None:
                    secret = generated.parse_disclosure(transcript)
                if secret is not None and not confirmed:
                    require(ack and not termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ECHONL)
                            and os.tcgetpgrp(master) == process.pid, "attended confirmation terminal differs")
                    generated.proc_secret_absent(process.pid, secret)
                    generated.send(master, secret + b"\n", deadline)
                    confirmation_sent = time.monotonic_ns() if measurement is not None else None
                    if measurement is not None:
                        measurement["ack_prompt_to_confirmation_ns"] = confirmation_sent - ack_observed
                    confirmed = True
                    report["private_terminal_observed"] = True
                if process.poll() is not None and not events:
                    break
        code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        require(protocol_done and life.pidfd_ready(pidfd, 0) and termios.tcgetattr(slave) == initial,
                "attended helper exit or terminal restoration unobserved")
        if args.case in DEATH_CASES:
            death.validate_death_receipt(code, report.get("producer_killed_by_pidfd"),
                life.pidfd_ready(pidfd, 0), report.get("normal_final_received"))
            require(joint.output(process, deadline) == b"", "killed producer emitted output before standard stream EOF")
            require(not stdout and not stderr and final_payload is None, "killed producer emitted a normal reply")
            report["standard_stream_eof_observed"] = True
        elif args.case == "abandon-after-load":
            require(code == 1 and not stdout and stderr == generated.REFUSAL_MESSAGE and final_payload is None,
                    "abandoned helper claimed closure")
        else:
            require(code == 0 and not stderr and stdout == final_payload + b"\n", "native helper final stream differs")
        require(secret is not None and secret not in stdout and secret not in stderr and transcript.count(secret) == 1,
                "native secret output boundary differs")
        for mutation in retained_mutations:
            mutation.validate()
        report.update(process_exit_observed=True, terminal_restored=True, standard_streams_clean=True,
                      secret_boundaries_checked=True)
        publication_verified = time.monotonic_ns() if measurement is not None else None
        if measurement is not None:
            measurement["confirmation_to_verified_publication_ns"] = publication_verified - confirmation_sent
        if initial_opening is not None:
            report["initial_opening_permit"] = finish_initial_opening(args, initial_opening, credential, runtime)
        if validate_consumed_opening_selection(args) is not None:
            joint.observe(args, report, runtime)
            report["opening_permit_consumption"] = observe_consumed_opening(
                args, credential, secret, runtime, report)
        if validate_keeper_opening_release_selection(args) is not None:
            joint.observe(args, report, runtime)
            report["keeper_opening_release"] = observe_keeper_opening_release(
                args, credential, secret, runtime, report)
        ssh_mode = validate_keeper_ssh_release_selection(args)
        if ssh_mode is not None:
            import keeper_ssh_probe

            joint.observe(args, report, runtime)
            if measurement is not None:
                measurement["publication_to_independent_admission_ns"] = time.monotonic_ns() - publication_verified
            report["keeper_ssh_release"] = validate_keeper_ssh_release(
                keeper_ssh_probe.observe(args, credential, secret, runtime, report, ssh_mode))
        preserved.validate()
        report.update(preserved_namespace=preserved.summarize(), namespace_preserved_verified=True)
        endpoint = validate_endpoint_selection(args)
        if endpoint is not None:
            if endpoint == "active":
                report["active_endpoint_proof_requested"] = True
                report["active_endpoint_proof"] = observe_active_endpoint_proof(
                    args, credential, secret, runtime, report)
            else:
                attended = endpoint == "attended"
                key = "attended_endpoint_proof" if attended else "endpoint_proof"
                report[key + "_requested"] = True
                if attended:
                    report[ENDPOINT_WRONG_SECRET_REFUSED] = observe_endpoint_proof(
                        args, credential, secret, runtime, report, attended=True, reject_wrong_secret=True)
                report[key] = observe_endpoint_proof(args, credential, secret, runtime, report, attended=attended)
        completed = True
    finally:
        if initial_opening is not None:
            for descriptor in (initial_opening.get("permit"), initial_opening["directory"]):
                if descriptor is not None:
                    os.close(descriptor)
        for mutation in retained_mutations:
            mutation.close()
        generated.wipe(secret)
        generated.wipe(transcript)
        if not completed:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            UNCERTAIN.append((process, pidfd, master, slave, channel, peer, packages, admissions, preserved))
        else:
            for fd in (pidfd, master, slave, packages, admissions):
                os.close(fd)
            for stream in (channel, peer, process.stdout, process.stderr):
                stream.close()
            if owns_preserved:
                preserved.close()


def source_paths(args):
    paths = {"helper": args.writer, "observer": Path(__file__), "draft_source": Path(draft.__file__),
            "provider_source": Path(providers.__file__), "binding_source": Path(binding.__file__),
            "loader_source": Path(loader.__file__), "generated_source": Path(generated.__file__),
            "lifecycle_source": Path(life.__file__), "openssh_source": Path(native.__file__),
            "spawn_source": Path(spawn.__file__), "session_source": Path(session.__file__),
            "fixed_source": Path(draft.fixed.__file__), "heap_source": Path(draft.heap.__file__),
            "joint_reader_source": Path(joint.__file__), "reader_helper": args.reader,
            "namespace_source": Path(namespaces.__file__), "multi_source": Path(__file__).with_name("creation_multi_probe.py"),
            "death_source": Path(death.__file__), "controller_source": Path(__file__).with_name("run_creation_native_probe.py")}
    if getattr(args, "mount_check", None) is not None:
        paths.update(mounts_source=Path(__file__).with_name("creation_mount_probe.py"),
                     mount_common_source=Path(__file__).with_name("creation_mount_common.py"),
                     mount_helper_source=Path(__file__).with_name("creation_mount_helper.py"),
                     live_mount_controller_source=Path(__file__).with_name("run_creation_mount_probe.py"))
    if getattr(args, "keeper_ssh_release", None) is not None:
        paths.update(keeper_ssh_source=Path(__file__).with_name("keeper_ssh_probe.py"),
                     ssh_lifecycle_source=Path(__file__).with_name("ssh_lifecycle_probe.py"),
                     terminal_source=Path(__file__).with_name("terminal_probe.py"))
    return paths


def validate_mount_selection(args):
    check = getattr(args, "mount_check", None)
    require(check is None or check in MOUNT_CHECKS, "native mount selection differs")
    if check is None:
        require(args.reader_check not in joint.MOUNT_CHECKS, "reader mount requires an explicit mount selection")
    elif check.startswith("writer-"):
        require(args.case == "publish-invalidate-before-package" and args.reader_check == "stable",
                "writer mount case or reader selection differs")
    else:
        require(args.case == "publish-complete" and args.reader_check == "mount-" + check.removeprefix("reader-"),
                "reader mount case or reader selection differs")
    return check


def finish_actor(args, report, bindings, provider, verifier, *, reader_required=True):
    for held in bindings:
        draft.revalidate_binding(held)
    require(all(generated.file_sha256(path) == report.get(name + "_sha256")
                for name, path in source_paths(args).items()), "producer actor sources changed")
    provider.revalidate()
    verifier.revalidate()
    report.update(helper_unchanged=True, sources_unchanged=True, root_binding_unchanged=True, status="observed")
    return validate_observation(report, args.case, reader_required=reader_required)


def main():
    parser = argparse.ArgumentParser()
    for name in ("writer", "reader", "report", "expected-setup"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--reader-check", choices=joint.CHECKS, default="stable")
    parser.add_argument("--mount-check", choices=MOUNT_CHECKS)
    endpoint = parser.add_mutually_exclusive_group()
    endpoint.add_argument("--endpoint-proof", action="store_true")
    endpoint.add_argument("--endpoint-attended", action="store_true")
    endpoint.add_argument("--endpoint-active", action="store_true")
    parser.add_argument("--opening-permit-check", action="store_true")
    parser.add_argument("--publish-initial-opening-permit", action="store_true")
    parser.add_argument("--consume-initial-opening-permit", choices=("normal", "sync-error"))
    parser.add_argument("--keeper-opening-release", choices=("normal", "missing", "keeper-lost"))
    parser.add_argument("--keeper-ssh-release", choices=("normal", "disconnect", "unconfirmed"))
    parser.add_argument("--measure-operational-stages", action="store_true")
    parser.add_argument("--case", choices=CASES, required=True)
    args = parser.parse_args()
    report = dict(version=1, experiment="rooted-native-creation", case=args.case, status="incomplete",
                  **dict.fromkeys(FLAGS, False), process_exit_observed=False)
    bindings = []
    provider = verifier = runtime = None
    try:
        if validate_mount_selection(args) is not None:
            report["mount_check"] = args.mount_check
        require(all(path.is_absolute() for path in (args.writer, args.reader, args.report, args.expected_setup)), "absolute paths required")
        require(args.reader_check == "stable" or args.case == "publish-complete", "reader mutation requires a complete publication")
        validate_endpoint_selection(args)
        validate_opening_permit_selection(args)
        validate_initial_opening_selection(args)
        validate_consumed_opening_selection(args)
        validate_keeper_opening_release_selection(args)
        validate_keeper_ssh_release_selection(args)
        validate_operational_measurement_selection(args)
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1"
                and os.getuid() == os.geteuid() == os.getgid() == os.getegid() == 65534, "disposable UID required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        report["resource_limits"] = life.require_resources(expected_script="creation_native_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        setup = draft.decode(args.expected_setup.read_bytes(), 32768)
        require(type(setup) is dict and type(setup.get("version")) is int and setup["version"] == 1
                and setup.get("status") == "observed" and type(setup.get("result")) is dict
                and setup["result"].get("status") == "observed", "native setup observation differs")
        expected = setup["result"]
        for root, prefix in ((draft.REAL, "real"), (draft.DECOY, "decoy")):
            held = draft.retain_binding(root)
            bindings.append(held)
            require(held[3] == expected[prefix + "_installation_id"]
                    and held[4]["sha256"] == expected[prefix + "_binding_sha256"], "native binding differs from setup")
            report[prefix + "_binding_sha256"] = held[4]["sha256"]
        runtime = Path(tempfile.mkdtemp(prefix="rooted-native-", dir="/tmp"))
        runtime.chmod(0o700)
        provider = providers.Provider()
        provider.open()
        version, architecture = provider.package(life.private_env(runtime))
        report["provenance"] = dict(package_version=version, package_arch=architecture, components=provider.components())
        verifier = providers.RetainedFile("/usr/bin/openssl", executable=True)
        report["openssl_sha256"] = verifier.sha256
        sources = source_paths(args)
        hashes = {name + "_sha256": generated.file_sha256(path) for name, path in sources.items()}
        report.update(hashes)
        if args.mount_check is not None:
            import creation_mount_probe as mounts
            mounts.run(args, report, bindings, provider, verifier, runtime)
        elif args.case in MULTI_CASES:
            import creation_multi_probe as multi
            multi.observe(args, report, bindings, provider, verifier, runtime)
        else:
            observe(args, report, bindings, provider, verifier, runtime)
            if (args.consume_initial_opening_permit is None and args.keeper_opening_release is None
                    and args.keeper_ssh_release is None):
                joint.observe(args, report, runtime)
            if args.opening_permit_check:
                report["opening_permit_check"] = observe_opening_permit_check(args, runtime, report)
        for held in bindings:
            draft.revalidate_binding(held)
        require(all(generated.file_sha256(path) == hashes[name + "_sha256"] for name, path in sources.items()),
                "native observer sources changed")
        provider.revalidate()
        verifier.revalidate()
        report.update(helper_unchanged=True, sources_unchanged=True, root_binding_unchanged=True, status="observed")
        validate_observation(report, args.case)
        provider.close()
        verifier.close()
        for held in bindings:
            os.close(held[0])
            os.close(held[1])
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        report.update(status="incomplete", failure={"kind": type(error).__name__, "frames": failure_frames(error)})
        UNCERTAIN.append((bindings, provider, verifier, runtime))
        try:
            life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        except OSError:
            pass
        print("Rooted native creation experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
