#!/usr/bin/env python3
"""Run one bounded disposable experiment, retaining incomplete observations."""

import argparse
import io
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import time
import uuid


LIMIT = 512 * 1024 * 1024
DEADLINE = 300
CASES = {"normal-return", "term", "kill", "direct-agent-kill",
         "pre-arm-parent-death", "post-arm-pre-exec-parent-death",
         "native-unlinked-parent-kill"}
SESSION_CASES = {"ready-sign-idle-lock", "changed-binding-before-ready",
                 "changed-lease-before-ready", "changed-lease-before-sign",
                 "changed-lease-before-publish", "lock-during-startup",
                 "agent-death-before-sign"}
CONCURRENT_CASES = {"completed-before-lock", "lock-before-native-send",
                    "lock-native-response-pending", "lock-after-native-reply",
                    "lock-before-publication", "parallel-lock-callers",
                    "lease-change-before-publication"}
RUST_CONCURRENT_CASES = CONCURRENT_CASES | {"session-abandon"}
RUST_ABANDON_CASES = {"admission-limit", "pidfd-failure", "owner-abandon"}
RUST_ISOLATION_CASES = {"process-boundary"}
RUST_RESERVATION_CASES = {"contention", "normal-release", "abandonment-retains", "death-is-not-exit",
                          "replacement-binding", "invalidation-retains"}
RUST_CONNECTED_CASES = {"roundtrip", "wrong-generation", "wrong-store", "wrong-role", "forwarded-sender",
                       "peer-exit", "historical-listener"}
RUST_ADMISSION_STORAGE_CASES = {"roundtrip", "existing-destination", "concurrent-publishers", "snapshot-in-place",
    "snapshot-replacement", "symlink-refused", "hardlink-refused", "pre-rename-error",
    "post-rename-sync-error", "crash-before-rename", "crash-after-rename", "acl-refused"}
RUST_ROOT_READER_CASES = {"valid", "wrong-root-owner", "writable-ancestor", "wrong-prefix-mode", "wrong-slot-owner",
    "binding-owner", "binding-mode", "binding-malformed", "binding-oversized", "binding-identity",
    "ancestor-symlink", "binding-symlink", "missing-binding", "state-mode", "state-acl",
    "snapshot-private-replacement", "unrelated-entry", "binding-hardlink", "binding-fifo"}
RUST_IPC_CASES = {"roundtrip", "wrong-generation", "wrong-store", "replay", "forwarded-sender",
                  "ancillary-rights", "peer-exit", "receive-timeout", "truncated-frame", "oversized-frame"}
LOADER_EXPECTED = {
    "load-ready-close": ("ready", "close", "ready_closed"),
    "cancel-at-prompt": ("prompt", "cancel", "cancelled"),
    "cancel-after-secret-handoff": ("secret-handoff", "cancel", "cancelled"),
    "cancel-before-readiness": ("before-readiness", "cancel", "cancelled"),
    "supervisor-kill-at-prompt": ("prompt", "supervisor-kill", "supervisor_killed"),
    "supervisor-kill-after-secret-handoff": ("secret-handoff", "supervisor-kill", "supervisor_killed"),
    "loader-death-at-prompt": ("prompt", "loader-kill", "loader_failed_closed"),
    "supervisor-kill-before-readiness": ("before-readiness", "supervisor-kill", "supervisor_killed"),
}
LOADER_CASES = set(LOADER_EXPECTED)
SSH_EXPECTED = {
    mode + "-" + suffix: (mode, seam, disconnect)
    for mode in ("pty", "pipe")
    for suffix, seam, disconnect in (("normal-close", "ready", False),
                                      ("disconnect-prompt", "prompt", True),
                                      ("disconnect-secret-handoff", "secret-handoff", True),
                                      ("disconnect-ready", "ready", True))
}
SSH_CASES = set(SSH_EXPECTED)
PROVIDER_CASES = {"system-package-identity", "retained-executable-objects", "path-poison-refused",
                  "unsafe-executable-refused", "native-keygen-decrypt", "native-empty-agent",
                  "native-load-and-sign", "wrong-passphrase-refused", "damaged-ciphertext-refused",
                  "headless-load-refused", "post-close-connections-refused"}
PROVIDER_COMPONENTS = {"ssh", "ssh-keygen", "ssh-agent", "ssh-add"}
PROVIDER_CHECKS = ("trusted_paths_checked", "retained_objects_checked", "package_database_matched",
                   "system_package_md5_matched", "path_poison_ignored", "unsafe_path_refused",
                   "native_identity_verified", "native_proof_verified", "keygen_public_matches",
                   "wrong_passphrase_refused", "damaged_ciphertext_refused", "headless_load_refused",
                   "retained_connection_refused", "new_connection_refused", "agent_exit_observed",
                   "snapshots_revalidated")
KDF_TARGETS = {"package-create": 2000, "activation-proof": 2000, "unlocked-sign-proof": 50,
               **{"reject-rounds-" + str(value): 50 for value in (0, 1, 16, 63, 65, 4294967295)},
               "reject-large-field": 50}
KDF_CASES = set(KDF_TARGETS)
HEAP_EXPECTED = {"valid-package": True, "rounds-u32max": False, "key-count-u32max": False,
                 "declared-body-1048575": False, "declared-body-1048576": False,
                 "declared-body-4294967295": False, "declared-salt-1048575": False,
                 "near-cap-large-field": False, "file-65536-malformed": False,
                 "file-65537-oversize": False}
HEAP_CASES = set(HEAP_EXPECTED)
GENERATED_NO_DISCLOSURE = {"wrong-ack", "eof-before-ack", "term-before-ack", "no-tty", "unset-disposable"}
GENERATED_CASES = GENERATED_NO_DISCLOSURE | {"valid-generated-package", "mismatch", "eof-confirmation",
                                             "term-confirmation", "terminal-hangup"}
GENERATED_FAILURE_CASES = {"valid-generated-package", "terminal-drift-acknowledgement",
                           "terminal-drift-confirmation", "interrupted-key-synced", "interrupted-manifest-synced",
                           "interrupted-before-publish", "interrupted-after-publish"}
GENERATED_ERROR_CASES = {"valid-generated-package", "returned-key-synced", "returned-manifest-synced",
                         "returned-before-publish", "returned-after-publish"}
RUST_LAUNCHER_CASES = {"stable-thread", "exec-error", "creator-thread-exit", "owner-before-arm",
                       "owner-after-arm", "native-agent", "native-agent-owner-death"}
RUST_LOADER_CASES = {"normal", "cancel-prompt", "cancel-handoff", "cancel-before-ready", "owner-kill-prompt",
                     "owner-kill-handoff", "owner-kill-before-ready", "loader-kill-prompt"}
RUST_SSH_LOADER_EXPECTED = {
    mode + "-" + suffix: (mode, seam, disconnected)
    for mode in ("pty", "pipe")
    for suffix, seam, disconnected in (("normal-close", "ready", False),
                                       ("disconnect-prompt", "prompt", True),
                                       ("disconnect-handoff", "handoff", True),
                                       ("disconnect-before-ready", "before-ready", True),
                                       ("disconnect-ready", "ready", True))
}
RUST_SSH_LOADER_CASES = set(RUST_SSH_LOADER_EXPECTED)
RUST_SSH_CONCURRENT_EXPECTED = {
    mode + "-" + suffix: (mode, seam, disconnected, sent, reply, verified, published)
    for mode in ("pty", "pipe")
    for suffix, seam, disconnected, sent, reply, verified, published in (
        ("normal-close", "published", False, 1, 1, 1, 1),
        ("disconnect-before-native-send", "before-native-send", True, 0, 0, 0, 0),
        ("disconnect-native-response-pending", "native-response-pending", True, 1, 0, 0, 0),
        ("disconnect-after-native-reply", "after-native-reply", True, 1, 1, 0, 0),
        ("disconnect-before-publication", "before-publication", True, 1, 1, 1, 0),
        ("disconnect-published", "published", True, 1, 1, 1, 1))
}
RUST_SSH_CONCURRENT_CASES = set(RUST_SSH_CONCURRENT_EXPECTED)
EXPERIMENTS = {"lifecycle": ("lifecycle_probe.py", CASES),
               "session": ("session_probe.py", SESSION_CASES),
               "concurrent": ("concurrent_probe.py", CONCURRENT_CASES),
               "loader": ("loader_probe.py", LOADER_CASES),
               "ssh": ("ssh_lifecycle_probe.py", SSH_CASES),
               "provider": ("provider_probe.py", PROVIDER_CASES),
               "kdf": ("kdf_probe.py", KDF_CASES),
               "heap": ("heap_probe.py", HEAP_CASES),
               "generated": ("generated_probe.py", GENERATED_CASES),
               "generated-deadline": ("generated_probe.py", set()),
               "generated-failures": ("generated_probe.py", GENERATED_FAILURE_CASES),
               "generated-publication-errors": ("generated_probe.py", GENERATED_ERROR_CASES),
               "rust-launcher": ("rust_launcher_probe.py", RUST_LAUNCHER_CASES),
               "rust-loader": ("rust_loader_probe.py", RUST_LOADER_CASES),
               "rust-ssh-loader": ("rust_ssh_loader_probe.py", RUST_SSH_LOADER_CASES),
               "rust-concurrent": ("rust_concurrent_probe.py", RUST_CONCURRENT_CASES),
               "rust-creation-binding": ("rust_creation_binding_probe.py", {"complete", "wrong-key", "wrong-comment"}),
               "rust-abandon": ("rust_abandon_probe.py", RUST_ABANDON_CASES),
               "rust-isolation": ("rust_isolation_probe.py", RUST_ISOLATION_CASES),
               "rust-reservation": ("rust_reservation_probe.py", RUST_RESERVATION_CASES),
               "rust-connected": ("rust_connected_probe.py", RUST_CONNECTED_CASES),
               "rust-admission-storage": ("rust_admission_storage_probe.py", RUST_ADMISSION_STORAGE_CASES),
               "rust-root-reader": ("rust_root_reader_probe.py", RUST_ROOT_READER_CASES),
               "rust-ipc": ("rust_ipc_probe.py", RUST_IPC_CASES),
               "rust-ssh-concurrent": ("rust_ssh_concurrent_probe.py", RUST_SSH_CONCURRENT_CASES)}
