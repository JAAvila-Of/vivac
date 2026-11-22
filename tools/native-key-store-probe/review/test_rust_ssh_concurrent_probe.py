"""Pure admission checks for SSH loss during Rust signing."""
import json
import unittest

import rust_ssh_concurrent_probe as probe
from test_rust_concurrent_probe import final as local_final


IDENTITY = {"agent_pid": 111, "loader_pid": 222, "creator_tid": 333}


def final(case="normal-close"):
    source = {"normal-close": "completed-before-lock", "disconnect-published": "completed-before-lock",
              "disconnect-before-native-send": "lock-before-native-send",
              "disconnect-native-response-pending": "lock-native-response-pending",
              "disconnect-after-native-reply": "lock-after-native-reply",
              "disconnect-before-publication": "lock-before-publication"}[case]
    value = local_final(source)
    published = case in ("normal-close", "disconnect-published")
    value.update(case=case, selected_seam="published" if published else case.removeprefix("disconnect-"),
                 request_outcome="published" if published else "cancelled_by_ssh",
                 ssh_disconnect_observed=case != "normal-close",
                 ssh_disconnect_cause=None if case == "normal-close" else "eof")
    return value


class ProtocolTests(unittest.TestCase):
    def test_twelve_exact_transport_cases_have_strict_outcomes(self):
        self.assertEqual(len(set(probe.CASES)), 12)
        for suffix in ("normal-close", "disconnect-before-native-send", "disconnect-native-response-pending",
                       "disconnect-after-native-reply", "disconnect-before-publication", "disconnect-published"):
            value = final(suffix)
            self.assertEqual(probe.parse_final(json.dumps(value).encode(), suffix, IDENTITY), value)
            for mode in ("pty", "pipe"):
                self.assertIn(mode + "-" + suffix, probe.CASES)

    def test_disconnect_requires_observed_cause_and_cannot_invent_publication(self):
        for case, changes in (("disconnect-before-publication", {"ssh_disconnect_cause": None}),
                              ("disconnect-native-response-pending", {"ssh_disconnect_cause": "sigkill"}),
                              ("normal-close", {"ssh_disconnect_cause": "sighup"}),
                              ("disconnect-after-native-reply", {"proofs_published": 1}),
                              ("disconnect-published", {"proofs_published": 0}),
                              ("disconnect-before-native-send", {"ssh_disconnect_observed": 1})):
            with self.subTest(case=case, changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_final(json.dumps(dict(final(case), **changes)).encode(), case, IDENTITY)

    def test_closure_requires_join_native_exits_and_selected_work_counts(self):
        for changes in ({"workers_joined": 0}, {"close_acknowledged": False}, {"active": True},
                        {"resources_closed_after_drain": False}, {"agent_exit_signal": 0},
                        {"loader_exit_code": False}, {"native_requests_sent": 0},
                        {"native_replies_received": 1}, {"drain_wait_observed": False}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_final(json.dumps(dict(final("disconnect-native-response-pending"), **changes)).encode(),
                                  "disconnect-native-response-pending", IDENTITY)

    def test_identity_extra_payload_and_duplicate_fields_are_refused(self):
        for changes in ({"agent_pid": 222}, {"creator_tid": True}, {"signature": "public proof stays on control"},
                        {"full_session_accredited": True}, {"pressure_iterations": 0}, {"version": True}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_final(json.dumps(dict(final(), **changes)).encode(), "normal-close", IDENTITY)
        for payload in (b'{"case":"normal-close","case":"another"}', b" " * 8193):
            with self.subTest(length=len(payload)), self.assertRaises(Exception):
                probe.parse_final(payload, "normal-close", IDENTITY)

    def test_transfer_requires_exactly_six_cloexec_capabilities(self):
        probe.validate_transfer(b"I", [3, 4, 5, 6, 7, 8], False, True)
        for marker, values, truncated, valid in ((b"F", [3,4,5,6,7,8], False, True),
                (b"I", [3,4,5,6,7], False, True), (b"I", [3,4,5,6,7,8,9], False, True),
                (b"I", [3,4,5,6,7,8], True, True), (b"I", [3,4,5,6,7,8], False, False)):
            with self.subTest(marker=marker, count=len(values)), self.assertRaises(probe.ProbeError):
                probe.validate_transfer(marker, values, truncated, valid)

    def test_closing_frame_is_not_a_close_ack_and_requires_transport_cause(self):
        value = {"stage": "closing-paused", "case": "disconnect-before-publication", "seam": "before-publication",
                 "active": True, "close_ack": False, "lock_callers": 1,
                 "ssh_disconnect_observed": True, "ssh_disconnect_cause": "sighup"}
        probe.validate_closing(value, "disconnect-before-publication")
        for changes in ({"close_ack": True}, {"active": False}, {"lock_callers": True},
                        {"ssh_disconnect_cause": None}, {"ssh_disconnect_observed": False}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.validate_closing(dict(value, **changes), "disconnect-before-publication")
        for suffix in ("normal-close", "disconnect-published"):
            with self.subTest(suffix=suffix), self.assertRaises(probe.ProbeError):
                probe.validate_closing(dict(value, case=suffix, seam="published"), suffix)


if __name__ == "__main__":
    unittest.main()
