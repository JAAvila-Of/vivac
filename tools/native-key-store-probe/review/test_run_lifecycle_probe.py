"""Control-plane regressions; no container or native executable is started."""

import argparse
import copy
import io
import json
from pathlib import Path
import tempfile
import tarfile
import unittest
from unittest.mock import patch

import run_lifecycle_probe as runner


def concurrent_cases():
    stages = {
        "completed-before-lock": ("before-native-send", "published", 1, 1, 1, 1),
        "lock-before-native-send": ("before-native-send", "cancelled_by_lock", 0, 0, 0, 0),
        "lock-native-response-pending": ("native-response-pending", "cancelled_by_lock", 1, 0, 0, 0),
        "lock-after-native-reply": ("after-native-reply", "cancelled_by_lock", 1, 1, 0, 0),
        "lock-before-publication": ("before-publication", "cancelled_by_lock", 1, 1, 1, 0),
        "parallel-lock-callers": ("before-native-send", "cancelled_by_lock", 0, 0, 0, 0),
        "lease-change-before-publication": ("before-publication", "lease_invalidated", 1, 1, 1, 0),
    }
    return [{"case": name, "selected_seam": seam, "request_outcome": outcome,
             "native_requests_sent": sent, "native_replies_received": replies,
             "proofs_verified": verified, "proofs_published": published,
             "state": "CLOSED", "active": False, "resources_closed_after_drain": True,
             "workers_joined": True, "close_acknowledged": True, "request_late_refused": True,
             "closing_observed_while_paused": True, "drain_wait_observed": True,
             "lock_acknowledgments": 2 if name == "parallel-lock-callers" else 1}
            for name, (seam, outcome, sent, replies, verified, published) in stages.items()]


def loader_cases():
    stages = [
        ("load-ready-close", "ready", "close", "ready_closed"),
        ("cancel-at-prompt", "prompt", "cancel", "cancelled"),
        ("cancel-after-secret-handoff", "secret-handoff", "cancel", "cancelled"),
        ("cancel-before-readiness", "before-readiness", "cancel", "cancelled"),
        ("supervisor-kill-at-prompt", "prompt", "supervisor-kill", "supervisor_killed"),
        ("supervisor-kill-after-secret-handoff", "secret-handoff", "supervisor-kill", "supervisor_killed"),
        ("loader-death-at-prompt", "prompt", "loader-kill", "loader_failed_closed"),
        ("supervisor-kill-before-readiness", "before-readiness", "supervisor-kill", "supervisor_killed"),
    ]
    result = []
    for name, seam, trigger, outcome in stages:
        loaded = seam in ("ready", "before-readiness")
        crash = trigger == "supervisor-kill"
        result.append({"case": name, "selected_seam": seam, "trigger": trigger, "outcome": outcome,
                       "readiness_published": outcome == "ready_closed", "close_acknowledged": not crash,
                       "secret_writes": 0 if seam == "prompt" else 1,
                       "loader_alive_at_trigger": not loaded, "loader_suspended_at_handoff": seam != "prompt",
                       "loader_returncode": 0 if loaded else -9, "agent_returncode": -9,
                       "supervisor_returncode": -9 if crash else 0, "resources_closed_after_exit": not crash,
                       "pre_ready_signature_verified": loaded,
                       **{field: True for field in ("selected_seam_reached", "prompt_echo_disabled",
                          "pty_master_retained", "loader_exit_observed", "agent_exit_observed",
                          "supervisor_exit_observed", "retained_connection_refused",
                          "new_connection_refused", "processes_drained")}})
    return result



def rust_loader_observation():
    matrix = (
        ("normal", "ready", 1, True, True, False, False),
        ("cancel-prompt", "prompt", 0, False, False, False, False),
        ("cancel-handoff", "handoff", 1, False, False, False, False),
        ("cancel-before-ready", "before-ready", 1, False, True, False, False),
        ("owner-kill-prompt", "prompt", 0, False, False, True, False),
        ("owner-kill-handoff", "handoff", 1, False, False, True, False),
        ("owner-kill-before-ready", "before-ready", 1, False, True, True, False),
        ("loader-kill-prompt", "prompt", 0, False, False, False, True),
    )
    cases = []
    for name, seam, writes, ready, proof, owner_dead, loader_failed in matrix:
        cases.append({"case": name, "status": "observed", "selected_seam": seam,
                      "secret_writes": writes, "ready_published": ready,
                      "independent_proof_verified": proof, "close_ack": not owner_dead,
                      "owner_sigkill_observed": owner_dead, "loader_failed_observed": loader_failed,
                      "loader_stopped_observed": bool(writes),
                      **{field: True for field in ("agent_exit_observed", "loader_exit_observed",
                         "pty_master_retained_through_exit", "retained_connection_refused",
                         "new_connection_refused", "lease_revalidated", "sealed_snapshot",
                         "pty_slave_private")}})
    return {"version": 1, "experiment": "rust-loader", "status": "observed",
            "custody_accredited": False, "launcher_accredited": False,
            "full_session_accredited": False, "ssh_disconnect_tested": False,
            "loader_lifecycle_tested": True, "sentinel_survived": True,
            "resources_closed_after_exit": True, "cases": cases}


def rust_ssh_loader_observation():
    cases = []
    for mode in ("pty", "pipe"):
        for suffix, seam, disconnected in (("normal-close", "ready", False),
                                           ("disconnect-prompt", "prompt", True),
                                           ("disconnect-handoff", "handoff", True),
                                           ("disconnect-before-ready", "before-ready", True),
                                           ("disconnect-ready", "ready", True)):
            cases.append({"case": mode + "-" + suffix, "status": "observed", "selected_seam": seam,
                          "secret_writes": int(seam != "prompt"), "ready_published": seam == "ready",
                          "independent_proof_verified": seam in ("ready", "before-ready"),
                          "loader_stopped_observed": seam != "prompt", "close_ack": True,
                          "owner_sigkill_observed": False, "loader_failed_observed": False,
                          "ssh_mode": mode, "ssh_stdin_is_tty": mode == "pty",
                          "ssh_disconnect_observed": disconnected, "ssh_disconnect_cause": "eof" if disconnected else None,
                          "ssh_client_exit_code": -9 if disconnected else 0,
                          **{field: True for field in ("agent_exit_observed", "loader_exit_observed",
                             "pty_master_retained_through_exit", "retained_connection_refused",
                             "new_connection_refused", "lease_revalidated", "sealed_snapshot",
                             "pty_slave_private", "supervisor_exit_observed", "supervisor_exec_verified")}})
    return {"version": 1, "experiment": "rust-ssh-loader", "status": "observed", "cases": cases,
            "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
            "ssh_disconnect_tested": True, "loader_lifecycle_tested": True,
            "sentinel_survived": True, "resources_closed_after_exit": True}


