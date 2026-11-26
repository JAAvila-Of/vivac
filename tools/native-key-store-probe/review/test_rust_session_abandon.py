"""Reject false closure and recovery claims from the session-abandon fixture."""
import json
import unittest

import rust_concurrent_probe as probe
import run_lifecycle_probe as runner


IDENTITY = {"agent_pid": 111, "loader_pid": 222, "creator_tid": 333}


def final():
    return dict(IDENTITY, case="session-abandon", status="observed", version=1,
                selected_seam="before-native-send", request_outcome="failed_after_abandon",
                state="CLOSING", active=False, native_requests_sent=0, native_replies_received=0,
                proofs_verified=0, proofs_published=0, workers_joined=1, lock_callers=1,
                lock_acknowledgments=0, close_acknowledged=False, resources_closed_after_drain=False,
                admission_uncertain=True, readmission_refused=3, descriptors_retained=True,
                retained_descriptor_count=5, drop_elapsed_ns=1, loader_exit_code=0,
                loader_exit_signal=None, pressure_iterations=1, custody_accredited=False,
                full_session_accredited=False, concurrent_lock_tested=False, callback_uncertain=True)


class SessionAbandonProtocolTests(unittest.TestCase):
    def test_abandonment_reports_retention_without_acknowledged_closure(self):
        value = final()
        self.assertEqual(probe.parse_abandon_final(json.dumps(value).encode(), IDENTITY), value)

    def test_abandonment_cannot_claim_cleanup_or_hide_live_work(self):
        for changes in ({"close_acknowledged": True}, {"resources_closed_after_drain": True},
                        {"workers_joined": 0}, {"active": True}, {"proofs_published": 1},
                        {"admission_uncertain": False}, {"readmission_refused": 0},
                        {"descriptors_retained": False}, {"callback_uncertain": False},
                        {"drop_elapsed_ns": True}, {"drop_elapsed_ns": 0},
                        {"agent_pid": True}, {"full_session_accredited": True},
                        {"version": True}, {"secret": "unselected payload"}):
            with self.subTest(changes=changes), self.assertRaises(probe.loader.ProbeError):
                probe.parse_abandon_final(json.dumps(dict(final(), **changes)).encode(), IDENTITY)

    def test_runner_requires_independent_response_and_retention_without_ack(self):
        from test_rust_concurrent_probe import observation
        for changes in ({"helper_responsive_after_drop": False},
                        {"retained_descriptors_independently_checked": False},
                        {"resources_retained_until_helper_exit": False},
                        {"close_acknowledged": True}, {"resources_closed_after_drain": True},
                        {"agent_exit_signal": 0}, {"agent_exit_signal": True},
                        {"helper_exit_code": 9}, {"concurrent_lock_tested": True}):
            report = observation()
            next(case for case in report["cases"] if case["case"] == "session-abandon").update(changes)
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                runner.validate_rust_concurrent_observation(report)


if __name__ == "__main__":
    unittest.main()