CONCURRENT_EXPECTED = {
    "completed-before-lock": ("before-native-send", "published", 1, 1, 1, 1),
    "lock-before-native-send": ("before-native-send", "cancelled_by_lock", 0, 0, 0, 0),
    "lock-native-response-pending": ("native-response-pending", "cancelled_by_lock", 1, None, 0, 0),
    "lock-after-native-reply": ("after-native-reply", "cancelled_by_lock", 1, 1, 0, 0),
    "lock-before-publication": ("before-publication", "cancelled_by_lock", 1, 1, 1, 0),
    "parallel-lock-callers": ("before-native-send", "cancelled_by_lock", 0, 0, 0, 0),
    "lease-change-before-publication": ("before-publication", "lease_invalidated", 1, 1, 1, 0),
}
EVENT_QUERY = r"""
$ErrorActionPreference = 'Stop'
$since = [datetime]::Parse($env:VIVAC_EVENT_SINCE).ToUniversalTime()
$queries = @(
 @{LogName='Microsoft-Windows-TerminalServices-LocalSessionManager/Operational';Id=36},
 @{LogName='Microsoft-Windows-RemoteDesktopServices-RdpCoreTS/Operational';Id=226},
 @{LogName='System';ProviderName='Microsoft-Windows-Winlogon';Id=4008},
 @{LogName='System';ProviderName='Service Control Manager';Id=7031}
)
$found = @()
foreach ($query in $queries) {
 $query.StartTime = $since
 try { $events = @(Get-WinEvent -FilterHashtable $query -MaxEvents 8 -ErrorAction Stop) }
 catch {
  if ($_.FullyQualifiedErrorId -like 'NoMatchingEventsFound*') { continue }
  throw
 }
 foreach ($event in $events) {
  $found += @{log=$event.LogName;id=$event.Id;at=$event.TimeCreated.ToUniversalTime().ToString('o')}
 }
}
ConvertTo-Json -InputObject @($found) -Compress
"""


class RunnerError(Exception):
    pass


def validate_rust_ssh_concurrent_observation(observation):
    if (type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != "rust-ssh-concurrent" or observation.get("status") != "observed"):
        raise ValueError("Rust SSH concurrent report identity differs")
    for field in ("custody_accredited", "launcher_accredited", "full_session_accredited"):
        if observation.get(field) is not False:
            raise ValueError("Rust SSH concurrent report claims untested admission")
    for field in ("ssh_disconnect_tested", "concurrent_lock_tested", "sentinel_survived", "resources_closed_after_exit"):
        if observation.get(field) is not True:
            raise ValueError("Rust SSH concurrent report is incomplete")
    cases = observation.get("cases")
    if (not isinstance(cases, list) or len(cases) != len(RUST_SSH_CONCURRENT_CASES)
            or any(not isinstance(case, dict) for case in cases)
            or {case.get("case") for case in cases} != RUST_SSH_CONCURRENT_CASES):
        raise ValueError("Rust SSH concurrent matrix differs")
    fields = {"case", "status", "selected_seam", "request_outcome", "state", "active",
              "native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published",
              "lock_callers", "lock_acknowledgments", "workers_joined", "close_acknowledged",
              "resources_closed_after_drain", "closing_observed_while_paused", "request_late_refused",
              "drain_wait_observed", "busy_refused", "closed_refused", "creator_tid", "agent_pid", "loader_pid",
              "agent_exit_code", "agent_exit_signal", "loader_exit_code", "loader_exit_signal", "pressure_iterations",
              "version", "custody_accredited", "full_session_accredited", "concurrent_lock_tested",
              "ssh_disconnect_observed", "ssh_disconnect_cause", "ssh_mode", "ssh_stdin_is_tty",
              "ssh_client_exit_code", "supervisor_exit_observed", "supervisor_exec_verified",
              "readiness_proof_verified", "published_proof_verified", "lease_invalidated_observed",
              "pty_master_retained_through_exit", "pty_slave_private", "sealed_snapshot",
              "retained_connection_refused", "new_connection_refused"}
    for case in cases:
        mode, seam, disconnected, sent, reply, verified, published = RUST_SSH_CONCURRENT_EXPECTED[case["case"]]
        cancelling = not bool(published)
        if (set(case) != fields or case["status"] != "observed" or case["selected_seam"] != seam
                or case["state"] != "CLOSED" or case["ssh_mode"] != mode
                or case["request_outcome"] != ("cancelled_by_ssh" if cancelling else "published")):
            raise ValueError("Rust SSH concurrent cause or seam differs")
        counters = {"native_requests_sent": sent, "native_replies_received": reply, "proofs_verified": verified,
                    "proofs_published": published, "lock_callers": 1, "lock_acknowledgments": 1,
                    "workers_joined": 1, "version": 1, "agent_exit_signal": 9, "loader_exit_code": 0,
                    "ssh_client_exit_code": -9 if disconnected else 0}
        if any(type(case[field]) is not int or case[field] != value for field, value in counters.items()):
            raise ValueError("Rust SSH concurrent event or exit count differs")
        if case["agent_exit_code"] is not None or case["loader_exit_signal"] is not None:
            raise ValueError("Rust SSH concurrent native exit cause differs")
        for field in ("agent_pid", "loader_pid", "creator_tid", "pressure_iterations", "closed_refused", "busy_refused"):
            minimum = int(field != "busy_refused" or bool(published))
            if type(case[field]) is not int or case[field] < minimum:
                raise ValueError("Rust SSH concurrent identity or control count differs")
        expected = {"active": False, "close_acknowledged": True, "resources_closed_after_drain": True,
                    "closing_observed_while_paused": cancelling, "request_late_refused": cancelling,
                    "drain_wait_observed": cancelling, "custody_accredited": False,
                    "full_session_accredited": False, "concurrent_lock_tested": True,
                    "ssh_disconnect_observed": disconnected, "ssh_stdin_is_tty": mode == "pty",
                    "published_proof_verified": bool(published), "lease_invalidated_observed": False}
        if any(case[field] is not value for field, value in expected.items()):
            raise ValueError("Rust SSH concurrent overlap or publication differs")
        cause = case["ssh_disconnect_cause"]
        if (disconnected and cause not in ("eof", "eio", "sighup")) or (not disconnected and cause is not None):
            raise ValueError("Rust SSH concurrent transport cause was not observed")
        for field in ("supervisor_exit_observed", "supervisor_exec_verified", "readiness_proof_verified",
                      "pty_master_retained_through_exit", "pty_slave_private", "sealed_snapshot",
                      "retained_connection_refused", "new_connection_refused"):
            if case[field] is not True:
                raise ValueError("Rust SSH concurrent independent resource boundary differs")


def validate_rust_ipc_observation(observation):
    from rust_ipc_probe import validate_observation
    validate_observation(observation)


def validate_rust_isolation_observation(observation):
    from rust_isolation_probe import validate_observation
    validate_observation(observation)


def validate_rust_reservation_observation(observation):
    from rust_reservation_probe import validate_observation
    validate_observation(observation)


def validate_rust_admission_storage_observation(observation):
    from rust_admission_storage_probe import validate_observation
    validate_observation(observation)


def validate_rust_root_reader_observation(observation):
    from rust_root_reader_probe import validate_observation
    validate_observation(observation)


def validate_rust_connected_observation(observation):
    from rust_connected_probe import validate_observation
    validate_observation(observation)


def validate_rust_abandon_observation(observation):
    from rust_abandon_probe import validate_observation
    validate_observation(observation)


def validate_session_abandon_case(case):
    fields = {"case", "status", "version", "selected_seam", "request_outcome", "state", "active",
              "native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published",
              "workers_joined", "lock_callers", "lock_acknowledgments", "close_acknowledged",
              "resources_closed_after_drain", "admission_uncertain", "readmission_refused", "descriptors_retained",
              "retained_descriptor_count", "drop_elapsed_ns", "creator_tid", "agent_pid", "loader_pid",
              "loader_exit_code", "loader_exit_signal", "pressure_iterations", "custody_accredited",
              "full_session_accredited", "concurrent_lock_tested", "callback_uncertain",
              "readiness_proof_verified", "published_proof_verified", "retained_connection_refused",
              "new_connection_refused", "lease_invalidated_observed", "pty_master_retained_through_exit",
              "pty_slave_private", "sealed_snapshot", "helper_responsive_after_drop",
              "retained_descriptors_independently_checked", "agent_exit_signal", "agent_exit_code",
              "helper_exit_code", "resources_retained_until_helper_exit"}
    if (set(case) != fields or case["case"] != "session-abandon" or case["status"] != "observed"
            or case["selected_seam"] != "before-native-send" or case["request_outcome"] != "failed_after_abandon"
            or case["state"] != "CLOSING" or case["loader_exit_signal"] is not None
            or case["agent_exit_code"] is not None):
        raise ValueError("session abandonment schema or cause differs")
    counts = {"version": 1, "native_requests_sent": 0, "native_replies_received": 0, "proofs_verified": 0,
              "proofs_published": 0, "workers_joined": 1, "lock_callers": 1, "lock_acknowledgments": 0,
              "readmission_refused": 3, "retained_descriptor_count": 5, "loader_exit_code": 0,
              "agent_exit_signal": 9, "helper_exit_code": 0}
    if any(type(case[field]) is not int or case[field] != count for field, count in counts.items()):
        raise ValueError("session abandonment counters differ")
    for field in ("drop_elapsed_ns", "creator_tid", "agent_pid", "loader_pid", "pressure_iterations"):
        if type(case[field]) is not int or not 0 < case[field] <= 0xffffffffffffffff:
            raise ValueError("session abandonment observation is missing")
    false_fields = ("active", "close_acknowledged", "resources_closed_after_drain", "custody_accredited",
                    "full_session_accredited", "concurrent_lock_tested", "published_proof_verified", "lease_invalidated_observed")
    true_fields = ("admission_uncertain", "descriptors_retained", "callback_uncertain", "readiness_proof_verified",
                   "retained_connection_refused", "new_connection_refused", "pty_master_retained_through_exit",
                   "pty_slave_private", "sealed_snapshot", "helper_responsive_after_drop",
                   "retained_descriptors_independently_checked", "resources_retained_until_helper_exit")
    if any(case[field] is not False for field in false_fields) or any(case[field] is not True for field in true_fields):
        raise ValueError("session abandonment retention or non-acknowledgment differs")


