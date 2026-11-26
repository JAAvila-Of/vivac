"""Pure validation tests for bounded cooperative reservation observations."""
import copy
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import rust_reservation_probe as probe
import run_lifecycle_probe as runner


def final(case):
    native = case in ("abandonment-retains", "death-is-not-exit")
    death = case == "death-is-not-exit"
    result = dict.fromkeys(probe.SCOPE_FIELDS, False)
    result.update(version=1, case=case, status="observed", host_pid=10, creator_tid=10,
                  worker_pid=11, worker_token=1, native_pid=12 if native else None,
                  native_token=1 if case == "abandonment-retains" else None,
                  same_ofd_contention_refused=True, different_store_available=True,
                  readmitted_before_worker_exit=case == "normal-release", readmitted_after_worker_death=death,
                  reservation_retained_after_action=not (death or case == "normal-release"),
                  lease_invalidated=case == "invalidation-retains", replacement_refused=case == "replacement-binding",
                  original_valid=case == "replacement-binding", admission_poisoned=case == "abandonment-retains",
                  drop_elapsed_ns=100 if case == "abandonment-retains" else 0,
                  worker_exit_code=None if death else 0, worker_exit_signal=9 if death else None,
                  worker_exit_observed=True, pressure_threads=4, pressure_iterations=1,
                  resources_closed_after_worker_exit=True)
    return result


def observation():
    report = dict.fromkeys(probe.SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-reservation", status="observed", cases=[],
                  resource_limits={}, helper_sha256="a" * 64, observer_sha256="b" * 64,
                  resources_closed_after_observed_exit=True)
    for case in sorted(probe.CASES):
        native = case in ("abandonment-retains", "death-is-not-exit")
        value = final(case)
        value.update(process_identities_verified=True, worker_alive_after_action=case != "death-is-not-exit",
                     native_alive_after_action=native, native_exit_observed=native,
                     native_exit_signal=9 if native else None, native_status_source="subreaper" if native else "none",
                     host_exit_code=0, normal_worker_exit_commanded=case != "death-is-not-exit",
                     negative_control_pidfd_kill_verified=case == "death-is-not-exit",
                     resources_closed_after_observed_exit=True)
        report["cases"].append(value)
    return report


