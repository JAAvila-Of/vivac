"""Pure validation of bounded abandonment evidence."""
import copy
import json
import unittest

import rust_abandon_probe as probe


def helper(case):
    admission = case == "admission-limit"
    failed = case == "pidfd-failure"
    return {"version": 1, "case": case, "status": "observed", "custody_accredited": False,
            "product_recovery": False, "close_ack": False, "owners_reserved": 4 if admission else 1,
            "fifth_owner_errno": 16 if admission else None, "readmission_after_release": admission,
            "admission_poisoned": not admission, "poison_refusals": 0 if admission else 3,
            "spawn_errno": 24 if failed else None, "shutdown_errno": 110 if failed else None,
            "drop_elapsed_ns": 0 if admission else 12345, "marker_retained": None if admission else failed,
            "marker_fd": None if admission else 11, "marker_dev": None if admission else 2,
            "marker_ino": None if admission else 30, "child_pid": None if admission else 99}


def observation():
    cases = []
    for name in ("admission-limit", "pidfd-failure", "owner-abandon"):
        value = helper(name)
        admission = name == "admission-limit"
        failed = name == "pidfd-failure"
        value.update(drop_response_observed=not admission, helper_responsive_after_drop=not admission,
                     marker_identity_verified=failed, child_alive_after_drop=failed,
                     normal_exit_commanded=True, helper_exit_code=0, child_exit_observed=not admission,
                     child_exit_signal=9 if failed else None,
                     child_status_source="subreaper" if failed else "none" if admission else "pidfd",
                     resources_closed_after_exit=True)
        cases.append(value)
    return {"version": 1, "experiment": "rust-abandon", "status": "observed",
            "custody_accredited": False, "product_recovery": False, "close_ack": False,
            "resources_closed_after_exit": True, "cases": cases, "helper_sha256": "a" * 64,
            "observer_sha256": "b" * 64, "resource_limits": {}}


class AbandonValidationTests(unittest.TestCase):
    def test_three_cases_and_post_drop_response_are_accepted(self):
        probe.validate_observation(observation())
        for name in probe.CASES:
            payload = dict(helper(name), stage="responsive")
            self.assertEqual(probe.parse_helper(json.dumps(payload).encode(), name, "responsive"), payload)

    def test_pidfd_failure_cannot_claim_success_without_live_retention_and_response(self):
        for field, value in (("spawn_errno", 0), ("shutdown_errno", 0), ("marker_retained", False),
                             ("marker_identity_verified", False), ("child_alive_after_drop", False),
                             ("helper_responsive_after_drop", False), ("drop_response_observed", False),
                             ("normal_exit_commanded", False), ("child_exit_signal", 15),
                             ("child_status_source", "pidfd"), ("close_ack", True),
                             ("admission_poisoned", False), ("poison_refusals", 0),
                             ("admission_poisoned", 1), ("admission_poisoned", None),
                             ("helper_exit_code", -9)):
            report = observation()
            report["cases"][1][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_owner_abandon_records_both_possible_descriptor_outcomes(self):
        report = observation()
        report["cases"][2].update(marker_retained=True, marker_identity_verified=True,
                                  child_exit_signal=9, child_status_source="subreaper")
        probe.validate_observation(report)
        report["cases"][2]["marker_identity_verified"] = False
        with self.assertRaises(ValueError):
            probe.validate_observation(report)

    def test_exact_matrix_scope_types_and_schema_are_required(self):
        for mutate in ("missing", "duplicate", "payload", "recovery", "bool-count", "nan-time"):
            report = observation()
            if mutate == "missing":
                report["cases"].pop()
            elif mutate == "duplicate":
                report["cases"][-1] = copy.deepcopy(report["cases"][0])
            elif mutate == "payload":
                report["cases"][1]["content"] = "unneeded"
            elif mutate == "recovery":
                report["product_recovery"] = True
            elif mutate == "bool-count":
                report["cases"][0]["owners_reserved"] = True
            else:
                report["cases"][1]["drop_elapsed_ns"] = float("nan")
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_duplicate_json_fields_and_extra_packet_fields_are_rejected(self):
        payload = json.dumps(dict(helper("pidfd-failure"), stage="responsive"))
        with self.assertRaises(ValueError):
            probe.parse_helper((payload[:-1] + ',"close_ack":false}').encode(), "pidfd-failure", "responsive")
        with self.assertRaises(ValueError):
            probe.parse_helper(json.dumps(dict(helper("pidfd-failure"), stage="responsive", secret="extra")).encode(),
                               "pidfd-failure", "responsive")


if __name__ == "__main__":
    unittest.main()