def validate_rust_creation_binding_observation(observation):
    if (type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != "rust-creation-binding" or observation.get("status") != "observed"):
        raise ValueError("Rust creation-binding report identity differs")
    for field in ("custody_accredited", "creation_admission", "producer_origin_checked"):
        if observation.get(field) is not False:
            raise ValueError("Rust creation-binding report claims untested admission")
    for field in ("sentinel_survived", "resources_closed_after_exit"):
        if observation.get(field) is not True:
            raise ValueError("Rust creation-binding report is incomplete")
    cases = observation.get("cases")
    if (type(cases) is not list or len(cases) != 3
            or {case.get("case") for case in cases} != {"complete", "wrong-key", "wrong-comment"}):
        raise ValueError("Rust creation-binding case coverage differs")
    for case in cases:
        positive = case["case"] == "complete"
        if (case.get("status") != "observed" or case.get("stage") != "creation-final"
                or case.get("state") != "CLOSED" or case.get("active") is not False):
            raise ValueError("Rust creation-binding final state differs")
        for field in ("custody_accredited", "creation_admission", "producer_origin_checked", "ready_published"):
            if case.get(field) is not False:
                raise ValueError("Rust creation-binding case claims untested admission")
        for field, expected in (("native_binding_verified", positive), ("identity_mismatch", not positive),
                                ("independent_signature_verified", positive), ("altered_message_refused", positive)):
            if case.get(field) is not expected:
                raise ValueError("Rust creation-binding outcome differs")
        for field in ("close_acknowledged", "resources_closed_after_drain", "independent_exits_observed",
                      "original_identity_observed", "retained_connection_refused", "new_connection_refused",
                      "pty_master_retained_through_exit", "pty_slave_private", "sealed_snapshot", "lease_revalidated"):
            if case.get(field) is not True:
                raise ValueError("Rust creation-binding independent observation differs")
        counts = dict.fromkeys(("native_requests_sent", "native_replies_received", "proofs_verified",
                               "proofs_published", "workers_joined"), int(positive))
        counts.update(lock_callers=1, lock_acknowledgments=1, busy_refused=0, closed_refused=0,
                      agent_exit_signal=9, loader_exit_code=0)
        if any(type(case.get(field)) is not int or case[field] != expected for field, expected in counts.items()):
            raise ValueError("Rust creation-binding counters or exits differ")
        if case.get("agent_exit_code") is not None or case.get("loader_exit_signal") is not None:
            raise ValueError("Rust creation-binding exit cause differs")


def validate_rust_concurrent_observation(observation):
    if (type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != "rust-concurrent" or observation.get("status") != "observed"):
        raise ValueError("Rust concurrent report identity differs")
    for field in ("custody_accredited", "launcher_accredited", "full_session_accredited", "ssh_disconnect_tested"):
        if observation.get(field) is not False:
            raise ValueError("Rust concurrent report claims untested admission")
    for field in ("concurrent_lock_tested", "sentinel_survived", "resources_closed_after_exit"):
        if observation.get(field) is not True:
            raise ValueError("Rust concurrent report is incomplete")
    cases = observation.get("cases")
    if (not isinstance(cases, list) or len(cases) != len(RUST_CONCURRENT_CASES)
            or any(not isinstance(case, dict) for case in cases)
            or {case.get("case") for case in cases} != RUST_CONCURRENT_CASES):
        raise ValueError("Rust concurrent matrix differs")
    fields = {"case", "status", "selected_seam", "request_outcome", "state", "active",
              "native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published",
              "lock_callers", "lock_acknowledgments", "workers_joined", "close_acknowledged",
              "resources_closed_after_drain", "closing_observed_while_paused", "request_late_refused",
              "drain_wait_observed", "busy_refused", "closed_refused", "creator_tid", "agent_pid", "loader_pid",
              "agent_exit_code", "agent_exit_signal", "loader_exit_code", "loader_exit_signal",
              "pressure_iterations", "version", "custody_accredited", "full_session_accredited", "concurrent_lock_tested",
              "readiness_proof_verified", "published_proof_verified", "retained_connection_refused",
              "new_connection_refused", "lease_invalidated_observed", "pty_master_retained_through_exit",
              "pty_slave_private", "sealed_snapshot"}
    for case in cases:
        if case["case"] == "session-abandon":
            validate_session_abandon_case(case)
            continue
        seam, outcome, sent, replies, verified, published = CONCURRENT_EXPECTED[case["case"]]
        closing = outcome == "cancelled_by_lock"
        count = 2 if case["case"] == "parallel-lock-callers" else 1
        if (set(case) != fields or case["status"] != "observed" or case["selected_seam"] != seam
                or case["request_outcome"] != outcome or case["state"] != "CLOSED"):
            raise ValueError("Rust concurrent cause or seam differs")
        counters = {"native_requests_sent": sent, "native_replies_received": 0 if replies is None else replies,
                    "proofs_verified": verified, "proofs_published": published, "lock_callers": count,
                    "lock_acknowledgments": count, "workers_joined": 1, "version": 1,
                    "agent_exit_signal": 9, "loader_exit_code": 0}
        if any(type(case[field]) is not int or case[field] != value for field, value in counters.items()):
            raise ValueError("Rust concurrent event count differs")
        if case["agent_exit_code"] is not None or case["loader_exit_signal"] is not None:
            raise ValueError("Rust concurrent native exit cause differs")
        for field in ("pressure_iterations", "closed_refused", "agent_pid", "loader_pid", "creator_tid", "busy_refused"):
            minimum = int(field != "busy_refused" or outcome == "published")
            if type(case[field]) is not int or case[field] < minimum:
                raise ValueError("Rust concurrent control counter differs")
        expected = {"active": False, "close_acknowledged": True, "resources_closed_after_drain": True,
                    "closing_observed_while_paused": closing, "request_late_refused": closing,
                    "drain_wait_observed": closing, "custody_accredited": False, "full_session_accredited": False,
                    "concurrent_lock_tested": True, "published_proof_verified": outcome == "published",
                    "lease_invalidated_observed": outcome == "lease_invalidated"}
        if any(case[field] is not value for field, value in expected.items()):
            raise ValueError("Rust concurrent overlap or publication differs")
        for field in ("readiness_proof_verified", "retained_connection_refused", "new_connection_refused",
                      "pty_master_retained_through_exit", "pty_slave_private", "sealed_snapshot"):
            if case[field] is not True:
                raise ValueError("Rust concurrent independent resource boundary differs")


def validate_rust_ssh_loader_observation(observation):
    if (type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != "rust-ssh-loader" or observation.get("status") != "observed"):
        raise ValueError("Rust SSH loader report identity differs")
    for field in ("custody_accredited", "launcher_accredited", "full_session_accredited"):
        if observation.get(field) is not False:
            raise ValueError("Rust SSH loader claims untested admission")
    for field in ("ssh_disconnect_tested", "loader_lifecycle_tested", "sentinel_survived", "resources_closed_after_exit"):
        if observation.get(field) is not True:
            raise ValueError("Rust SSH loader report is incomplete")
    cases = observation.get("cases")
    if (not isinstance(cases, list) or len(cases) != len(RUST_SSH_LOADER_CASES)
            or any(not isinstance(case, dict) for case in cases)
            or {case.get("case") for case in cases} != RUST_SSH_LOADER_CASES):
        raise ValueError("Rust SSH loader case matrix differs")
    fields = {"case", "status", "selected_seam", "secret_writes", "ready_published", "independent_proof_verified",
              "agent_exit_observed", "loader_exit_observed", "pty_master_retained_through_exit", "loader_stopped_observed",
              "close_ack", "owner_sigkill_observed", "loader_failed_observed", "retained_connection_refused",
              "new_connection_refused", "lease_revalidated", "sealed_snapshot", "pty_slave_private",
              "ssh_mode", "ssh_stdin_is_tty", "ssh_disconnect_observed", "ssh_disconnect_cause",
              "ssh_client_exit_code", "supervisor_exit_observed", "supervisor_exec_verified"}
    for case in cases:
        mode, seam, disconnected = RUST_SSH_LOADER_EXPECTED[case["case"]]
        if (set(case) != fields or case["status"] != "observed"
                or case["selected_seam"] != seam or case["ssh_mode"] != mode):
            raise ValueError("Rust SSH loader selected seam differs")
        counters = {"secret_writes": int(seam != "prompt"), "ssh_client_exit_code": -9 if disconnected else 0}
        if any(type(case[field]) is not int or case[field] != value for field, value in counters.items()):
            raise ValueError("Rust SSH loader handoff or client exit differs")
        expected = {"ready_published": seam == "ready", "independent_proof_verified": seam in ("ready", "before-ready"),
                    "loader_stopped_observed": seam != "prompt", "close_ack": True,
                    "owner_sigkill_observed": False, "loader_failed_observed": False,
                    "ssh_stdin_is_tty": mode == "pty", "ssh_disconnect_observed": disconnected}
        if any(case[field] is not value for field, value in expected.items()):
            raise ValueError("Rust SSH loader outcome differs")
        cause = case["ssh_disconnect_cause"]
        if (disconnected and cause not in ("eof", "eio", "sighup")) or (not disconnected and cause is not None):
            raise ValueError("Rust SSH loader lacks an observed transport cause")
        for field in ("agent_exit_observed", "loader_exit_observed", "pty_master_retained_through_exit",
                      "retained_connection_refused", "new_connection_refused", "lease_revalidated", "sealed_snapshot",
                      "pty_slave_private", "supervisor_exit_observed", "supervisor_exec_verified"):
            if case[field] is not True:
                raise ValueError("Rust SSH loader resource boundary differs")