def rust_ssh_concurrent_observation():
    from test_rust_concurrent_probe import final
    cases = []
    for mode in ("pty", "pipe"):
        for suffix, base, seam, disconnected in (
                ("normal-close", "completed-before-lock", "published", False),
                ("disconnect-before-native-send", "lock-before-native-send", "before-native-send", True),
                ("disconnect-native-response-pending", "lock-native-response-pending", "native-response-pending", True),
                ("disconnect-after-native-reply", "lock-after-native-reply", "after-native-reply", True),
                ("disconnect-before-publication", "lock-before-publication", "before-publication", True),
                ("disconnect-published", "completed-before-lock", "published", True)):
            value = final(base)
            published = seam == "published"
            value.update(case=mode + "-" + suffix, selected_seam=seam,
                         request_outcome="published" if published else "cancelled_by_ssh",
                         ssh_disconnect_observed=disconnected, ssh_disconnect_cause="eof" if disconnected else None,
                         ssh_mode=mode, ssh_stdin_is_tty=mode == "pty", ssh_client_exit_code=-9 if disconnected else 0,
                         supervisor_exit_observed=True, supervisor_exec_verified=True,
                         readiness_proof_verified=True, published_proof_verified=published,
                         lease_invalidated_observed=False, pty_master_retained_through_exit=True,
                         pty_slave_private=True, sealed_snapshot=True,
                         retained_connection_refused=True, new_connection_refused=True)
            cases.append(value)
    return {"version": 1, "experiment": "rust-ssh-concurrent", "status": "observed", "cases": cases,
            "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
            "ssh_disconnect_tested": True, "concurrent_lock_tested": True,
            "sentinel_survived": True, "resources_closed_after_exit": True}


def ssh_cases():
    result = []
    for mode in ("pty", "pipe"):
        for source in (loader_cases()[0], loader_cases()[1], loader_cases()[2], loader_cases()[0]):
            value = copy.deepcopy(source)
            disconnected = len(result) % 4 != 0
            seam = value["selected_seam"]
            value.pop("supervisor_returncode")
            value.update(case=mode + ("-disconnect-" + seam if disconnected else "-normal-close"),
                         ssh_mode=mode, trigger="ssh-client-kill" if disconnected else "close",
                         outcome="ssh_disconnected" if disconnected else "ready_closed",
                         disconnect_observed=disconnected, disconnect_cause="eof" if disconnected else None,
                         ssh_client_returncode=-9 if disconnected else 0)
            result.append(value)
    return result


def provider_observation():
    return {"status": "observed", "custody_accredited": False, "sentinel_survived": True,
            "provider_capabilities_tested": True, "provider_admitted": False,
            "os_id": "ubuntu", "os_version": "24.04", "package_name": "openssh-client",
            "package_version": "1:9.6p1-3ubuntu13.14", "package_arch": "amd64",
            "provenance_kind": "administrator-trusted-dpkg-database", "archive_signature_verified": False,
            **{key: True for key in runner.PROVIDER_CHECKS},
            "components": [{"name": name, "path": "/usr/bin/" + name, "sha256": "c" * 64,
                            "root_owned": True, "nonwritable": True, "fd_retained": True,
                            "revalidated": True} for name in ("ssh", "ssh-keygen", "ssh-agent", "ssh-add")],
            "cases": [{"case": name, "observed": True} for name in runner.PROVIDER_CASES]}


def kdf_observation(series="A"):
    result = {"status": "observed", "custody_accredited": False, "sentinel_survived": True,
              "series": series, "samples_per_case": 7, "rounds": 64, "kdf_measurements_tested": True,
              "kdf_policy_adopted": False, "performance_admitted": False,
              "parser_auxiliary_heap_measured": False, "p99_estimated": False,
              "child_maxrss_kib": 16384,
              **{key: True for key in ("all_proofs_verified", "profile_refusals_before_native",
                 "native_exits_observed", "retained_resources_closed_after_exit", "snapshots_revalidated")},
              "cases": []}
    for name, target in runner.KDF_TARGETS.items():
        case = {"case": name, "samples_ms": [1, 2, 3, 4, 5, 6, 7], "nominal_ms": target,
                "tolerance_ms": target * 11 / 10, "classification": "nominal"}
        if name.startswith("reject-"):
            case.update(structural_exit_codes=[1] * 7, native_dispatches=0,
                        control_package_accepted=True, mutation_rehashed=True)
        result["cases"].append(case)
    result["raw_attempts"] = kdf_attempts(result)
    return result


def kdf_attempts(observation):
    return [{"case": case["case"], "elapsed_ms": sample, "returned": True}
            for case in observation["cases"] for sample in case["samples_ms"]]


def heap_observation():
    def measurement(accepted):
        return {"version": 1, "measurement": "additional-live-rust-requested-heap", "accepted": accepted,
                "baseline_bytes": 100, "peak_additional_bytes": 1024, "live_after_bytes": 100,
                "budget_bytes": 2097152, "within_budget": True, "allocator_valid": True,
                "realloc_conservative": True}
    return {"status": "observed", "custody_accredited": False, "kdf_policy_adopted": False,
            "performance_admitted": False, "heap_measurements_tested": True,
            "parser_auxiliary_heap_measured": True, "measurement_kind": "additional-live-rust-requested-heap",
            "budget_bytes": 2097152, "samples_per_case": 3, "control_kind": "valid-package-check",
            **{key: True for key in ("all_measurements_valid", "fixtures_rehashed", "checker_equivalence",
               "resources_closed_after_measurement", "retained_control_accepted")},
            "retained_control_observation": measurement(True),
            "cases": [{"case": name, "expected_accepted": accepted,
                       "baseline_checker_exit_code": 0 if accepted else 1,
                       "observations": [measurement(accepted) for _ in range(3)],
                       "peak_additional_bytes": 1024, "mutation_rehashed": name != "valid-package"}
                      for name, accepted in runner.HEAP_EXPECTED.items()]}


def generated_observation(deadline_case=None, failure_matrix=False, publication_errors=False):
    result = {"version": 1, "experiment": "generated", "status": "observed",
              "control_kind": "valid-package-check", "cases": [], "provenance": {}, "resource_limits": {},
              "helper_sha256": {"writer": "a" * 64, "verifier": "b" * 64},
              "source_sha256": {"generated_probe": "c" * 64, "provider_probe": "d" * 64}}
    for field in ("complete", "resources_closed_after_measurement", "retained_control_accepted",
                  "native_public_extraction_matches", "native_public_extraction_tested"):
        result[field] = True
    for field in ("custody_accredited", "performance_admitted", "kdf_policy_adopted", "native_unlock_tested",
                  "production_launcher_tested", "deadline_expiry_tested", "agent_load_tested"):
        result[field] = False
    if deadline_case:
        result.update(experiment="generated-deadline", deadline_case=deadline_case, deadline_expiry_tested=True)
    names = {"valid-generated-package", "deadline-" + deadline_case} if deadline_case else runner.GENERATED_CASES
    if failure_matrix:
        names = runner.GENERATED_FAILURE_CASES
        result.update(experiment="generated-failures", failure_seams_tested=True,
                      terminal_replacement_tested=False, fsync_failure_tested=False)
    if publication_errors:
        names = runner.GENERATED_ERROR_CASES
        result.update(experiment="generated-publication-errors", publication_error_returns_tested=True,
                      terminal_replacement_tested=False, fsync_failure_tested=False)
    for name in sorted(names):
        headless = name in ("no-tty", "unset-disposable")
        disclosed = name not in runner.GENERATED_NO_DISCLOSURE and name not in (
            "deadline-acknowledgement", "terminal-drift-acknowledgement")
        positive = name == "valid-generated-package"
        case = {"case": name, "status": "observed", "exit_code": 0 if positive else 1,
                "package_created": positive, "standard_streams_clean": True,
                "echo_disabled": None if headless else True, "foreground": None if headless else True,
                "disclosures": int(disclosed), "process_metadata_checked": disclosed,
                "private_arguments": True if disclosed else None,
                "terminal_restored": None if headless or name == "terminal-hangup" else True}
        if positive:
            case.update(structural_check_accepted=True, native_public_extraction_matches=True)
        if name.startswith("deadline-"):
            case["delay_elapsed_ms"] = 60010 if deadline_case == "acknowledgement" else 180010
        if name.startswith("terminal-drift-"):
            case["attrs_changed"] = True
        if name.startswith("interrupted-"):
            committed = name == "interrupted-after-publish"
            case.update(exit_code=77, package_created=committed, fault_point=name.removeprefix("interrupted-"),
                        preparations_remaining=0 if committed else 1, package_check_accepted=True if committed else None,
                        filesystem_secret_absent=True)
            if committed:
                case["native_public_extraction_matches"] = True
        if name.startswith("returned-"):
            committed = name == "returned-after-publish"
            case.update(exit_code=2 if committed else 1, package_created=committed,
                        error_point=name.removeprefix("returned-"), preparations_remaining=0,
                        package_check_accepted=True if committed else None, filesystem_secret_absent=True,
                        indeterminate_durability_reported=committed)
            if committed:
                case["native_public_extraction_matches"] = True
        result["cases"].append(case)
    return result


