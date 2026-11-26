"""Guard metadata-only evidence of the attended unlock input experiment."""
import copy
import json
import unittest

import unlock_input_probe as probe


class UnlockInputTests(unittest.TestCase):
    def evidence(self, *, accepted=True, terminal=True, prompts=1, elapsed=30):
        return dict(accepted=accepted, exit_code=0 if accepted else 1,
                    prompts_observed=prompts, terminal_restored=terminal,
                    echo_disabled=True if terminal is not None else None,
                    foreground_owned=True if terminal is not None else None,
                    input_echo_detected=False, process_exit_observed=True,
                    pidfd_exit_observed=True, helper_unchanged=True,
                    state_unchanged=True, elapsed_ms=elapsed,
                    credential_unlocked=False, session_activated=False)

    def test_accepts_only_exact_metadata_for_attended_input(self):
        evidence = self.evidence()
        probe.validate_case("valid", evidence)
        for field, value in (("credential_unlocked", True), ("session_activated", True),
                             ("input_echo_detected", True), ("accepted", 1),
                             ("prompts_observed", 2), ("terminal_restored", False),
                             ("echo_disabled", False), ("foreground_owned", False),
                             ("process_exit_observed", False), ("pidfd_exit_observed", False),
                             ("helper_unchanged", False), ("state_unchanged", False),
                             ("secret", "must never be a report field")):
            changed = dict(evidence, **{field: value})
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_case("valid", changed)
        for field in evidence:
            changed = copy.deepcopy(evidence)
            del changed[field]
            with self.subTest(missing=field), self.assertRaises(probe.ProbeError):
                probe.validate_case("valid", changed)

    def test_refusals_preserve_terminal_and_never_claim_activation(self):
        refused = self.evidence(accepted=False, elapsed=15000)
        probe.validate_case("timeout", refused)
        for field, value in (("accepted", True), ("exit_code", 0),
                             ("elapsed_ms", 13000), ("terminal_restored", False),
                             ("credential_unlocked", True), ("session_activated", True)):
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_case("timeout", dict(refused, **{field: value}))
        headless = self.evidence(accepted=False, terminal=None, prompts=0)
        probe.validate_case("no-tty", headless)
        with self.assertRaises(probe.ProbeError):
            probe.validate_case("no-tty", dict(headless, prompts_observed=1))

    def test_constant_success_rejects_duplicates_types_and_extra_content(self):
        value = dict(probe.SUCCESS)
        probe.validate_success(json.dumps(value).encode("ascii"))
        for field, changed in (("version", True), ("attended_input_accepted", 1),
                               ("terminal_restored", False), ("credential_unlocked", True),
                               ("session_activated", True), ("secret", "must not be present")):
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_success(json.dumps(dict(value, **{field: changed})))
        repeated = json.dumps(value)[:-1] + ', "version": 1}'
        with self.assertRaises(probe.ProbeError):
            probe.validate_success(repeated)

    def test_complete_report_refuses_missing_cases_limits_and_forged_authority(self):
        cases = {}
        for name in probe.CASES:
            headless = name in probe.HEADLESS
            prompts = 0 if headless or name in ("background", "extra-argument") else 1
            evidence = self.evidence(accepted=name in probe.POSITIVE,
                                     terminal=None if headless or name == "terminal-hangup" else True,
                                     prompts=prompts, elapsed=15000 if name == "timeout" else 30)
            evidence["echo_disabled"] = True if prompts else None
            evidence["foreground_owned"] = False if name == "background" else True if prompts else None
            cases[name] = evidence
        report = dict(version=1, status="observed", cases=cases, helper_sha256="a" * 64,
                      source_sha256="b" * 64, resource_limits=dict(memory_bytes=134217728,
                          swap_bytes=0, pids=32, cpu_quota=100000, cpu_period=100000, temporary_bytes=16777216),
                      uid=65534, credential_unlocked=False, session_activated=False)
        probe.validate_report(report)
        for field, value in (("version", True), ("uid", True), ("status", "incomplete"),
                             ("helper_sha256", "A" * 64), ("source_sha256", "unknown"),
                             ("credential_unlocked", True), ("session_activated", True),
                             ("secret", "must not be present")):
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_report(dict(report, **{field: value}))
        for case in probe.CASES:
            changed = copy.deepcopy(report)
            del changed["cases"][case]
            with self.subTest(missing_case=case), self.assertRaises(probe.ProbeError):
                probe.validate_report(changed)
        for field, value in (("memory_bytes", 0), ("swap_bytes", 1), ("pids", 129),
                             ("cpu_quota", 100001), ("temporary_bytes", 0)):
            changed = copy.deepcopy(report)
            changed["resource_limits"][field] = value
            with self.subTest(limit=field), self.assertRaises(probe.ProbeError):
                probe.validate_report(changed)


if __name__ == "__main__":
    unittest.main()