def validate_rust_loader_observation(observation):
    if (type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != "rust-loader"):
        raise ValueError("Rust loader report identity differs")
    for field in ("custody_accredited", "launcher_accredited", "full_session_accredited", "ssh_disconnect_tested"):
        if observation.get(field) is not False:
            raise ValueError("Rust loader claims untested admission")
    for field in ("loader_lifecycle_tested", "sentinel_survived", "resources_closed_after_exit"):
        if observation.get(field) is not True:
            raise ValueError("Rust loader report is incomplete")
    cases = observation.get("cases")
    if (not isinstance(cases, list) or len(cases) != len(RUST_LOADER_CASES)
            or {case.get("case") for case in cases} != RUST_LOADER_CASES):
        raise ValueError("Rust loader case matrix differs")
    fields = {"case", "status", "selected_seam", "secret_writes", "ready_published", "independent_proof_verified",
              "agent_exit_observed", "loader_exit_observed", "pty_master_retained_through_exit", "loader_stopped_observed",
              "close_ack", "owner_sigkill_observed", "loader_failed_observed", "retained_connection_refused",
              "new_connection_refused", "lease_revalidated", "sealed_snapshot", "pty_slave_private"}
    for case in cases:
        name = case["case"]
        seam = "ready" if name == "normal" else "before-ready" if name.endswith("before-ready") else "handoff" if name.endswith("handoff") else "prompt"
        delivered = seam != "prompt"
        owner_dead = name.startswith("owner-kill-")
        if set(case) != fields or case["status"] != "observed" or case["selected_seam"] != seam:
            raise ValueError("Rust loader selected seam differs")
        if type(case["secret_writes"]) is not int or case["secret_writes"] != int(delivered):
            raise ValueError("Rust loader secret handoff differs")
        expected = {"ready_published": name == "normal", "independent_proof_verified": seam in ("ready", "before-ready"),
                    "loader_stopped_observed": delivered, "close_ack": not owner_dead,
                    "owner_sigkill_observed": owner_dead, "loader_failed_observed": name == "loader-kill-prompt"}
        if any(case[field] is not value for field, value in expected.items()):
            raise ValueError("Rust loader outcome differs")
        for field in ("agent_exit_observed", "loader_exit_observed", "pty_master_retained_through_exit",
                      "retained_connection_refused", "new_connection_refused", "lease_revalidated", "sealed_snapshot", "pty_slave_private"):
            if case[field] is not True:
                raise ValueError("Rust loader resource boundary differs")


def validate_rust_launcher_observation(observation):
    if (type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != "rust-launcher"
            or observation.get("resources_closed_after_exit") is not True):
        raise ValueError("Rust launcher observation identity differs")
    cases = observation.get("cases")
    if (not isinstance(cases, list) or len(cases) != len(RUST_LAUNCHER_CASES)
            or {case.get("case") for case in cases} != RUST_LAUNCHER_CASES):
        raise ValueError("Rust launcher case matrix differs")
    for field in ("custody_accredited", "launcher_accredited", "full_loader_tested", "threadsafe_fullflow", "secret_channel_tested"):
        if observation.get(field) is not False:
            raise ValueError("Rust launcher claims untested admission")
    required = {
        "stable-thread": ("parent_poison_present", "nnp_after_exec", "signal_mask_empty", "fd_allowlist_verified",
                          "child_environment_clean", "canary_excluded", "shutdown_observed"),
        "exec-error": ("exec_failure_observed", "failed_exec_stdout_empty", "owner_reusable_after_error", "shutdown_observed"),
        "creator-thread-exit": ("creator_thread_exit_observed", "parent_process_alive", "child_sigkill_observed",
                                "held_control_through_exit", "deliberately_forgotten_owner"),
        "native-agent": ("empty_identities_observed", "native_credentials_checked", "native_sigkill_observed",
                         "shutdown_observed", "retained_connection_refused", "new_connection_refused"),
        "native-agent-owner-death": ("child_sigkill_observed", "owner_sigkill_observed", "native_credentials_checked",
                                     "retained_connection_refused", "new_connection_refused"),
    }
    for case in observation["cases"]:
        name = case["case"]
        if case.get("status") != "observed":
            raise ValueError("Rust launcher case was not observed")
        if name in required and any(case.get(field) is not True for field in required[name]):
            raise ValueError("Rust launcher case control differs")
        if name in ("owner-before-arm", "owner-after-arm"):
            sig, code = case.get("child_signal"), case.get("child_code")
            if ((sig is None) == (code is None) or
                    (sig is not None and (type(sig) is not int or not 0 < sig < 65)) or
                    (code is not None and (type(code) is not int or not 0 <= code <= 255))):
                raise ValueError("Rust launcher child exit observation differs")
            if (case.get("child_exit_observed") is not True or case.get("owner_sigkill_observed") is not True
                    or case.get("observer_endpoint_retained") is not True or case.get("native_exec_observed") is not False
                    or case.get("seam") != ("B" if name == "owner-before-arm" else "A")):
                raise ValueError("Rust launcher parent-death seam differs")
            if name == "owner-after-arm" and case.get("child_signal") != 9:
                raise ValueError("Rust launcher armed child did not receive SIGKILL")
            if name == "owner-before-arm" and case.get("child_signal") is None and not case.get("child_code"):
                raise ValueError("Rust launcher unarmed child did not refuse")
            if name == "owner-before-arm" and case.get("parent_change_refusal_observed") is not True:
                raise ValueError("Rust launcher parent-change refusal was not observed")
        if name == "exec-error" and (case.get("exec_failure_raw_os_error") != 8
                                     or case.get("exec_preexec_seams_observed") != ["B", "A"]):
            raise ValueError("Rust launcher exec error was not propagated")
        if name == "stable-thread":
            for field in ("native_children_observed", "caller_threads_returned", "pressure_threads"):
                if type(case.get(field)) is not int or case[field] != 4:
                    raise ValueError("Rust launcher concurrency fixture differs")


