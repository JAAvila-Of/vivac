"""Pure control validation for Rust signing and lock overlap."""
import json
import unittest

import rust_concurrent_probe as probe
import run_lifecycle_probe as runner
from test_rust_session_abandon import final as abandon_final


def final(case="completed-before-lock"):
    if case == "session-abandon":
        return abandon_final()
    matrix = {
        "completed-before-lock": ("before-native-send", "published", 1, 1, 1, 1),
        "lock-before-native-send": ("before-native-send", "cancelled_by_lock", 0, 0, 0, 0),
        "lock-native-response-pending": ("native-response-pending", "cancelled_by_lock", 1, 0, 0, 0),
        "lock-after-native-reply": ("after-native-reply", "cancelled_by_lock", 1, 1, 0, 0),
        "lock-before-publication": ("before-publication", "cancelled_by_lock", 1, 1, 1, 0),
        "parallel-lock-callers": ("before-native-send", "cancelled_by_lock", 0, 0, 0, 0),
        "lease-change-before-publication": ("before-publication", "lease_invalidated", 1, 1, 1, 0),
    }
    seam, outcome, sent, reply, verified, published = matrix[case]
    closing = outcome == "cancelled_by_lock"
    count = 2 if case == "parallel-lock-callers" else 1
    return {"case": case, "status": "observed", "selected_seam": seam, "request_outcome": outcome,
            "state": "CLOSED", "active": False, "native_requests_sent": sent, "native_replies_received": reply,
            "proofs_verified": verified, "proofs_published": published, "lock_callers": count,
            "lock_acknowledgments": count, "workers_joined": 1, "close_acknowledged": True,
            "resources_closed_after_drain": True, "closing_observed_while_paused": closing,
            "request_late_refused": closing, "drain_wait_observed": closing,
            "busy_refused": int(outcome == "published"), "closed_refused": 1,
            "creator_tid": 333, "agent_pid": 111, "loader_pid": 222,
            "agent_exit_code": None, "agent_exit_signal": 9, "loader_exit_code": 0, "loader_exit_signal": None,
            "pressure_iterations": 1, "version": 1, "custody_accredited": False,
            "full_session_accredited": False, "concurrent_lock_tested": True}


def observation():
    cases = []
    for name in probe.CASES:
        value = final(name)
        value.update(readiness_proof_verified=True, published_proof_verified=name == "completed-before-lock",
                     lease_invalidated_observed=name == "lease-change-before-publication",
                     retained_connection_refused=True, new_connection_refused=True,
                     pty_master_retained_through_exit=True, pty_slave_private=True, sealed_snapshot=True)
        if name == "session-abandon":
            value.update(helper_responsive_after_drop=True, retained_descriptors_independently_checked=True,
                         agent_exit_signal=9, agent_exit_code=None, helper_exit_code=0,
                         resources_retained_until_helper_exit=True)
        cases.append(value)
    return {"version": 1, "experiment": "rust-concurrent", "status": "observed", "cases": cases,
            "custody_accredited": False, "launcher_accredited": False, "full_session_accredited": False,
            "ssh_disconnect_tested": False, "concurrent_lock_tested": True,
            "sentinel_survived": True, "resources_closed_after_exit": True}


IDENTITY = {"agent_pid": 111, "loader_pid": 222, "creator_tid": 333}


class ConcurrentProtocolTests(unittest.TestCase):
    def test_startup_cannot_substitute_child_or_creator_identity(self):
        from test_rust_loader_probe import frame
        identity = frame("prompt")
        value = frame("handoff")
        self.assertEqual(probe.bound_stage(json.dumps(value).encode(), "handoff", identity), value)
        for field in ("agent_pid", "loader_pid", "creator_tid"):
            with self.subTest(field=field), self.assertRaises(probe.loader.ProbeError):
                probe.bound_stage(json.dumps(dict(value, **{field: 99})).encode(), "handoff", identity)

    def test_eight_exact_outcomes_and_independent_runner_validation(self):
        self.assertEqual(len(set(probe.CASES)), 8)
        for name in probe.CASES:
            value = final(name)
            parsed = (probe.parse_abandon_final(json.dumps(value).encode(), IDENTITY) if name == "session-abandon"
                      else probe.parse_final(json.dumps(value).encode(), name, IDENTITY))
            self.assertEqual(parsed, value)
        runner.validate_rust_concurrent_observation(observation())

    def test_result_publication_requires_selected_work_and_cause(self):
        for name, changes in (("lock-before-publication", {"proofs_published": 1}),
                              ("lock-after-native-reply", {"native_replies_received": 0}),
                              ("lease-change-before-publication", {"request_outcome": "cancelled_by_lock"}),
                              ("completed-before-lock", {"proofs_verified": 0}),
                              ("lock-before-native-send", {"native_requests_sent": True})):
            with self.subTest(name=name), self.assertRaises(probe.loader.ProbeError):
                probe.parse_final(json.dumps(dict(final(name), **changes)).encode(), name, IDENTITY)

    def test_ack_requires_worker_join_native_exit_and_real_overlap(self):
        for changes in ({"workers_joined": 0}, {"active": True}, {"resources_closed_after_drain": False},
                        {"agent_exit_signal": 0}, {"loader_exit_code": False}, {"lock_acknowledgments": 1},
                        {"closing_observed_while_paused": False}, {"drain_wait_observed": False}):
            with self.subTest(changes=changes), self.assertRaises(probe.loader.ProbeError):
                probe.parse_final(json.dumps(dict(final("parallel-lock-callers"), **changes)).encode(),
                                  "parallel-lock-callers", IDENTITY)

    def test_final_cannot_change_native_identity_or_export_extra_payload(self):
        for changes in ({"agent_pid": 444}, {"creator_tid": True}, {"signature": "public proof on control only"},
                        {"full_session_accredited": True}, {"version": True}):
            with self.subTest(changes=changes), self.assertRaises(probe.loader.ProbeError):
                probe.parse_final(json.dumps(dict(final(), **changes)).encode(), "completed-before-lock", IDENTITY)

    def test_runner_refuses_missing_matrix_or_fabricated_external_controls(self):
        for field, value in (("lease_invalidated_observed", False), ("readiness_proof_verified", False),
                             ("pty_slave_private", False), ("retained_connection_refused", False)):
            report = observation()
            next(case for case in report["cases"] if case["case"] == "lease-change-before-publication")[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_concurrent_observation(report)
        report = observation()
        report["cases"].pop()
        with self.assertRaises(ValueError):
            runner.validate_rust_concurrent_observation(report)


if __name__ == "__main__":
    unittest.main()
