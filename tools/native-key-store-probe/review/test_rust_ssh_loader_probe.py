"""Pure admission checks for the SSH Rust loader observer."""
import json
import unittest

import rust_ssh_loader_probe as probe
from test_rust_loader_probe import frame


class ProtocolTests(unittest.TestCase):
    def test_matrix_has_all_ten_transport_seams(self):
        self.assertEqual(len(set(probe.CASES)), 10)
        for mode in ("pty", "pipe"):
            for suffix in ("normal-close", "disconnect-prompt", "disconnect-handoff",
                           "disconnect-before-ready", "disconnect-ready"):
                self.assertIn(mode + "-" + suffix, probe.CASES)

    def test_final_disconnect_requires_a_real_transport_cause(self):
        value = dict(frame("closed"), version=1, case="normal", status="observed", pressure_iterations=1,
                     custody_accredited=False, launcher_accredited=False, full_session_accredited=False,
                     ssh_disconnect_observed=True, ssh_disconnect_cause="eof")
        self.assertEqual(probe.parse_final(json.dumps(value).encode(), True), value)
        for changes in ({"ssh_disconnect_cause": None}, {"ssh_disconnect_cause": "killed"},
                        {"ssh_disconnect_observed": 1}, {"extra": False}):
            with self.subTest(changes=changes), self.assertRaises(Exception):
                probe.parse_final(json.dumps(dict(value, **changes)).encode(), True)
        with self.assertRaises(Exception):
            probe.parse_final(json.dumps(value).encode(), False)

    def test_normal_final_requires_null_disconnect_cause(self):
        value = dict(frame("closed"), version=1, case="normal", status="observed", pressure_iterations=1,
                     custody_accredited=False, launcher_accredited=False, full_session_accredited=False,
                     ssh_disconnect_observed=False, ssh_disconnect_cause=None)
        self.assertEqual(probe.parse_final(json.dumps(value).encode(), False), value)
        with self.assertRaises(Exception):
            probe.parse_final(json.dumps(dict(value, ssh_disconnect_cause="sighup")).encode(), False)

    def test_native_identity_cannot_change_between_stage_packets(self):
        identity = frame("prompt")
        value = frame("handoff")
        self.assertEqual(probe.parse_bound_stage(json.dumps(value).encode(), "handoff", identity), value)
        for key in ("agent_pid", "loader_pid", "creator_tid"):
            with self.subTest(key=key), self.assertRaises(probe.ProbeError):
                probe.parse_bound_stage(json.dumps(dict(value, **{key: 99})).encode(), "handoff", identity)

    def test_final_packets_reject_duplicate_keys_and_oversized_payloads(self):
        for packet in (b'{"ssh_disconnect_observed":false,"ssh_disconnect_observed":true}', b" " * 8193):
            with self.subTest(size=len(packet)), self.assertRaises(Exception):
                probe.parse_final(packet, False)

    def test_initial_descriptor_transfer_has_exact_marker_and_five_descriptors(self):
        probe.validate_transfer(b"I", [3, 4, 5, 6, 7], False, True)
        for marker, descriptors, truncated, valid in ((b"F", [3,4,5,6,7], False, True),
                (b"I", [3,4,5,6], False, True), (b"I", [3,4,5,6,7,8], False, True),
                (b"I", [3,4,5,6,7], True, True), (b"I", [3,4,5,6,7], False, False)):
            with self.subTest(marker=marker, count=len(descriptors)), self.assertRaises(probe.ProbeError):
                probe.validate_transfer(marker, descriptors, truncated, valid)


if __name__ == "__main__":
    unittest.main()