def validate_generated_observation(observation, deadline_case=None, failure_matrix=False, publication_errors=False):
    if (type(failure_matrix) is not bool or type(publication_errors) is not bool
            or sum((deadline_case is not None, failure_matrix, publication_errors)) > 1):
        raise ValueError("generated-secret experiment selections conflict")
    fields = {"agent_load_tested", "cases", "complete", "control_kind", "custody_accredited",
              "deadline_expiry_tested", "experiment", "helper_sha256", "kdf_policy_adopted",
              "native_public_extraction_matches", "native_public_extraction_tested", "native_unlock_tested",
              "performance_admitted", "production_launcher_tested", "provenance", "resource_limits",
              "resources_closed_after_measurement", "retained_control_accepted", "source_sha256", "status", "version"}
    if failure_matrix:
        fields |= {"failure_seams_tested", "terminal_replacement_tested", "fsync_failure_tested"}
        if (observation.get("failure_seams_tested") is not True
                or observation.get("terminal_replacement_tested") is not False
                or observation.get("fsync_failure_tested") is not False):
            raise ValueError("generated-secret failure coverage differs")
    if publication_errors:
        fields |= {"publication_error_returns_tested", "terminal_replacement_tested", "fsync_failure_tested"}
        if (observation.get("publication_error_returns_tested") is not True
                or observation.get("terminal_replacement_tested") is not False
                or observation.get("fsync_failure_tested") is not False):
            raise ValueError("generated-secret publication error coverage differs")
    if deadline_case is not None:
        fields.add("deadline_case")
        if deadline_case not in ("acknowledgement", "confirmation") or observation.get("deadline_case") != deadline_case:
            raise ValueError("generated-secret deadline selection differs")
    if (set(observation) != fields or observation.get("status") != "observed"
            or type(observation.get("version")) is not int or observation["version"] != 1
            or observation.get("experiment") != ("generated-publication-errors" if publication_errors else
                                                   "generated-failures" if failure_matrix else
                                                   "generated-deadline" if deadline_case else "generated")
            or observation.get("control_kind") != "valid-package-check"):
        raise ValueError("generated-secret report identity differs")
    for field, names in (("helper_sha256", {"writer", "verifier"}),
                         ("source_sha256", {"generated_probe", "provider_probe"})):
        hashes = observation[field]
        if (not isinstance(hashes, dict) or set(hashes) != names
                or any(not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None
                       for value in hashes.values())):
            raise ValueError("generated-secret component identity differs")
    for field in ("complete", "resources_closed_after_measurement", "retained_control_accepted",
                  "native_public_extraction_matches", "native_public_extraction_tested"):
        if observation.get(field) is not True:
            raise ValueError("generated-secret observation was not established")
    for field in ("custody_accredited", "performance_admitted", "kdf_policy_adopted", "native_unlock_tested",
                  "production_launcher_tested", "agent_load_tested"):
        if observation.get(field) is not False:
            raise ValueError("generated-secret report claims untested admission")
    if observation.get("deadline_expiry_tested") is not (deadline_case is not None):
        raise ValueError("generated-secret deadline evidence differs")
    cases = observation.get("cases")
    required = {"valid-generated-package", "deadline-" + deadline_case} if deadline_case else GENERATED_CASES
    if failure_matrix:
        required = GENERATED_FAILURE_CASES
    if publication_errors:
        required = GENERATED_ERROR_CASES
    if (not isinstance(cases, list) or len(cases) != len(required)
            or {case.get("case") for case in cases} != required):
        raise ValueError("generated-secret case matrix differs")
    fields = {"case", "status", "exit_code", "package_created", "terminal_restored", "disclosures",
              "echo_disabled", "foreground", "private_arguments", "standard_streams_clean",
              "process_metadata_checked"}
    for case in cases:
        name = case["case"]
        positive = name == "valid-generated-package"
        interrupted = name.startswith("interrupted-")
        returned = name.startswith("returned-")
        committed = name in ("interrupted-after-publish", "returned-after-publish")
        expected_fields = fields | ({"structural_check_accepted", "native_public_extraction_matches"} if positive else set())
        if name.startswith("terminal-drift-"):
            expected_fields.add("attrs_changed")
            if case.get("attrs_changed") is not True or case.get("exit_code") != 1:
                raise ValueError("generated-secret terminal drift was not observed")
        if interrupted:
            expected_fields |= {"fault_point", "preparations_remaining", "package_check_accepted", "filesystem_secret_absent"}
            if committed:
                expected_fields.add("native_public_extraction_matches")
            if (case.get("fault_point") != name.removeprefix("interrupted-")
                    or type(case.get("preparations_remaining")) is not int
                    or case["preparations_remaining"] != (0 if committed else 1)
                    or case.get("package_check_accepted") is not (True if committed else None)
                    or case.get("filesystem_secret_absent") is not True
                    or (committed and case.get("native_public_extraction_matches") is not True)):
                raise ValueError("generated-secret publication seam differs")
        if returned:
            expected_fields |= {"error_point", "preparations_remaining", "package_check_accepted",
                                "filesystem_secret_absent", "indeterminate_durability_reported"}
            if committed:
                expected_fields.add("native_public_extraction_matches")
            if (case.get("error_point") != name.removeprefix("returned-")
                    or type(case.get("preparations_remaining")) is not int or case["preparations_remaining"] != 0
                    or case.get("package_check_accepted") is not (True if committed else None)
                    or case.get("filesystem_secret_absent") is not True
                    or case.get("indeterminate_durability_reported") is not committed
                    or (committed and case.get("native_public_extraction_matches") is not True)):
                raise ValueError("generated-secret error-return cleanup differs")
        if name.startswith("deadline-"):
            expected_fields.add("delay_elapsed_ms")
            elapsed = case.get("delay_elapsed_ms")
            lower, upper = (59000, 80000) if deadline_case == "acknowledgement" else (179000, 200000)
            if type(elapsed) not in (int, float) or not math.isfinite(elapsed) or not lower <= elapsed <= upper:
                raise ValueError("generated-secret real deadline was not observed")
        if set(case) != expected_fields or case.get("status") != "observed":
            raise ValueError("generated-secret case schema differs")
        code = case["exit_code"]
        if (type(code) is not int or (code == 0) is not positive
                or (interrupted and code != 77)
                or (returned and code != (2 if committed else 1))
                or case["package_created"] is not (positive or committed) or case["standard_streams_clean"] is not True):
            raise ValueError("generated-secret case outcome differs")
        headless = name in ("no-tty", "unset-disposable")
        for field in ("echo_disabled", "foreground"):
            if case[field] is not (None if headless else True):
                raise ValueError("generated-secret terminal boundary differs")
        disclosed = name not in GENERATED_NO_DISCLOSURE and name not in (
            "deadline-acknowledgement", "terminal-drift-acknowledgement")
        if (type(case["disclosures"]) is not int or case["disclosures"] != int(disclosed)
                or case["process_metadata_checked"] is not disclosed
                or case["private_arguments"] is not (True if disclosed else None)):
            raise ValueError("generated-secret disclosure boundary differs")
        if case["terminal_restored"] is not (None if headless or name == "terminal-hangup" else True):
            raise ValueError("generated-secret terminal restoration differs")
        if positive and (case["structural_check_accepted"] is not True
                         or case["native_public_extraction_matches"] is not True):
            raise ValueError("generated-secret positive control differs")


def validate_concurrent_observation(observation):
    if observation.get("concurrent_lock_tested") is not True:
        raise ValueError("concurrent lock observation was not established")
    for case in observation["cases"]:
        seam, outcome, sent, replies, verified, published = CONCURRENT_EXPECTED[case["case"]]
        expected = {"selected_seam": seam, "request_outcome": outcome, "state": "CLOSED",
                    "native_requests_sent": sent, "proofs_verified": verified, "proofs_published": published}
        if any(case.get(key) != value for key, value in expected.items()):
            raise ValueError("concurrent case did not establish its selected outcome")
        for field in ("resources_closed_after_drain", "workers_joined", "close_acknowledged"):
            if case.get(field) is not True:
                raise ValueError("concurrent closure was not established")
        for field in ("native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published"):
            if type(case.get(field)) is not int or not 0 <= case[field] <= 1:
                raise ValueError("concurrent counters were malformed")
        if (replies is not None and case["native_replies_received"] != replies) or case.get("active") is not False:
            raise ValueError("concurrent native work did not drain")
        if outcome == "cancelled_by_lock":
            for field in ("request_late_refused", "closing_observed_while_paused", "drain_wait_observed"):
                if case.get(field) is not True:
                    raise ValueError("concurrent overlap was not established")
        if case["case"] == "parallel-lock-callers" and case.get("lock_acknowledgments") != 2:
            raise ValueError("parallel close callers were not acknowledged")


def validate_loader_observation(observation):
    if observation.get("loader_lifecycle_tested") is not True:
        raise ValueError("loader lifecycle observation was not established")
    for case in observation["cases"]:
        seam, trigger, outcome = LOADER_EXPECTED[case["case"]]
        crash = trigger == "supervisor-kill"
        loaded = seam in ("before-readiness", "ready")
        writes = 0 if seam == "prompt" else 1
        expected = {"selected_seam": seam, "trigger": trigger, "outcome": outcome,
                    "readiness_published": outcome == "ready_closed", "close_acknowledged": not crash,
                    "secret_writes": writes, "loader_alive_at_trigger": not loaded,
                    "loader_suspended_at_handoff": writes == 1,
                    "loader_returncode": 0 if loaded else -9, "agent_returncode": -9,
                    "supervisor_returncode": -9 if crash else 0,
                    "resources_closed_after_exit": not crash,
                    "pre_ready_signature_verified": loaded}
        if any(type(case.get(key)) is not type(value) or case[key] != value for key, value in expected.items()):
            raise ValueError("loader case did not establish its selected outcome")
        for field in ("selected_seam_reached", "prompt_echo_disabled", "pty_master_retained",
                      "loader_exit_observed", "agent_exit_observed", "supervisor_exit_observed",
                      "retained_connection_refused", "new_connection_refused", "processes_drained"):
            if case.get(field) is not True:
                raise ValueError("loader exit or refusal was not observed")


def validate_ssh_observation(observation):
    if observation.get("ssh_disconnect_tested") is not True:
        raise ValueError("SSH disconnect observation was not established")
    for case in observation["cases"]:
        mode, seam, disconnected = SSH_EXPECTED[case["case"]]
        loaded = seam == "ready"
        expected = {"ssh_mode": mode, "selected_seam": seam,
                    "trigger": "ssh-client-kill" if disconnected else "close",
                    "outcome": "ssh_disconnected" if disconnected else "ready_closed",
                    "disconnect_observed": disconnected, "readiness_published": loaded,
                    "secret_writes": 0 if seam == "prompt" else 1,
                    "loader_alive_at_trigger": not loaded,
                    "loader_suspended_at_handoff": seam != "prompt",
                    "loader_returncode": 0 if loaded else -9, "agent_returncode": -9,
                    "ssh_client_returncode": -9 if disconnected else 0,
                    "pre_ready_signature_verified": loaded}
        if any(type(case.get(key)) is not type(value) or case[key] != value for key, value in expected.items()):
            raise ValueError("SSH case did not establish its selected outcome")
        if (disconnected and case.get("disconnect_cause") not in ("eof", "eio", "sighup")) or (
                not disconnected and case.get("disconnect_cause") is not None):
            raise ValueError("SSH transport closure was not observed")
        for field in ("selected_seam_reached", "close_acknowledged", "prompt_echo_disabled",
                      "pty_master_retained", "loader_exit_observed", "agent_exit_observed",
                      "supervisor_exit_observed", "resources_closed_after_exit",
                      "retained_connection_refused", "new_connection_refused", "processes_drained"):
            if case.get(field) is not True:
                raise ValueError("SSH exit or refusal was not observed")