def rust_launcher_observation():
    report = {"version": 1, "experiment": "rust-launcher", "status": "observed", "custody_accredited": False,
              "launcher_accredited": False, "full_loader_tested": False, "threadsafe_fullflow": False,
              "secret_channel_tested": False, "resources_closed_after_exit": True, "cases": []}
    controls = {
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
    for name in sorted(runner.RUST_LAUNCHER_CASES):
        case = {"case": name, "status": "observed", **{key: True for key in controls.get(name, ())}}
        if name.startswith("owner-"):
            case.update(child_exit_observed=True, owner_sigkill_observed=True, observer_endpoint_retained=True,
                        native_exec_observed=False, seam="B" if name == "owner-before-arm" else "A",
                        child_signal=None if name == "owner-before-arm" else 9,
                        child_code=1 if name == "owner-before-arm" else None)
            if name == "owner-before-arm":
                case["parent_change_refusal_observed"] = True
        if name == "stable-thread":
            case.update(native_children_observed=4, caller_threads_returned=4, pressure_threads=4)
        if name == "exec-error":
            case.update(exec_failure_raw_os_error=8, exec_preexec_seams_observed=["B", "A"])
        report["cases"].append(case)
    return report


class RunnerTests(unittest.TestCase):
    def test_root_reader_preserves_guards_and_preseeds_fixtures_before_start(self):
        from test_rust_root_reader_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running":False,"ExitCode":0,"OOMKilled":False},
            experiment="rust-root-reader", launcher_build="owned-reader", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        for argument in ("/probe/rust_root_reader_probe.py", "--fixture-root", "/store/run", "--memory=512m",
                         "--cpus=1", "--pids-limit=128", "no-new-privileges", "65534:65534", "ALL", "none", "240s",
                         "type=volume,source=owned-reader,target=/launcher-build,readonly"):
            self.assertIn(argument, create)
        copy_index = next(n for n, call in enumerate(calls) if call == ["docker","cp","-",report["container_id"] + ":/store/run"])
        self.assertLess(copy_index, next(n for n, call in enumerate(calls) if call[1] == "start"))
        self.assertIn(["docker","volume","rm",report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-root-reader", launcher_build=build)

    def test_root_reader_container_zero_requires_complete_unpromoted_observations(self):
        from test_rust_root_reader_probe import observation
        for mutate in (lambda v: v.update(root_bootstrap_accredited=True),
                       lambda v: v.update(provisioning_accredited=True), lambda v: v["cases"].pop(),
                       lambda v: v["cases"][0].update(host_exit_code=1)):
            value = observation()
            mutate(value)
            result, _, report = self.simulate([[],[],[],[]], {"Running":False,"ExitCode":0,"OOMKilled":False},
                experiment="rust-root-reader", launcher_build="owned-reader", observation=value)
            self.assertEqual(result, 1)
            self.assertEqual(report["status"], "incomplete")

    def test_admission_storage_preserves_limits_and_separate_private_store(self):
        from test_rust_admission_storage_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-admission-storage", launcher_build="owned-metadata", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        for argument in ("/probe/rust_admission_storage_probe.py", "--memory=512m", "--cpus=1", "--pids-limit=128",
                         "no-new-privileges", "none", "240s",
                         "type=volume,source=owned-metadata,target=/launcher-build,readonly",
                         "type=volume,source=" + report["store_volume"] + ",target=/store"):
            self.assertIn(argument, create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-admission-storage", launcher_build=build)

    def test_admission_storage_container_success_cannot_credit_admission_or_missing_cases(self):
        from test_rust_admission_storage_probe import observation
        for mutate in (lambda value: value.update(creation_admission=True), lambda value: value["cases"].pop()):
            value = observation()
            mutate(value)
            result, _, report = self.simulate(
                [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
                experiment="rust-admission-storage", launcher_build="owned-metadata", observation=value)
            self.assertEqual(result, 1)
            self.assertEqual(report["status"], "incomplete")

    def test_rust_connected_routes_sibling_cases_with_separate_helper_and_original_limits(self):
        from test_rust_connected_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-connected", launcher_build="owned-connected", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        for argument in ("/probe/rust_connected_probe.py", "--memory=512m", "--cpus=1", "--pids-limit=128",
                         "no-new-privileges", "none", "240s",
                         "type=volume,source=owned-connected,target=/launcher-build,readonly",
                         "type=volume,source=" + report["store_volume"] + ",target=/store"):
            self.assertIn(argument, create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-connected", launcher_build=build)

    def test_rust_connected_runner_refuses_container_zero_with_missing_peer_evidence(self):
        from test_rust_connected_probe import observation
        incomplete = observation()
        incomplete["cases"][0]["client_identity_verified"] = False
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-connected", launcher_build="owned-connected", observation=incomplete)
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_rust_reservation_routes_six_cases_with_original_limits_and_private_store(self):
        from test_rust_reservation_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-reservation", launcher_build="owned-reservation", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        for argument in ("/probe/rust_reservation_probe.py", "--memory=512m", "--cpus=1", "--pids-limit=128",
                         "no-new-privileges", "--network", "none", "240s",
                         "type=volume,source=owned-reservation,target=/launcher-build,readonly",
                         "type=volume,source=" + report["store_volume"] + ",target=/store"):
            self.assertIn(argument, create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-reservation", launcher_build=build)

    def test_rust_reservation_runner_rejects_missing_observation_after_successful_container(self):
        from test_rust_reservation_probe import observation
        incomplete = observation()
        incomplete["cases"].pop()
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-reservation", launcher_build="owned-reservation", observation=incomplete)
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_rust_ipc_routes_ten_cases_with_separate_readonly_helper_and_existing_limits(self):
        from test_rust_ipc_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-ipc", launcher_build="owned-ipc", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_ipc_probe.py", create)
        self.assertIn("type=volume,source=owned-ipc,target=/launcher-build,readonly", create)
        self.assertIn("--memory=512m", create)
        self.assertIn("--cpus=1", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-ipc", launcher_build=build)

    def test_rust_ipc_runner_rejects_missing_evidence_and_excess_claims(self):
        from test_rust_ipc_probe import observation
        for field, value in (("server_sticky_refusal", False), ("injector_identity_verified", False),
                             ("injector_exit_observed", False), ("lock_ack", True), ("generation", "extra")):
            report = observation()
            next(case for case in report["cases"] if case["case"] == "forwarded-sender")[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_ipc_observation(report)

    def test_rust_isolation_uses_separate_readonly_helper_and_existing_limits(self):
        from test_rust_isolation_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-isolation", launcher_build="owned-isolation", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_isolation_probe.py", create)
        self.assertIn("type=volume,source=owned-isolation,target=/launcher-build,readonly", create)
        self.assertIn("--memory=512m", create)
        self.assertIn("--cpus=1", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-isolation", launcher_build=build)

    def test_rust_isolation_runner_requires_complete_evidence_without_recovery_claims(self):
        from test_rust_isolation_probe import observation
        for field, value in (("native_alive_while_worker_b_completed", False),
                             ("worker_a_alive_while_worker_b_completed", False),
                             ("worker_a_marker_identity_verified", False), ("same_store_recovery", True)):
            report = observation()
            report["cases"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_isolation_observation(report)

    def test_rust_abandon_uses_separate_readonly_helper_and_observed_exit_reports(self):
        from test_rust_abandon_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-abandon", launcher_build="owned-abandon", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_abandon_probe.py", create)
        self.assertIn("type=volume,source=owned-abandon,target=/launcher-build,readonly", create)
        self.assertIn("--memory=512m", create)
        self.assertIn("--cpus=1", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-abandon", launcher_build=build)

    def test_rust_abandon_runner_refuses_unacknowledged_or_fabricated_evidence(self):
        from test_rust_abandon_probe import observation
        for field, value in (("helper_responsive_after_drop", False), ("marker_identity_verified", False),
                             ("close_ack", True), ("admission_poisoned", False), ("helper_exit_code", -9)):
            report = observation()
            report["cases"][1][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_abandon_observation(report)
        incomplete = observation()
        incomplete["cases"].pop()
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-abandon", launcher_build="owned-abandon", observation=incomplete)
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_session_abandon_cannot_claim_closure_or_release_its_descriptor_lease(self):
        from test_rust_concurrent_probe import observation
        for field, value in (("close_acknowledged", True), ("resources_closed_after_drain", True),
                             ("readmission_refused", 0), ("descriptors_retained", False),
                             ("retained_descriptor_count", 4), ("helper_responsive_after_drop", False),
                             ("retained_descriptors_independently_checked", False), ("callback_uncertain", False)):
            report = observation()
            next(case for case in report["cases"] if case["case"] == "session-abandon")[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_concurrent_observation(report)

    def test_rust_ssh_concurrent_preserves_readonly_mounts_and_all_twelve_cases(self):
        observation = rust_ssh_concurrent_observation()
        runner.validate_rust_ssh_concurrent_observation(observation)
        self.assertEqual(len(observation["cases"]), 12)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-ssh-concurrent", launcher_build="owned-ssh-gate", observation=observation)
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_ssh_concurrent_probe.py", create)
        self.assertIn("type=volume,source=owned-ssh-gate,target=/launcher-build,readonly", create)
        self.assertIn("type=volume,source=owned-build,target=/build,readonly", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-ssh-concurrent", launcher_build=build)

    def test_rust_ssh_concurrent_requires_observed_cause_and_no_late_publication(self):
        mutations = (
            ("pipe-disconnect-before-native-send", "ssh_disconnect_cause", None),
            ("pty-disconnect-after-native-reply", "ssh_disconnect_cause", "supervisor-kill"),
            ("pipe-normal-close", "ssh_disconnect_observed", True),
            ("pipe-normal-close", "ssh_client_exit_code", False),
            ("pty-disconnect-native-response-pending", "native_replies_received", 1),
            ("pipe-disconnect-before-publication", "proofs_published", 1),
            ("pty-disconnect-published", "proofs_published", 0),
            ("pipe-disconnect-published", "published_proof_verified", False),
            ("pty-disconnect-before-native-send", "closing_observed_while_paused", False),
            ("pipe-disconnect-before-publication", "workers_joined", True),
            ("pipe-disconnect-after-native-reply", "supervisor_exec_verified", False),
            ("pty-normal-close", "ssh_stdin_is_tty", False),
        )
        for name, field, value in mutations:
            observation = rust_ssh_concurrent_observation()
            next(case for case in observation["cases"] if case["case"] == name)[field] = value
            with self.subTest(name=name, field=field), self.assertRaises(ValueError):
                runner.validate_rust_ssh_concurrent_observation(observation)

    def test_rust_ssh_concurrent_refuses_missing_cases_or_excess_claims(self):
        for mutation in ("missing", "duplicate", "payload", "scope"):
            observation = rust_ssh_concurrent_observation()
            if mutation == "missing":
                observation["cases"].pop()
            elif mutation == "duplicate":
                observation["cases"][-1] = copy.deepcopy(observation["cases"][0])
            elif mutation == "payload":
                observation["cases"][0]["signature"] = "public proof remains on control channel"
            else:
                observation["full_session_accredited"] = True
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                runner.validate_rust_ssh_concurrent_observation(observation)

    def test_rust_concurrent_requires_separate_readonly_helper_and_original_verifier(self):
        from test_rust_concurrent_probe import observation
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-concurrent", launcher_build="owned-gate", observation=observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_concurrent_probe.py", create)
        self.assertIn("type=volume,source=owned-gate,target=/launcher-build,readonly", create)
        self.assertIn("type=volume,source=owned-build,target=/build,readonly", create)
        self.assertEqual(create[create.index("--verifier") + 1], "/build/writer189/debug/openssh-proof-verifier-probe")
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-concurrent", launcher_build=build)

    def test_report_initialization_contains_only_private_directory_metadata(self):
        with patch.object(runner, "command") as control:
            runner.initialize_reports("c" * 64)
        self.assertEqual(control.call_args.args[0], ["docker", "cp", "--archive", "-", "c" * 64 + ":/reports"])
        with tarfile.open(fileobj=io.BytesIO(control.call_args.kwargs["payload"])) as archive:
            members = archive.getmembers()
            self.assertEqual(len(members), 1)
            self.assertEqual((members[0].name, members[0].uid, members[0].gid, members[0].mode),
                             ("run", 65534, 65534, 0o700))
            self.assertTrue(members[0].isdir())

    def test_resource_configuration_rejects_missing_or_relaxed_controls(self):
        config = {
            "Config": {"User": "65534:65534"},
            "HostConfig": {"Memory": 536870912, "MemorySwap": 536870912,
                           "NanoCpus": 1000000000, "PidsLimit": 128,
                           "ReadonlyRootfs": True, "NetworkMode": "none", "PidMode": "",
                           "Privileged": False, "CapDrop": ["ALL"],
                           "SecurityOpt": ["no-new-privileges"],
                           "LogConfig": {"Type": "json-file", "Config": {"max-size": "1m", "max-file": "1"}},
                           "Tmpfs": {"/tmp": "rw,nosuid,nodev,mode=0700,uid=65534,gid=65534,size=128m"}},
            "Mounts": [{"Destination": "/probe", "RW": False},
                       {"Destination": "/build", "RW": False, "Name": "build"},
                       {"Destination": "/reports", "RW": True, "Name": "reports"}],
        }
        runner.verify_config(config, "build", "reports")
        session = copy.deepcopy(config)
        session["Mounts"].append({"Destination": "/store", "RW": True, "Name": "store"})
        runner.verify_config(session, "build", "reports", "store")
        heap = copy.deepcopy(session)
        heap["Mounts"].append({"Destination": "/heap-build", "RW": False, "Name": "heap-build"})
        runner.verify_config(heap, "build", "reports", "store", "heap-build")
        heap["Mounts"][-1]["RW"] = True
        with self.assertRaises(runner.RunnerError):
            runner.verify_config(heap, "build", "reports", "store", "heap-build")
        with self.assertRaises(runner.RunnerError):
            runner.verify_config(session, "build", "reports", "reports")
        for field, value in (("Memory", 0), ("MemorySwap", -1), ("NanoCpus", 0),
                             ("PidsLimit", 0), ("PidMode", "host"),
                             ("ReadonlyRootfs", False), ("NetworkMode", "default")):
            with self.subTest(field=field):
                unsafe = copy.deepcopy(config)
                unsafe["HostConfig"][field] = value
                with self.assertRaises(runner.RunnerError):
                    runner.verify_config(unsafe, "build", "reports")

    def simulate(self, events, state, *, stop_fails=False, observation=None, experiment="lifecycle",
                 diagnostic=False, series=None, heap_build=None, generated_build=None, deadline_case=None,
                 launcher_build=None):
        calls = []
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        reports = Path(temporary.name) / "new-reports"
        args = argparse.Namespace(reports_dir=reports, image="local-fixture", build_volume="owned-build",
                                  experiment=experiment, diagnostic_startup=diagnostic, series=series,
                                  heap_build_volume=heap_build, generated_build_volume=generated_build,
                                  deadline_case=deadline_case, launcher_build_volume=launcher_build)

        def control(arguments, timeout=15, payload=None, merge_stderr=False):
            calls.append(arguments)
            verb = arguments[1]
            if verb == "image":
                return json.dumps("sha256:" + "a" * 64).encode()
            if verb == "create":
                return b"b" * 64
            if verb == "inspect":
                return json.dumps(state if "--format" in arguments else [{"Id": "b" * 64}]).encode()
            if verb == "stop" and stop_fails:
                raise runner.RunnerError("stop failed")
            if verb == "kill" and stop_fails:
                raise runner.RunnerError("kill failed")
            if verb == "cp" and "--archive" not in arguments and observation is not None:
                (reports / "observations.json").write_text(json.dumps(observation), encoding="utf-8")
            return b""

        with patch.object(runner.sys, "platform", "win32"), patch.dict(runner.os.environ, RUST_WORK_JOB="test-only"), \
                patch.object(runner, "command", side_effect=control), \
                patch.object(runner, "verify_config"), patch.object(runner, "rdp_events", side_effect=events):
            result = runner.run(args)
        return result, calls, json.loads((reports / "runner.json").read_text(encoding="utf-8"))

    def test_session_error_stops_only_selected_container_and_preserves_failure(self):
        result, calls, report = self.simulate([[], [], [{"id": 36}]], {"Running": True})
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")
        self.assertEqual(report["events"], [{"id": 36}])
        stops = [call for call in calls if call[1] == "stop"]
        self.assertEqual(stops, [["docker", "stop", "--time", "2", report["container_id"]]])
        self.assertTrue(any(call[1] == "cp" for call in calls))

    def test_rust_launcher_uses_a_separate_readonly_build_and_no_sentinel_claim(self):
        observation = rust_launcher_observation()
        runner.validate_rust_launcher_observation(observation)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-launcher", launcher_build="owned-launcher", observation=observation)
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("type=volume,source=owned-launcher,target=/launcher-build,readonly", create)
        self.assertIn("/launcher-build/launcher189/debug/rust-launcher-probe", create)
        self.assertEqual(report["status"], "observed")
        with self.assertRaises(runner.RunnerError):
            self.simulate([], {}, experiment="rust-launcher")

    def test_rust_launcher_requires_real_seams_exec_failure_and_unusable_agent(self):
        for name, field, value in (("exec-error", "exec_failure_raw_os_error", 13),
                                   ("owner-after-arm", "child_signal", 15),
                                   ("owner-before-arm", "child_code", 0),
                                   ("owner-before-arm", "child_code", True),
                                   ("owner-before-arm", "parent_change_refusal_observed", False),
                                   ("native-agent-owner-death", "retained_connection_refused", False),
                                   ("stable-thread", "native_children_observed", True),
                                   ("creator-thread-exit", "held_control_through_exit", False)):
            observation = rust_launcher_observation()
            next(case for case in observation["cases"] if case["case"] == name)[field] = value
            with self.subTest(name=name, field=field), self.assertRaises(ValueError):
                runner.validate_rust_launcher_observation(observation)
        for changes in ({"cases": []}, {"launcher_accredited": True}, {"version": True}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                runner.validate_rust_launcher_observation(dict(rust_launcher_observation(), **changes))

    def test_rust_loader_accepts_complete_matrix_and_separate_readonly_helper(self):
        observation = rust_loader_observation()
        runner.validate_rust_loader_observation(observation)
        self.assertEqual(len(observation["cases"]), 8)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-loader", launcher_build="owned-loader", observation=observation)
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], "observed")
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_loader_probe.py", create)
        self.assertIn("type=volume,source=owned-loader,target=/launcher-build,readonly", create)
        self.assertIn("type=volume,source=owned-build,target=/build,readonly", create)
        self.assertEqual(create[create.index("--launcher-helper") + 1],
                         "/launcher-build/launcher189/debug/rust-launcher-probe")
        self.assertEqual(create[create.index("--verifier") + 1],
                         "/build/writer189/debug/openssh-proof-verifier-probe")
        exports = [call for call in calls if call[1] == "cp" and "--archive" not in call]
        self.assertEqual([call[2] for call in exports], [report["container_id"] + ":/reports/run/."])
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-loader", launcher_build=build)

    def test_rust_loader_rejects_wrong_cause_seam_proof_and_close_ack(self):
        mutations = (
            ("owner-kill-prompt", "owner_sigkill_observed", False),
            ("cancel-prompt", "owner_sigkill_observed", True),
            ("loader-kill-prompt", "loader_failed_observed", False),
            ("cancel-prompt", "loader_failed_observed", True),
            ("cancel-handoff", "selected_seam", "prompt"),
            ("cancel-before-ready", "independent_proof_verified", False),
            ("cancel-prompt", "independent_proof_verified", True),
            ("normal", "ready_published", False),
            ("owner-kill-before-ready", "ready_published", True),
            ("owner-kill-handoff", "close_ack", True),
            ("cancel-handoff", "close_ack", False),
            ("cancel-handoff", "loader_stopped_observed", False),
        )
        for name, field, value in mutations:
            with self.subTest(name=name, field=field):
                observation = rust_loader_observation()
                next(case for case in observation["cases"] if case["case"] == name)[field] = value
                with self.assertRaises(ValueError):
                    runner.validate_rust_loader_observation(observation)

    def test_rust_loader_requires_exact_secret_count_and_private_retained_pty(self):
        for name, field, value in (
                ("cancel-prompt", "secret_writes", 1),
                ("cancel-handoff", "secret_writes", 0),
                ("normal", "secret_writes", 2),
                ("normal", "secret_writes", True),
                ("cancel-prompt", "secret_writes", False),
                ("normal", "pty_slave_private", False),
                ("cancel-prompt", "pty_master_retained_through_exit", False),
                ("owner-kill-handoff", "agent_exit_observed", False),
                ("loader-kill-prompt", "loader_exit_observed", False)):
            with self.subTest(name=name, field=field, value=value):
                observation = rust_loader_observation()
                next(case for case in observation["cases"] if case["case"] == name)[field] = value
                with self.assertRaises(ValueError):
                    runner.validate_rust_loader_observation(observation)

    def test_rust_loader_rejects_missing_duplicate_or_foreign_case(self):
        for mutation in ("missing", "duplicate", "foreign"):
            with self.subTest(mutation=mutation):
                observation = rust_loader_observation()
                if mutation == "missing":
                    observation["cases"].pop()
                elif mutation == "duplicate":
                    observation["cases"][-1] = copy.deepcopy(observation["cases"][0])
                else:
                    observation["cases"][-1]["case"] = "foreign-case"
                with self.assertRaises(ValueError):
                    runner.validate_rust_loader_observation(observation)
        observation = rust_loader_observation()
        observation["cases"][0]["signature"] = "public-proof-must-stay-on-control-channel"
        with self.assertRaises(ValueError):
            runner.validate_rust_loader_observation(observation)

    def test_rust_ssh_loader_runs_ten_cases_with_separate_readonly_helper(self):
        observation = rust_ssh_loader_observation()
        runner.validate_rust_ssh_loader_observation(observation)
        self.assertEqual(len(observation["cases"]), 10)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="rust-ssh-loader", launcher_build="owned-ssh-loader", observation=observation)
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], "observed")
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/rust_ssh_loader_probe.py", create)
        self.assertIn("type=volume,source=owned-ssh-loader,target=/launcher-build,readonly", create)
        self.assertIn("type=volume,source=owned-build,target=/build,readonly", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="rust-ssh-loader", launcher_build=build)

    def test_rust_ssh_loader_requires_transport_cause_and_real_remote_exec(self):
        for name, field, value in (
                ("pipe-disconnect-prompt", "ssh_disconnect_cause", None),
                ("pty-disconnect-handoff", "ssh_disconnect_cause", "supervisor-kill"),
                ("pipe-normal-close", "ssh_disconnect_cause", "eof"),
                ("pty-disconnect-ready", "ssh_disconnect_observed", False),
                ("pipe-disconnect-prompt", "ssh_client_exit_code", 0),
                ("pipe-normal-close", "ssh_client_exit_code", False),
                ("pty-normal-close", "supervisor_exec_verified", False),
                ("pipe-disconnect-ready", "supervisor_exit_observed", False),
                ("pty-disconnect-prompt", "ssh_stdin_is_tty", False)):
            with self.subTest(name=name, field=field):
                observation = rust_ssh_loader_observation()
                next(case for case in observation["cases"] if case["case"] == name)[field] = value
                with self.assertRaises(ValueError):
                    runner.validate_rust_ssh_loader_observation(observation)

    def test_rust_ssh_loader_preserves_handoff_proof_and_closure_boundaries(self):
        for name, field, value in (
                ("pipe-disconnect-prompt", "secret_writes", 1),
                ("pty-disconnect-handoff", "secret_writes", True),
                ("pipe-disconnect-before-ready", "ready_published", True),
                ("pipe-disconnect-before-ready", "independent_proof_verified", False),
                ("pty-disconnect-ready", "close_ack", False),
                ("pty-disconnect-ready", "owner_sigkill_observed", True),
                ("pipe-normal-close", "pty_slave_private", False),
                ("pipe-disconnect-handoff", "loader_stopped_observed", False),
                ("pty-disconnect-prompt", "pty_master_retained_through_exit", False)):
            with self.subTest(name=name, field=field):
                observation = rust_ssh_loader_observation()
                next(case for case in observation["cases"] if case["case"] == name)[field] = value
                with self.assertRaises(ValueError):
                    runner.validate_rust_ssh_loader_observation(observation)

    def test_rust_ssh_loader_rejects_missing_cases_extra_payload_and_product_claims(self):
        for field in ("custody_accredited", "launcher_accredited", "full_session_accredited"):
            observation = rust_ssh_loader_observation()
            observation[field] = True
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_ssh_loader_observation(observation)
        for mutation in ("missing", "duplicate", "payload"):
            observation = rust_ssh_loader_observation()
            if mutation == "missing":
                observation["cases"].pop()
            elif mutation == "duplicate":
                observation["cases"][-1] = copy.deepcopy(observation["cases"][0])
            else:
                observation["cases"][0]["signature"] = "public proof stays on control channel"
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                runner.validate_rust_ssh_loader_observation(observation)

    def test_generated_mounts_new_writer_readonly_and_keeps_original_verifier(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="generated", generated_build="owned-generated", observation=generated_observation())
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/generated_probe.py", create)
        self.assertIn("type=volume,source=owned-generated,target=/generated-build,readonly", create)
        self.assertIn("/generated-build/writer189/debug/ssh-writer-graph-audit", create)
        self.assertIn("/build/writer189/debug/openssh-proof-verifier-probe", create)
        self.assertEqual(report["status"], "observed")

    def test_generated_requires_a_distinct_explicit_build_volume(self):
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="generated", generated_build=build)
        with self.assertRaises(runner.RunnerError):
            self.simulate([], {}, generated_build="owned-generated")

    def test_generated_rejects_false_controls_and_unmeasured_admission_claims(self):
        for field in ("native_public_extraction_matches", "native_public_extraction_tested",
                      "retained_control_accepted", "resources_closed_after_measurement", "complete",
                      "deadline_expiry_tested", "production_launcher_tested", "agent_load_tested",
                      "native_unlock_tested", "performance_admitted", "kdf_policy_adopted", "custody_accredited"):
            observation = generated_observation()
            observation[field] = not observation[field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_generated_observation(observation)

    def test_generated_cannot_hide_leaks_or_pretend_headless_terminal_observation(self):
        for name, field, value in (("valid-generated-package", "standard_streams_clean", False),
                                   ("valid-generated-package", "disclosures", True),
                                   ("wrong-ack", "disclosures", 1),
                                   ("no-tty", "foreground", True),
                                   ("mismatch", "package_created", True),
                                   ("term-confirmation", "process_metadata_checked", False),
                                   ("terminal-hangup", "terminal_restored", True),
                                   ("valid-generated-package", "unexpected_payload", "public fixture")):
            observation = generated_observation()
            case = next(item for item in observation["cases"] if item["case"] == name)
            case[field] = value
            with self.subTest(name=name, field=field), self.assertRaises(ValueError):
                runner.validate_generated_observation(observation)

    def test_generated_rejects_unknown_top_level_payload_or_unidentified_helpers(self):
        for field, value in (("unexpected_payload", "public fixture"), ("status", "incomplete"),
                             ("helper_sha256", {"writer": "a" * 64}),
                             ("source_sha256", {"generated_probe": "not-a-digest", "provider_probe": "d" * 64})):
            observation = generated_observation()
            observation[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_generated_observation(observation)

    def test_real_generated_deadline_requires_its_own_matrix_and_selection(self):
        for selection in ("acknowledgement", "confirmation"):
            with self.subTest(selection=selection):
                observation = generated_observation(selection)
                runner.validate_generated_observation(observation, selection)
                result, calls, report = self.simulate(
                    [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
                    experiment="generated-deadline", generated_build="owned-generated",
                    deadline_case=selection, observation=observation)
                self.assertEqual(result, 0)
                create = next(call for call in calls if call[1] == "create")
                self.assertIn("--deadline-case", create)
                self.assertIn(selection, create)
                self.assertEqual(report["status"], "observed")
        with self.assertRaises(runner.RunnerError):
            self.simulate([], {}, experiment="generated-deadline", generated_build="owned-generated")
        with self.assertRaises(runner.RunnerError):
            self.simulate([], {}, experiment="generated", generated_build="owned-generated", deadline_case="confirmation")

    def test_real_generated_deadline_rejects_early_or_nonfinite_exit_observations(self):
        for selection, elapsed in (("acknowledgement", 58000), ("acknowledgement", 80001),
                                    ("confirmation", 178000), ("confirmation", 200001),
                                    ("confirmation", float("nan")), ("confirmation", True)):
            observation = generated_observation(selection)
            next(case for case in observation["cases"] if case["case"].startswith("deadline-"))["delay_elapsed_ms"] = elapsed
            with self.subTest(selection=selection, elapsed=elapsed), self.assertRaises(ValueError):
                runner.validate_generated_observation(observation, selection)

    def test_generated_failures_select_their_own_matrix_without_claiming_fsync_faults(self):
        observation = generated_observation(failure_matrix=True)
        runner.validate_generated_observation(observation, failure_matrix=True)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="generated-failures", generated_build="owned-generated", observation=observation)
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("--failure-matrix", create)
        self.assertNotIn("--deadline-case", create)
        self.assertEqual(report["status"], "observed")
        for field in ("terminal_replacement_tested", "fsync_failure_tested"):
            changed = dict(observation, **{field: True})
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_generated_observation(changed, failure_matrix=True)

    def test_generated_publication_interruptions_preserve_the_commit_boundary(self):
        for name, field, value in (("interrupted-before-publish", "package_created", True),
                                   ("interrupted-after-publish", "package_created", False),
                                   ("interrupted-after-publish", "exit_code", 1),
                                   ("interrupted-after-publish", "preparations_remaining", True),
                                   ("interrupted-after-publish", "package_check_accepted", None),
                                   ("interrupted-key-synced", "filesystem_secret_absent", False),
                                   ("terminal-drift-acknowledgement", "disclosures", 1),
                                   ("terminal-drift-acknowledgement", "exit_code", 77),
                                   ("terminal-drift-confirmation", "exit_code", 2),
                                   ("terminal-drift-confirmation", "attrs_changed", False)):
            observation = generated_observation(failure_matrix=True)
            next(case for case in observation["cases"] if case["case"] == name)[field] = value
            with self.subTest(name=name, field=field), self.assertRaises(ValueError):
                runner.validate_generated_observation(observation, failure_matrix=True)

    def test_generated_publication_error_returns_have_an_exclusive_matrix(self):
        observation = generated_observation(publication_errors=True)
        runner.validate_generated_observation(observation, publication_errors=True)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="generated-publication-errors", generated_build="owned-generated", observation=observation)
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("--publication-errors", create)
        self.assertNotIn("--failure-matrix", create)
        self.assertEqual(report["status"], "observed")
        with self.assertRaises(ValueError):
            runner.validate_generated_observation(observation, failure_matrix=True, publication_errors=True)

    def test_generated_error_returns_cannot_confuse_cleanup_commit_or_interruption(self):
        for name, field, value in (("returned-key-synced", "preparations_remaining", 1),
                                   ("returned-manifest-synced", "package_created", True),
                                   ("returned-before-publish", "exit_code", 77),
                                   ("returned-before-publish", "exit_code", 2),
                                   ("returned-after-publish", "package_created", False),
                                   ("returned-after-publish", "exit_code", 1),
                                   ("returned-after-publish", "indeterminate_durability_reported", False),
                                   ("returned-after-publish", "package_check_accepted", False)):
            observation = generated_observation(publication_errors=True)
            next(case for case in observation["cases"] if case["case"] == name)[field] = value
            with self.subTest(name=name, field=field), self.assertRaises(ValueError):
                runner.validate_generated_observation(observation, publication_errors=True)

    def test_session_error_before_start_preserves_evidence_without_starting(self):
        result, calls, report = self.simulate([[], [{"id": 36}]], {"Running": False})
        self.assertEqual(result, 1)
        self.assertEqual(report["events"], [{"id": 36}])
        self.assertFalse(any(call[1] == "start" for call in calls))

    def test_exit_zero_without_observations_is_not_success(self):
        result, _, report = self.simulate([[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False})
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_oom_is_not_success(self):
        result, _, report = self.simulate([[], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": True})
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_stop_failure_retains_container_for_recovery(self):
        result, calls, report = self.simulate([[], [], [{"id": 226}]], {}, stop_fails=True)
        self.assertEqual(result, 1)
        self.assertTrue(report["cleanup_failed"])
        self.assertFalse(any(call[1] == "rm" or (call[1] == "cp" and "--archive" not in call) for call in calls))

    def test_success_requires_recovered_observations(self):
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True,
                         "cases": [{"case": name} for name in runner.CASES]},
        )
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], "observed")

    def test_session_exports_only_reports_and_removes_its_separate_store(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="session", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True,
                         "cases": [{"case": name} for name in runner.SESSION_CASES]},
        )
        self.assertEqual(result, 0)
        exports = [call for call in calls if call[1] == "cp" and "--archive" not in call]
        self.assertEqual(len(exports), 1)
        self.assertEqual(exports[0][2], report["container_id"] + ":/reports/run/.")
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)

    def test_lifecycle_report_cannot_satisfy_session_experiment(self):
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="session", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True,
                         "cases": [{"case": name} for name in runner.CASES]},
        )
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_concurrent_experiment_requires_its_own_matrix_and_store(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="concurrent", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True, "concurrent_lock_tested": True,
                         "cases": concurrent_cases()},
        )
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/concurrent_probe.py", create)
        self.assertIn("--store-root", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        result, _, _ = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="concurrent", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True,
                         "cases": [{"case": name} for name in runner.SESSION_CASES]},
        )
        self.assertEqual(result, 1)

    def test_case_names_without_concurrent_observation_do_not_pass(self):
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="concurrent", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True, "concurrent_lock_tested": False,
                         "cases": [{"case": name} for name in runner.CONCURRENT_CASES]},
        )
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_concurrent_report_rejects_wrong_reason_or_unfinished_work(self):
        for field, value in (("request_outcome", "published"), ("selected_seam", "wrong-seam"),
                             ("workers_joined", False), ("active", True),
                             ("resources_closed_after_drain", False), ("drain_wait_observed", False)):
            with self.subTest(field=field):
                cases = concurrent_cases()
                cases[1][field] = value
                with self.assertRaises(ValueError):
                    runner.validate_concurrent_observation({"concurrent_lock_tested": True, "cases": cases})

    def test_loader_experiment_exports_only_metadata_and_removes_store(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="loader", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True, "loader_lifecycle_tested": True,
                         "cases": loader_cases()},
        )
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/loader_probe.py", create)
        exports = [call for call in calls if call[1] == "cp" and "--archive" not in call]
        self.assertEqual([call[2] for call in exports], [report["container_id"] + ":/reports/run/."])
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)

    def test_loader_report_cannot_substitute_hup_or_unobserved_exit(self):
        for field, value in (("loader_returncode", -1), ("pty_master_retained", False),
                             ("close_acknowledged", True), ("readiness_published", True),
                             ("agent_exit_observed", False), ("outcome", "cancelled")):
            with self.subTest(field=field):
                cases = loader_cases()
                cases[4][field] = value
                with self.assertRaises(ValueError):
                    runner.validate_loader_observation({"loader_lifecycle_tested": True, "cases": cases})

    def test_startup_diagnostic_can_never_accredit_a_matrix(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="loader", diagnostic=True,
            observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True, "loader_lifecycle_tested": True, "cases": loader_cases()},
        )
        self.assertEqual(result, 1)
        self.assertTrue(report["diagnostic_only"])
        self.assertEqual(report["status"], "incomplete")
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("--diagnostic-startup", create)

    def test_ssh_requires_real_transport_evidence_and_observed_native_exits(self):
        for field, value in (("disconnect_observed", False), ("disconnect_cause", "control-cancel"),
                             ("ssh_client_returncode", 0), ("supervisor_exit_observed", False),
                             ("loader_returncode", -1), ("pty_master_retained", False),
                             ("resources_closed_after_exit", False), ("selected_seam_reached", False)):
            with self.subTest(field=field):
                cases = ssh_cases()
                cases[1][field] = value
                with self.assertRaises(ValueError):
                    runner.validate_ssh_observation({"ssh_disconnect_tested": True, "cases": cases})

    def test_ssh_routes_to_own_script_and_exports_no_store_bytes(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="ssh", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True, "ssh_disconnect_tested": True, "cases": ssh_cases()},
        )
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/ssh_lifecycle_probe.py", create)
        exports = [call for call in calls if call[1] == "cp" and "--archive" not in call]
        self.assertEqual([call[2] for call in exports], [report["container_id"] + ":/reports/run/."])
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)

    def test_old_loader_matrix_cannot_accredit_ssh(self):
        result, _, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="ssh", observation={"status": "observed", "custody_accredited": False,
                         "sentinel_survived": True, "ssh_disconnect_tested": True, "cases": loader_cases()},
        )
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], "incomplete")

    def test_provider_report_cannot_claim_other_system_or_vendor_attestation(self):
        for key, value in (("os_id", "debian"), ("os_version", "26.04"),
                           ("provider_admitted", True), ("archive_signature_verified", True),
                           ("provenance_kind", "vendor-attested"), ("system_package_md5_matched", False),
                           ("snapshots_revalidated", False), ("package_version", "arbitrary\ntext")):
            with self.subTest(key=key):
                observation = provider_observation()
                observation[key] = value
                with self.assertRaises(ValueError):
                    runner.validate_provider_observation(observation)

    def test_provider_requires_exact_retained_objects_and_actual_case_observations(self):
        for key, value in (("path", "/tmp/ssh"), ("sha256", "not-a-digest"),
                           ("root_owned", False), ("fd_retained", False), ("revalidated", False)):
            with self.subTest(key=key):
                observation = provider_observation()
                observation["components"][0][key] = value
                with self.assertRaises(ValueError):
                    runner.validate_provider_observation(observation)
        observation = provider_observation()
        observation["cases"][0]["observed"] = False
        with self.assertRaises(ValueError):
            runner.validate_provider_observation(observation)

    def test_provider_exports_metadata_without_a_persistent_package_store(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="provider", observation=provider_observation(),
        )
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/provider_probe.py", create)
        self.assertNotIn("--store-root", create)
        self.assertNotIn("store_volume", report)
        exports = [call for call in calls if call[1] == "cp" and "--archive" not in call]
        self.assertEqual([call[2] for call in exports], [report["container_id"] + ":/reports/run/."])

    def test_kdf_preserves_an_above_band_sample_without_claiming_admission(self):
        observation = kdf_observation()
        observation["cases"][0].update(samples_ms=[1, 2, 3, 4, 5, 6, 2400], classification="above-band")
        observation["raw_attempts"] = kdf_attempts(observation)
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="kdf", series="A", observation=observation,
        )
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], "observed")
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("--series", create)
        self.assertIn("/probe/kdf_probe.py", create)
        self.assertIn("--store-root", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)

    def test_kdf_refuses_dropped_invalid_samples_or_hidden_outliers(self):
        for values, classification in (([1] * 6, "nominal"), ([True] * 7, "nominal"),
                                       ([float("nan")] * 7, "nominal"), ([-1] * 7, "nominal"),
                                       ([1, 2, 3, 4, 5, 6, 2400], "nominal")):
            with self.subTest(values=values):
                observation = kdf_observation()
                observation["cases"][0].update(samples_ms=values, classification=classification)
                with self.assertRaises(ValueError):
                    runner.validate_kdf_observation(observation, "A")

    def test_kdf_cannot_substitute_series_or_claim_heap_or_security_admission(self):
        for key, value in (("series", "B"), ("kdf_policy_adopted", True),
                           ("performance_admitted", True), ("parser_auxiliary_heap_measured", True),
                           ("p99_estimated", True), ("rounds", 65)):
            with self.subTest(key=key):
                observation = kdf_observation()
                observation[key] = value
                with self.assertRaises(ValueError):
                    runner.validate_kdf_observation(observation, "A")

    def test_kdf_requires_rehashed_negative_control_and_zero_native_dispatch(self):
        for key, value in (("structural_exit_codes", [0] * 7), ("structural_exit_codes", [-9] * 7),
                           ("native_dispatches", 1), ("control_package_accepted", False),
                           ("mutation_rehashed", False)):
            with self.subTest(key=key):
                observation = kdf_observation()
                case = next(value for value in observation["cases"] if value["case"].startswith("reject-"))
                case[key] = value
                with self.assertRaises(ValueError):
                    runner.validate_kdf_observation(observation, "A")

    def test_kdf_attempt_ledger_cannot_drop_a_sample_or_turn_failure_into_success(self):
        observation = kdf_observation()
        observation["raw_attempts"].pop()
        with self.assertRaises(ValueError):
            runner.validate_kdf_observation(observation, "A")
        for field, value in (("returned", False), ("elapsed_ms", 123), ("case", "another-case")):
            with self.subTest(field=field):
                observation = kdf_observation()
                observation["raw_attempts"][0][field] = value
                with self.assertRaises(ValueError):
                    runner.validate_kdf_observation(observation, "A")

    def test_heap_uses_separate_readonly_build_and_final_package_control(self):
        result, calls, report = self.simulate(
            [[], [], [], []], {"Running": False, "ExitCode": 0, "OOMKilled": False},
            experiment="heap", heap_build="owned-heap-build", observation=heap_observation(),
        )
        self.assertEqual(result, 0)
        create = next(call for call in calls if call[1] == "create")
        self.assertIn("/probe/heap_probe.py", create)
        self.assertIn("type=volume,source=owned-heap-build,target=/heap-build,readonly", create)
        self.assertIn("--heap-helper", create)
        self.assertIn(["docker", "volume", "rm", report["store_volume"]], calls)
        for build in (None, "owned-build"):
            with self.subTest(build=build), self.assertRaises(runner.RunnerError):
                self.simulate([], {}, experiment="heap", heap_build=build)

    def test_heap_enforces_hard_memory_bound_and_rejects_counter_anomalies(self):
        for field, value in (("peak_additional_bytes", 2097153), ("allocator_valid", False),
                             ("realloc_conservative", False), ("accepted", False),
                             ("baseline_bytes", True), ("live_after_bytes", 9999999)):
            with self.subTest(field=field):
                observation = heap_observation()
                observation["cases"][0]["observations"][0][field] = value
                with self.assertRaises(ValueError):
                    runner.validate_heap_observation(observation)

    def test_heap_cannot_hide_a_peak_or_replace_its_final_control(self):
        observation = heap_observation()
        observation["cases"][0]["observations"][0]["peak_additional_bytes"] = 2048
        with self.assertRaises(ValueError):
            runner.validate_heap_observation(observation)
        observation = heap_observation()
        observation["retained_control_observation"]["accepted"] = False
        with self.assertRaises(ValueError):
            runner.validate_heap_observation(observation)


if __name__ == "__main__":
    unittest.main()
