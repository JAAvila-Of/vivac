"""Guard independent child-exit evidence and reject forged session authority."""
import copy
import unittest

import exit_watch_probe as probe


class ExitWatchTests(unittest.TestCase):
    def evidence(self, name):
        return dict(version=1, case=name, pending_observed=True,
                    stopped_observed=name == "stopped-pending",
                    stopped_pending_observed=name == "stopped-pending",
                    exit_observed=True, reaped_before_observation=name == "exit-after-reap",
                    consumed_rejected=True, proof_revalidated=True,
                    unknown_token_refused=name == "unknown-token-refused",
                    native_exit_observed=True, closure_authority=False, session_activated=False)

    def test_only_complete_retained_exit_evidence_is_accepted(self):
        for name in probe.CASES:
            evidence = self.evidence(name)
            probe.validate_case(name, evidence)
            for field, value in (("version", True), ("case", "unknown"), ("pending_observed", False),
                                 ("exit_observed", False), ("consumed_rejected", False),
                                 ("proof_revalidated", False), ("native_exit_observed", False),
                                 ("closure_authority", True), ("session_activated", True)):
                with self.subTest(name=name, field=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, dict(evidence, **{field: value}))
            for field in evidence:
                changed = copy.deepcopy(evidence)
                del changed[field]
                with self.subTest(name=name, missing=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, changed)

    def test_stop_and_reaping_cannot_be_substituted_for_each_other(self):
        for name in probe.CASES:
            evidence = self.evidence(name)
            probe.validate_case(name, evidence)
            for field in ("stopped_observed", "stopped_pending_observed", "reaped_before_observation",
                          "unknown_token_refused"):
                with self.subTest(name=name, field=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, dict(evidence, **{field: not evidence[field]}))
                with self.subTest(name=name, numeric=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(name, dict(evidence, **{field: int(evidence[field])}))

    def test_complete_report_preserves_all_cases_and_resource_limits(self):
        report = dict(version=1, status="observed", cases={name: self.evidence(name) for name in probe.CASES},
                      helper_sha256="a" * 64, source_sha256="b" * 64, uid=65534,
                      closure_authority=False, session_activated=False,
                      resource_limits=dict(memory_bytes=134217728, swap_bytes=0, pids=32,
                                           cpu_quota=100000, cpu_period=100000, temporary_bytes=16777216))
        probe.validate_report(report)
        for name in probe.CASES:
            changed = copy.deepcopy(report)
            del changed["cases"][name]
            with self.subTest(missing=name), self.assertRaises(probe.ProbeError):
                probe.validate_report(changed)
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


if __name__ == "__main__":
    unittest.main()