def validate_provider_observation(observation):
    expected = {"provider_capabilities_tested": True, "provider_admitted": False,
                "os_id": "ubuntu", "os_version": "24.04", "package_name": "openssh-client",
                "provenance_kind": "administrator-trusted-dpkg-database",
                "archive_signature_verified": False}
    if any(type(observation.get(key)) is not type(value) or observation[key] != value
           for key, value in expected.items()):
        raise ValueError("provider observation exceeded its admitted scope")
    if any(observation.get(key) is not True for key in PROVIDER_CHECKS):
        raise ValueError("provider checks were not established")
    if (not isinstance(observation.get("package_version"), str)
            or not re.fullmatch(r"[0-9][0-9A-Za-z.+:~\-]{0,127}", observation["package_version"])
            or not isinstance(observation.get("package_arch"), str)
            or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,31}", observation["package_arch"])):
        raise ValueError("provider package identity was malformed")
    components = observation.get("components", [])
    if len(components) != 4 or {item.get("name") for item in components} != PROVIDER_COMPONENTS:
        raise ValueError("provider component identity was incomplete")
    for item in components:
        if (item.get("path") != "/usr/bin/" + item["name"] or not isinstance(item.get("sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])
                or any(item.get(key) is not True for key in
                       ("root_owned", "nonwritable", "fd_retained", "revalidated"))):
            raise ValueError("provider executable object was not established")
    if any(case.get("observed") is not True for case in observation["cases"]):
        raise ValueError("provider case was not observed")


def validate_kdf_observation(observation, series):
    expected = {"series": series, "samples_per_case": 7, "rounds": 64,
                "kdf_measurements_tested": True, "kdf_policy_adopted": False,
                "performance_admitted": False, "parser_auxiliary_heap_measured": False,
                "p99_estimated": False}
    if any(type(observation.get(key)) is not type(value) or observation[key] != value
           for key, value in expected.items()):
        raise ValueError("KDF measurement scope was not established")
    for key in ("all_proofs_verified", "profile_refusals_before_native", "native_exits_observed",
                "retained_resources_closed_after_exit", "snapshots_revalidated"):
        if observation.get(key) is not True:
            raise ValueError("KDF measurement control was not established")
    if type(observation.get("child_maxrss_kib")) is not int or observation["child_maxrss_kib"] <= 0:
        raise ValueError("child RSS observation was not established")
    for case in observation["cases"]:
        nominal = KDF_TARGETS[case["case"]]
        band = nominal * 11 / 10
        samples = case.get("samples_ms")
        if (not isinstance(samples, list) or len(samples) != 7
                or any(type(value) not in (int, float) or not math.isfinite(value) or value < 0 for value in samples)):
            raise ValueError("KDF samples were incomplete or malformed")
        classification = "nominal" if max(samples) <= nominal else "within-tolerance" if max(samples) <= band else "above-band"
        if (type(case.get("nominal_ms")) not in (int, float) or case["nominal_ms"] != nominal
                or type(case.get("tolerance_ms")) not in (int, float)
                or not math.isclose(case["tolerance_ms"], band, rel_tol=0, abs_tol=1e-9)
                or case.get("classification") != classification):
            raise ValueError("KDF classification did not preserve observed samples")
        if case["case"].startswith("reject-"):
            if (case.get("structural_exit_codes") != [1] * 7
                    or any(type(value) is not int for value in case["structural_exit_codes"])
                    or type(case.get("native_dispatches")) is not int or case["native_dispatches"] != 0
                    or case.get("control_package_accepted") is not True or case.get("mutation_rehashed") is not True):
                raise ValueError("KDF refusal reached native dispatch or lacked its positive control")
    attempts = observation.get("raw_attempts")
    expected_attempts = [(case["case"], sample) for case in observation["cases"] for sample in case["samples_ms"]]
    if not isinstance(attempts, list) or len(attempts) != 70:
        raise ValueError("KDF attempt ledger was incomplete")
    for attempt, (name, sample) in zip(attempts, expected_attempts):
        if (not isinstance(attempt, dict) or set(attempt) != {"case", "elapsed_ms", "returned"}
                or attempt["case"] != name or attempt["returned"] is not True
                or type(attempt["elapsed_ms"]) not in (int, float)
                or not math.isfinite(attempt["elapsed_ms"]) or attempt["elapsed_ms"] != sample):
            raise ValueError("KDF attempt ledger did not preserve sample order and outcome")


def validate_heap_observation(observation):
    expected = {"heap_measurements_tested": True, "parser_auxiliary_heap_measured": True,
                "measurement_kind": "additional-live-rust-requested-heap", "budget_bytes": 2097152,
                "samples_per_case": 3, "control_kind": "valid-package-check"}
    if any(type(observation.get(key)) is not type(value) or observation[key] != value
           for key, value in expected.items()):
        raise ValueError("heap measurement scope was not established")
    for key in ("all_measurements_valid", "fixtures_rehashed", "checker_equivalence",
                "resources_closed_after_measurement", "retained_control_accepted"):
        if observation.get(key) is not True:
            raise ValueError("heap measurement controls were not established")
    for key in ("kdf_policy_adopted", "performance_admitted"):
        if observation.get(key) is not False:
            raise ValueError("heap observation exceeded its scope")

    def validate(value, accepted):
        fields = {"version", "measurement", "accepted", "baseline_bytes", "peak_additional_bytes",
                  "live_after_bytes", "budget_bytes", "within_budget", "allocator_valid", "realloc_conservative"}
        if not isinstance(value, dict) or set(value) != fields:
            raise ValueError("heap counter schema was malformed")
        if (type(value["version"]) is not int or value["version"] != 1
                or value["measurement"] != expected["measurement_kind"] or value["accepted"] is not accepted
                or any(value[key] is not True for key in ("within_budget", "allocator_valid", "realloc_conservative"))):
            raise ValueError("heap allocator result was invalid")
        for key in ("baseline_bytes", "peak_additional_bytes", "live_after_bytes", "budget_bytes"):
            if type(value[key]) is not int or not 0 <= value[key] <= 0xffffffffffffffff:
                raise ValueError("heap counters were malformed")
        if (value["budget_bytes"] != 2097152 or value["peak_additional_bytes"] > 2097152
                or value["baseline_bytes"] + value["peak_additional_bytes"] > 0xffffffffffffffff
                or value["live_after_bytes"] > value["baseline_bytes"] + value["peak_additional_bytes"]):
            raise ValueError("heap counters exceeded their hard bounds")

    for case in observation["cases"]:
        accepted = HEAP_EXPECTED[case["case"]]
        if (case.get("expected_accepted") is not accepted
                or case.get("mutation_rehashed") is not (case["case"] != "valid-package")
                or type(case.get("baseline_checker_exit_code")) is not int
                or case["baseline_checker_exit_code"] != (0 if accepted else 1)):
            raise ValueError("heap checker equivalence was not established")
        rows = case.get("observations")
        if not isinstance(rows, list) or len(rows) != 3:
            raise ValueError("heap observations were incomplete")
        for value in rows:
            validate(value, accepted)
        if (type(case.get("peak_additional_bytes")) is not int
                or case["peak_additional_bytes"] != max(value["peak_additional_bytes"] for value in rows)):
            raise ValueError("heap summary hid an observation")
    validate(observation.get("retained_control_observation"), True)


def command(arguments, timeout=15, payload=None, merge_stderr=False):
    result = subprocess.run(arguments, stdin=subprocess.DEVNULL if payload is None else None, input=payload,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT if merge_stderr else subprocess.DEVNULL,
                            timeout=timeout, check=False)
    if result.returncode:
        raise RunnerError("control command failed: " + arguments[0])
    if len(result.stdout) > 128 * 1024:
        raise RunnerError("control output exceeded its bound")
    return result.stdout


def initialize_reports(container_id, mount="/reports"):
    # Only metadata for a new private mount, applied while the container is stopped.
    if mount not in ("/reports", "/store"):
        raise RunnerError("private directory destination was refused")
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as output:
        directory = tarfile.TarInfo("run")
        directory.type = tarfile.DIRTYPE
        directory.uid = directory.gid = 65534
        directory.mode = 0o700
        output.addfile(directory)
    command(["docker", "cp", "--archive", "-", container_id + ":" + mount], payload=archive.getvalue())


def initialize_root_reader_fixtures(container_id):
    # Public synthetic fixtures, populated before the unprivileged container starts.
    binding = b'{"version":1,"installation_id":"00000000000000000000000001","owner_uid":65534,"owner_gid":65534,"profile_revision":1}'
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as output:
        def add(name, mode, uid=0, *, kind=tarfile.DIRTYPE, data=b"", target=""):
            parts = name.split("/")
            if name.startswith("/") or any(part in ("", ".", "..") for part in parts):
                raise RunnerError("fixture archive path was refused")
            if target and (target.startswith("/") or any(part in ("", ".", "..") for part in target.split("/"))):
                raise RunnerError("fixture archive link was refused")
            member = tarfile.TarInfo(name)
            member.uid = member.gid = uid
            member.mode = mode
            member.type = kind
            member.linkname = target
            member.size = len(data) if kind == tarfile.REGTYPE else 0
            output.addfile(member, io.BytesIO(data) if kind == tarfile.REGTYPE else None)
        for case in sorted(RUST_ROOT_READER_CASES):
            add(case, 0o755, 65534 if case == "wrong-root-owner" else 0)
            add(case + "/var", 0o777 if case == "writable-ancestor" else 0o755)
            lib = case + ("/var/lib-real" if case == "ancestor-symlink" else "/var/lib")
            add(lib, 0o755)
            add(lib + "/vivac", 0o755)
            add(lib + "/vivac/custody-v1", 0o555 if case == "wrong-prefix-mode" else 0o755)
            add(lib + "/vivac/custody-v1/users", 0o755)
            slot = lib + "/vivac/custody-v1/users/65534"
            add(slot, 0o755, 65534 if case == "wrong-slot-owner" else 0)
            add(slot + "/state", 0o755 if case == "state-mode" else 0o700, 65534)
            add(slot + "/state/packages", 0o700, 65534)
            add(slot + "/state/admissions", 0o700, 65534)
            if case == "ancestor-symlink":
                add(case + "/var/lib", 0o777, kind=tarfile.SYMTYPE, target="lib-real")
            if case == "missing-binding": continue
            contents = binding + b"\n" if case == "binding-malformed" else b"x" * 513 if case == "binding-oversized" else binding
            if case == "binding-identity": contents = binding.replace(b'"owner_uid":65534', b'"owner_uid":65533')
            if case in ("binding-symlink", "binding-hardlink"):
                add(slot + "/binding-real.json", 0o644, kind=tarfile.REGTYPE, data=contents)
                target = "binding-real.json" if case == "binding-symlink" else slot + "/binding-real.json"
                add(slot + "/binding.json", 0o777 if case == "binding-symlink" else 0o644,
                    kind=tarfile.SYMTYPE if case == "binding-symlink" else tarfile.LNKTYPE, target=target)
            elif case == "binding-fifo":
                add(slot + "/binding.json", 0o644, kind=tarfile.FIFOTYPE)
            else:
                add(slot + "/binding.json", 0o664 if case == "binding-mode" else 0o644,
                    65534 if case == "binding-owner" else 0, kind=tarfile.REGTYPE, data=contents)
    command(["docker", "cp", "-", container_id + ":/store/run"], payload=archive.getvalue())


def rdp_events(since):
    env = dict(os.environ, VIVAC_EVENT_SINCE=since)
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", EVENT_QUERY],
        env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, timeout=10, check=False,
    )
    if result.returncode or len(result.stdout) > 32 * 1024:
        raise RunnerError("RDP event observation is unavailable")
    events = json.loads(result.stdout.decode("utf-8-sig"))
    if not isinstance(events, list):
        raise RunnerError("RDP event observation was malformed")
    return events


def verify_config(config, build_volume, report_volume, store_volume=None, heap_build_volume=None,
                  generated_build_volume=None, launcher_build_volume=None):
    host = config["HostConfig"]
    expected = {"Memory": LIMIT, "MemorySwap": LIMIT, "NanoCpus": 1_000_000_000,
                "PidsLimit": 128, "ReadonlyRootfs": True, "NetworkMode": "none"}
    if any(host.get(key) != value for key, value in expected.items()):
        raise RunnerError("container resource configuration was refused")
    if config["Config"].get("User") != "65534:65534":
        raise RunnerError("container user configuration was refused")
    if host.get("CapDrop") != ["ALL"] or "no-new-privileges" not in host.get("SecurityOpt", []):
        raise RunnerError("container privilege configuration was refused")
    if host.get("PidMode") or host.get("Privileged"):
        raise RunnerError("container process isolation was refused")
    if host.get("LogConfig") != {"Type": "json-file", "Config": {"max-size": "1m", "max-file": "1"}}:
        raise RunnerError("container log configuration was refused")
    tmpfs = host.get("Tmpfs", {})
    if tmpfs != {"/tmp": "rw,nosuid,nodev,mode=0700,uid=65534,gid=65534,size=128m"}:
        raise RunnerError("container temporary storage configuration was refused")
    mounts = {mount["Destination"]: mount for mount in config["Mounts"]}
    expected_mounts = {"/probe", "/build", "/reports"}
    if store_volume is not None:
        expected_mounts.add("/store")
    if heap_build_volume is not None:
        expected_mounts.add("/heap-build")
    if generated_build_volume is not None:
        expected_mounts.add("/generated-build")
    if launcher_build_volume is not None:
        expected_mounts.add("/launcher-build")
    if set(mounts) != expected_mounts:
        raise RunnerError("container mounts were refused")
    if (mounts["/probe"]["RW"] or mounts["/build"]["RW"]
            or mounts["/build"].get("Name") != build_volume
            or mounts["/reports"].get("Name") != report_volume):
        raise RunnerError("container volume ownership was refused")
    if store_volume is not None and (mounts["/store"].get("Name") != store_volume
                                     or not mounts["/store"]["RW"]):
        raise RunnerError("container store ownership was refused")
    if heap_build_volume is not None and (mounts["/heap-build"].get("Name") != heap_build_volume
                                          or mounts["/heap-build"]["RW"]):
        raise RunnerError("instrumented helper mount was refused")
    if generated_build_volume is not None and (
            mounts["/generated-build"].get("Name") != generated_build_volume
            or mounts["/generated-build"]["RW"]):
        raise RunnerError("generated-secret helper mount was refused")
    if launcher_build_volume is not None and (mounts["/launcher-build"].get("Name") != launcher_build_volume
                                              or mounts["/launcher-build"]["RW"]):
        raise RunnerError("Rust launcher helper mount was refused")


def run(args):
    if sys.platform != "win32" or not os.environ.get("RUST_WORK_JOB"):
        raise RunnerError("run this Windows runtime suite through RustWork --family rust")
    if not args.reports_dir.is_absolute() or args.reports_dir.exists():
        raise RunnerError("select a new absolute report directory")
    experiment = getattr(args, "experiment", "lifecycle")
    if experiment not in EXPERIMENTS:
        raise RunnerError("unknown experiment was refused")
    diagnostic = getattr(args, "diagnostic_startup", False)
    if diagnostic and experiment != "loader":
        raise RunnerError("startup diagnostics are restricted to the loader experiment")
    series = getattr(args, "series", None)
    if (experiment == "kdf" and series not in ("A", "B")) or (experiment != "kdf" and series is not None):
        raise RunnerError("measurement series must select A or B only for the KDF experiment")
    heap_build = getattr(args, "heap_build_volume", None)
    if (experiment == "heap" and (not heap_build or heap_build == args.build_volume)) or (experiment != "heap" and heap_build):
        raise RunnerError("heap measurement requires a separate instrumented build volume")
    generated_build = getattr(args, "generated_build_volume", None)
    launcher_build = getattr(args, "launcher_build_volume", None)
    if ((experiment in ("rust-launcher", "rust-loader", "rust-ssh-loader", "rust-concurrent", "rust-creation-binding", "rust-ssh-concurrent", "rust-abandon", "rust-isolation", "rust-ipc", "rust-reservation", "rust-connected", "rust-admission-storage", "rust-root-reader") and (not launcher_build or launcher_build == args.build_volume))
            or (experiment not in ("rust-launcher", "rust-loader", "rust-ssh-loader", "rust-concurrent", "rust-creation-binding", "rust-ssh-concurrent", "rust-abandon", "rust-isolation", "rust-ipc", "rust-reservation", "rust-connected", "rust-admission-storage", "rust-root-reader") and launcher_build)):
        raise RunnerError("Rust launcher experiment requires a separate helper build volume")
    generated_experiment = experiment in ("generated", "generated-deadline", "generated-failures", "generated-publication-errors")
    if ((generated_experiment and (not generated_build or generated_build == args.build_volume))
            or (not generated_experiment and generated_build)):
        raise RunnerError("generated-secret experiment requires a separate helper build volume")
    script, required_cases = EXPERIMENTS[experiment]
    deadline_case = getattr(args, "deadline_case", None)
    if ((experiment == "generated-deadline" and deadline_case not in ("acknowledgement", "confirmation"))
            or (experiment != "generated-deadline" and deadline_case is not None)):
        raise RunnerError("real deadline selection requires its own generated-secret experiment")
    if deadline_case:
        required_cases = {"valid-generated-package", "deadline-" + deadline_case}
    from datetime import datetime, timedelta, timezone

    baseline = (datetime.now(timezone.utc) - timedelta(seconds=60)).isoformat()
    since = datetime.now(timezone.utc).isoformat()
    args.reports_dir.mkdir(parents=False)
    token = uuid.uuid4().hex
    name = "vivac-lifecycle-" + token
    report_volume = "vivac-lifecycle-report-" + token
    store_volume = "vivac-session-store-" + token if experiment in ("session", "concurrent", "loader", "ssh", "kdf", "heap", "generated", "generated-deadline", "generated-failures", "generated-publication-errors", "rust-launcher", "rust-loader", "rust-ssh-loader", "rust-concurrent", "rust-creation-binding", "rust-ssh-concurrent", "rust-abandon", "rust-isolation", "rust-ipc", "rust-reservation", "rust-connected", "rust-admission-storage", "rust-root-reader") else None
    report = {"status": "incomplete", "custody_accredited": False,
              "experiment": experiment, "container": name, "report_volume": report_volume, "events": []}
    if diagnostic:
        report["diagnostic_only"] = True
    if series is not None:
        report["series"] = series
    if store_volume is not None:
        report["store_volume"] = store_volume
    created = False
    container_id = None
    try:
        report["events"] = rdp_events(baseline)
        if report["events"]:
            raise RunnerError("recent session errors refuse a new experiment")
        image = json.loads(command(["docker", "image", "inspect", "--format", "{{json .Id}}", args.image]))
        if not isinstance(image, str) or not image.startswith("sha256:"):
            raise RunnerError("local image identity was unavailable")
        report["image_id"] = image
        command(["docker", "volume", "inspect", args.build_volume])
        if heap_build:
            command(["docker", "volume", "inspect", heap_build])
            report["heap_build_volume"] = heap_build
        if generated_build:
            command(["docker", "volume", "inspect", generated_build])
            report["generated_build_volume"] = generated_build
        if launcher_build:
            command(["docker", "volume", "inspect", launcher_build])
            report["launcher_build_volume"] = launcher_build
        container_id = command(["docker", "create", "--name", name, "--read-only", "--network", "none",
                 "--cap-drop", "ALL", "--user", "65534:65534",
                 "--security-opt", "no-new-privileges", "--cpus=1", "--memory=512m",
                 "--memory-swap=512m", "--pids-limit=128", "--ulimit", "core=0",
                 "--log-driver", "json-file", "--log-opt", "max-size=1m",
                 "--log-opt", "max-file=1", "--tmpfs",
                 "/tmp:rw,nosuid,nodev,mode=0700,uid=65534,gid=65534,size=128m",
                 "--env", "VIVAC_OPENSSH_DISPOSABLE=1", "--mount",
                 "type=bind,source=" + str(Path(__file__).resolve().parent) + ",target=/probe,readonly",
                 "--mount", "type=volume,source=" + args.build_volume + ",target=/build,readonly",
                 "--mount", "type=volume,source=" + report_volume + ",target=/reports"]
                 + (["--mount", "type=volume,source=" + store_volume + ",target=/store"] if store_volume else [])
                 + (["--mount", "type=volume,source=" + heap_build + ",target=/heap-build,readonly"] if heap_build else [])
                 + (["--mount", "type=volume,source=" + generated_build + ",target=/generated-build,readonly"] if generated_build else [])
                 + (["--mount", "type=volume,source=" + launcher_build + ",target=/launcher-build,readonly"] if launcher_build else [])
                 + ["--entrypoint", "/usr/bin/timeout", image,
                 "--signal=TERM", "--kill-after=5s", "240s", "python3", "-B", "/probe/" + script,
                 "--writer", ("/generated-build" if generated_build else "/build") + "/writer189/debug/ssh-writer-graph-audit",
                 "--verifier", ("/usr/bin/openssl" if experiment == "rust-creation-binding"
                                else "/build/writer189/debug/openssh-proof-verifier-probe")]
                 + (["--store-root", "/store/run"] if store_volume else [])
                 + (["--fixture-root", "/store/run"] if experiment == "rust-root-reader" else [])
                 + (["--diagnostic-startup"] if diagnostic else [])
                 + (["--series", series] if series is not None else [])
                 + (["--heap-helper", "/heap-build/writer189/debug/ssh-writer-graph-audit"] if heap_build else [])
                 + (["--deadline-case", deadline_case] if deadline_case else [])
                 + (["--failure-matrix"] if experiment == "generated-failures" else [])
                 + (["--publication-errors"] if experiment == "generated-publication-errors" else [])
                 + (["--launcher-helper", "/launcher-build/launcher189/debug/rust-launcher-probe"] if launcher_build else [])
                 + ["--report", "/reports/run/observations.json"]).decode("ascii").strip()
        if len(container_id) != 64 or any(char not in "0123456789abcdef" for char in container_id):
            raise RunnerError("created container identity was unavailable")
        created = True
        report["container_id"] = container_id
        config = json.loads(command(["docker", "inspect", container_id]))[0]
        if config["Id"] != container_id:
            raise RunnerError("created container identity changed")
        verify_config(config, args.build_volume, report_volume, store_volume, heap_build, generated_build, launcher_build)
        initialize_reports(container_id)
        if store_volume is not None:
            initialize_reports(container_id, "/store")
            if experiment == "rust-root-reader":
                initialize_root_reader_fixtures(container_id)
        report["limits_verified"] = True
        report["events"] = rdp_events(since)
        if report["events"]:
            raise RunnerError("new session errors refused container startup")
        command(["docker", "start", container_id])
        deadline = time.monotonic() + DEADLINE
        while True:
            report["events"] = rdp_events(since)
            if report["events"]:
                raise RunnerError("new session errors interrupted the experiment")
            state = json.loads(command(["docker", "inspect", "--format", "{{json .State}}", container_id]))
            report["state"] = state
            if not state["Running"]:
                if state.get("OOMKilled") or state["ExitCode"] != 0:
                    raise RunnerError("experiment did not complete successfully")
                report["events"] = rdp_events(since)
                if report["events"]:
                    raise RunnerError("new session errors interrupted the experiment")
                report["status"] = "observed"
                break
            if time.monotonic() >= deadline:
                raise RunnerError("experiment exceeded its total deadline")
            time.sleep(2)
    except (RunnerError, OSError, ValueError, subprocess.SubprocessError) as error:
        report["failure"] = str(error) if isinstance(error, RunnerError) else type(error).__name__
    finally:
        if created:
            stopped = False
            try:
                command(["docker", "stop", "--time", "2", container_id], timeout=12)
                stopped = True
            except (RunnerError, OSError, subprocess.SubprocessError):
                try:
                    command(["docker", "kill", container_id], timeout=12)
                    final_state = json.loads(command(["docker", "inspect", "--format", "{{json .State}}", container_id]))
                    stopped = not final_state["Running"]
                except (RunnerError, OSError, ValueError, KeyError, subprocess.SubprocessError):
                    pass
                if not stopped:
                    report["cleanup_failed"] = True
            if stopped:
                try:
                    report["final_state"] = json.loads(command(
                        ["docker", "inspect", "--format", "{{json .State}}", container_id]))
                except (RunnerError, OSError, ValueError, subprocess.SubprocessError):
                    report["state_recovery_failed"] = True
                try:
                    log = command(["docker", "logs", "--tail", "40", container_id], merge_stderr=True)
                    (args.reports_dir / "container.log").write_bytes(log)
                except (RunnerError, OSError, subprocess.SubprocessError):
                    report["log_recovery_failed"] = True
                try:
                    command(["docker", "cp", container_id + ":/reports/run/.", str(args.reports_dir)], timeout=15)
                    command(["docker", "rm", container_id])
                    if store_volume is not None:
                        command(["docker", "volume", "rm", store_volume])
                except (RunnerError, OSError, subprocess.SubprocessError):
                    report["recovery_failed"] = True
        if diagnostic:
            report["status"] = "incomplete"
            report.setdefault("failure", "startup diagnostics cannot accredit the loader matrix")
        if report["status"] == "observed":
            try:
                observation = json.loads((args.reports_dir / "observations.json").read_text(encoding="utf-8"))
                cases = observation.get("cases", [])
                control_ok = observation.get("retained_control_accepted") is True if experiment == "heap" or generated_experiment else observation.get("sentinel_survived") is True
                if experiment in ("rust-launcher", "rust-abandon", "rust-ipc"):
                    control_ok = observation.get("resources_closed_after_exit") is True
                elif experiment in ("rust-isolation", "rust-reservation", "rust-connected", "rust-admission-storage", "rust-root-reader"):
                    control_ok = observation.get("resources_closed_after_observed_exit") is True
                if (observation.get("status") != "observed" or observation.get("custody_accredited") is not False
                        or not control_ok
                        or len(cases) != len(required_cases)
                        or {case.get("case") for case in cases} != required_cases):
                    raise ValueError("incomplete observations")
                if experiment == "concurrent":
                    validate_concurrent_observation(observation)
                elif experiment == "loader":
                    validate_loader_observation(observation)
                elif experiment == "ssh":
                    validate_ssh_observation(observation)
                elif experiment == "provider":
                    validate_provider_observation(observation)
                elif experiment == "kdf":
                    validate_kdf_observation(observation, series)
                elif experiment == "heap":
                    validate_heap_observation(observation)
                elif generated_experiment:
                    validate_generated_observation(observation, deadline_case, experiment == "generated-failures",
                                                   experiment == "generated-publication-errors")
                elif experiment == "rust-launcher":
                    validate_rust_launcher_observation(observation)
                elif experiment == "rust-loader":
                    validate_rust_loader_observation(observation)
                elif experiment == "rust-ssh-loader":
                    validate_rust_ssh_loader_observation(observation)
                elif experiment == "rust-concurrent":
                    validate_rust_concurrent_observation(observation)
                elif experiment == "rust-creation-binding":
                    validate_rust_creation_binding_observation(observation)
                elif experiment == "rust-ssh-concurrent":
                    validate_rust_ssh_concurrent_observation(observation)
                elif experiment == "rust-abandon":
                    validate_rust_abandon_observation(observation)
                elif experiment == "rust-isolation":
                    validate_rust_isolation_observation(observation)
                elif experiment == "rust-reservation":
                    validate_rust_reservation_observation(observation)
                elif experiment == "rust-admission-storage":
                    validate_rust_admission_storage_observation(observation)
                elif experiment == "rust-root-reader":
                    validate_rust_root_reader_observation(observation)
                elif experiment == "rust-connected":
                    validate_rust_connected_observation(observation)
                elif experiment == "rust-ipc":
                    validate_rust_ipc_observation(observation)
            except (OSError, ValueError, TypeError, AttributeError, KeyError):
                report["status"] = "incomplete"
                report["failure"] = "experiment observations were not recovered"
        (args.reports_dir / "runner.json").write_text(
            json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return 0 if report["status"] == "observed" and not report.get("cleanup_failed") and not report.get("recovery_failed") else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--build-volume", required=True)
    parser.add_argument("--reports-dir", type=Path, required=True)
    parser.add_argument("--experiment", choices=sorted(EXPERIMENTS), default="lifecycle")
    parser.add_argument("--diagnostic-startup", action="store_true")
    parser.add_argument("--series", choices=("A", "B"))
    parser.add_argument("--heap-build-volume")
    parser.add_argument("--generated-build-volume")
    parser.add_argument("--launcher-build-volume")
    parser.add_argument("--deadline-case", choices=("acknowledgement", "confirmation"))
    args = parser.parse_args()
    try:
        return run(args)
    except (RunnerError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(str(error) if isinstance(error, RunnerError) else type(error).__name__, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