class ReservationValidationTests(unittest.TestCase):
    def test_all_six_cases_require_exact_evidence(self):
        report = observation()
        probe.validate_observation(report)
        runner.validate_rust_reservation_observation(report)
        self.assertEqual(runner.EXPERIMENTS["rust-reservation"], ("rust_reservation_probe.py", probe.CASES))
        for case in probe.CASES:
            value = final(case)
            self.assertEqual(probe.parse_final(json.dumps(value).encode()), value)

    def test_non_acknowledgment_is_required_at_report_and_case_levels(self):
        for field in probe.SCOPE_FIELDS:
            for at_case in (False, True):
                with self.subTest(field=field, case=at_case):
                    report = observation()
                    (report["cases"][0] if at_case else report)[field] = True
                    with self.assertRaises(ValueError):
                        probe.validate_observation(report)

    def test_missing_duplicate_extra_or_unknown_case_is_incomplete(self):
        for change in (lambda cases: cases.pop(), lambda cases: cases.append(copy.deepcopy(cases[0])),
                       lambda cases: cases.__setitem__(0, copy.deepcopy(cases[1])),
                       lambda cases: cases[0].update(case="other")):
            report = observation()
            change(report["cases"])
            with self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_boolean_integer_aliases_and_unknown_fields_are_rejected(self):
        for field, value in (("version", True), ("worker_token", True), ("host_pid", True),
                             ("pressure_threads", True), ("pressure_iterations", True),
                             ("same_ofd_contention_refused", 1), ("worker_exit_code", False),
                             ("extra", True)):
            report = final("contention")
            report[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_host(report)

    def test_negative_control_requires_worker_kill_but_live_native_then_verified_kill(self):
        report = observation()
        value = next(item for item in report["cases"] if item["case"] == "death-is-not-exit")
        for field, replacement in (("native_token", 1), ("worker_exit_signal", 15),
                                   ("worker_exit_code", 0), ("native_alive_after_action", False),
                                   ("native_exit_observed", False), ("native_exit_signal", 15),
                                   ("negative_control_pidfd_kill_verified", False),
                                   ("normal_worker_exit_commanded", True)):
            changed = dict(value, **{field: replacement})
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_case(changed)

    def test_each_case_cannot_substitute_another_cases_claims(self):
        evidence = {"normal-release": "readmitted_before_worker_exit", "death-is-not-exit": "readmitted_after_worker_death",
                    "replacement-binding": "replacement_refused", "invalidation-retains": "lease_invalidated",
                    "abandonment-retains": "admission_poisoned", "contention": "same_ofd_contention_refused"}
        for case, field in evidence.items():
            value = final(case)
            value[field] = False
            with self.subTest(case=case), self.assertRaises(ValueError):
                probe.validate_host(value)
        for case in ("replacement-binding", "invalidation-retains", "abandonment-retains"):
            value = final(case)
            value["reservation_retained_after_action"] = False
            with self.assertRaises(ValueError):
                probe.validate_host(value)

    def test_intermediate_observation_cannot_claim_future_exit_or_closure(self):
        value = final("normal-release")
        with self.assertRaises(ValueError):
            probe.validate_host(value, observation=True)
        value.update(worker_exit_code=None, worker_exit_observed=False,
                     resources_closed_after_worker_exit=False, pressure_iterations=0)
        probe.validate_host(value, observation=True)
        with self.assertRaises(ValueError):
            probe.validate_host(value)

    def test_bound_identity_cannot_change_or_alias_parent(self):
        value = final("abandonment-retains")
        identity = {field: value[field] for field in probe.IDENTITY_FIELDS}
        for field, replacement in (("creator_tid", 11), ("worker_pid", 10), ("native_pid", 11),
                                   ("worker_token", 2), ("native_token", None)):
            changed = dict(value, **{field: replacement})
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_host(changed, identity)

    def test_final_cannot_hide_missing_reap_or_nonzero_host_exit(self):
        report = observation()
        for field, replacement in (("native_status_source", "pidfd"), ("host_exit_code", -9),
                                   ("worker_exit_observed", False), ("resources_closed_after_worker_exit", False),
                                   ("resources_closed_after_observed_exit", False), ("drop_elapsed_ns", 0)):
            value = copy.deepcopy(next(item for item in report["cases"] if item["case"] == "abandonment-retains"))
            value[field] = replacement
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_case(value)

    def test_replacement_mutates_only_selected_private_paths(self):
        root = Path("/private/reservation-case")
        with patch.object(probe.os, "rename") as rename, \
                patch.object(probe.os, "stat") as stat, patch.object(probe.os, "fstat") as fstat:
            stat.return_value.st_ino = fstat.return_value.st_ino = 42
            probe.mutate("replacement-binding", root, 5, 6)
            self.assertEqual(rename.call_args_list[0].args, (root / "a", root / "a-old"))
            self.assertEqual(rename.call_args_list[1].args, (root / "b", root / "a"))

    def test_public_invalidation_changes_one_byte_through_existing_inode(self):
        root = Path("/private/reservation-case")
        with patch.multiple(probe.os, O_CLOEXEC=0x80000, O_NOFOLLOW=0x20000, create=True), \
                patch.object(probe.os, "open", return_value=7) as opened, \
                patch.object(probe.os, "pwrite", return_value=1, create=True) as write, patch.object(probe.os, "close") as close:
            probe.mutate("invalidation-retains", root, 5, 6)
            self.assertEqual(opened.call_args.args[0], root / "a" / "key.openssh")
            write.assert_called_once_with(7, b"P", 0)
            close.assert_called_once_with(7)


if __name__ == "__main__":
    unittest.main()
