"""Reject incomplete process-reference transport evidence and authority claims."""
import copy
import unittest

import registration_probe as probe


class RegistrationTests(unittest.TestCase):
    def evidence(self, name):
        return dict(version=1, case=name, references_received=name in ("valid", "consumed-rejected", "owned-pair"),
                    transport_rejected=name not in ("valid", "consumed-rejected", "owned-pair"),
                    single_use_rejected=True, supervisor_exit_observed=True,
                    child_exits_observed=True, truncated_rights_closed=name == "truncated-rights-rejected",
                    closure_authority=False, session_activated=False)

    def report(self):
        return dict(version=1, status="observed", cases={name: self.evidence(name) for name in probe.CASES},
                    helper_sha256="a" * 64, source_sha256="b" * 64, uid=65534,
                    closure_authority=False, session_activated=False,
                    resource_limits=dict(memory_bytes=134217728, swap_bytes=0, pids=32,
                                         cpu_quota=100000, cpu_period=100000, temporary_bytes=16777216))

    def test_each_case_requires_its_actual_transport_outcome_and_cleanup(self):
        for name in probe.CASES:
            evidence = self.evidence(name)
            probe.validate_case(name, evidence)
            for field in probe.FIELDS - {"version", "case"}:
                with self.subTest(name=name, field=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, dict(evidence, **{field: not evidence[field]}))
                with self.subTest(name=name, numeric=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, dict(evidence, **{field: int(evidence[field])}))
            for field in evidence:
                changed = copy.deepcopy(evidence)
                del changed[field]
                with self.subTest(name=name, missing=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, changed)

    def test_report_requires_all_cases_and_bounded_provenance(self):
        report = self.report()
        probe.validate_report(report)
        for name in probe.CASES:
            changed = copy.deepcopy(report)
            del changed["cases"][name]
            with self.subTest(missing=name), self.assertRaises(probe.ProbeError):
                probe.validate_report(changed)

    def test_case_set_keeps_the_original_cases_and_requires_owned_roster_cases(self):
        original = {"valid", "duplicate-token-rejected", "nonpidfd-rejected", "missing-reference-rejected",
                    "extra-reference-rejected", "wrong-challenge-rejected", "forwarded-sender-rejected",
                    "timeout", "consumed-rejected", "truncated-rights-rejected"}
        owned = {"owned-pair", "owned-extra-child", "owned-spawn-after-drop", "owned-dead-child",
                 "owned-repeated-capture"}
        self.assertEqual(len(probe.CASES), 15)
        self.assertEqual(set(probe.CASES), original | owned)
        for name in owned:
            report = self.report()
            report["cases"]["valid"] = report["cases"].pop(name)
            with self.subTest(replaced=name), self.assertRaises(probe.ProbeError):
                probe.validate_report(report)

    def test_owned_cases_reject_unknown_fields_and_substituted_case_identity(self):
        for name in ("owned-pair", "owned-extra-child", "owned-spawn-after-drop", "owned-dead-child",
                     "owned-repeated-capture"):
            evidence = self.evidence(name)
            for replacement in (dict(evidence, extra=True), dict(evidence, case="valid"),
                                dict(evidence, version=True)):
                with self.subTest(name=name, replacement=replacement), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, replacement)
        report = self.report()
        report["cases"]["owned-unknown"] = self.evidence("owned-pair")
        with self.assertRaises(probe.ProbeError):
            probe.validate_report(report)

    def test_report_rejects_invalid_identity_provenance_and_limits(self):
        report = self.report()
        for field, value in (("version", True), ("uid", True), ("status", "incomplete"),
                             ("helper_sha256", "A" * 64), ("source_sha256", "unknown"),
                             ("closure_authority", True), ("session_activated", True), ("extra", False)):
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_report(dict(report, **{field: value}))
        for field, value in (("memory_bytes", 0), ("swap_bytes", 1), ("pids", 129),
                             ("cpu_quota", 100001), ("temporary_bytes", 0)):
            changed = copy.deepcopy(report)
            changed["resource_limits"][field] = value
            with self.subTest(limit=field), self.assertRaises(probe.ProbeError):
                probe.validate_report(changed)

    def test_decoder_rejects_duplicate_fields_and_trailing_records(self):
        for payload in (b'{"version":1,"version":1}\n', b'{}\n{}\n', b'{}', b'{}\n' + b'x' * 8192):
            with self.subTest(payload_size=len(payload)), self.assertRaises((probe.ProbeError, ValueError)):
                probe.decode(payload)


if __name__ == "__main__":
    unittest.main()
