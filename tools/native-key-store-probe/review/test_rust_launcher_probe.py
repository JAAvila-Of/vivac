"""Pure controls for bounded Rust launcher observations."""
import errno
import json
import struct
import unittest
from unittest.mock import Mock

import rust_launcher_probe as probe

def result(case):
    value = {"version": 1, "case": case, "status": "ready" if case.endswith("owner-death") else "observed",
             "custody_accredited": False, "launcher_accredited": False, "threadsafe_fullflow": False}
    value.update({key: True for key in probe.NORMAL_FLAGS[case]})
    value.update({key: 1 for key in probe.EXTRA[case]})
    if case == "stable-thread":
        value.update(native_children_observed=4, caller_threads_returned=4, pressure_threads=4)
    if case == "exec-error":
        value.update(exec_failure_raw_os_error=errno.ENOEXEC, exec_preexec_seams_observed=["B", "A"])
    return value

class MetadataTests(unittest.TestCase):
    def test_all_normal_cases_require_exact_bounded_metadata(self):
        for case in probe.NORMAL_FLAGS:
            value = result(case)
            self.assertEqual(probe.parse_result(json.dumps(value).encode(), case, value["status"]), value)

    def test_no_full_admission_or_extra_payload_is_accepted(self):
        for field, invalid in (("launcher_accredited", True), ("custody_accredited", True),
                               ("threadsafe_fullflow", True), ("unexpected_payload", "public fixture")):
            value = result("stable-thread")
            value[field] = invalid
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.parse_result(json.dumps(value).encode(), "stable-thread")

    def test_concurrency_cannot_be_a_boolean_or_no_work(self):
        for field, invalid in (("pressure_threads", True), ("native_children_observed", 3),
                               ("pressure_iterations", 0), ("fd_allowlist_verified", False)):
            value = result("stable-thread")
            value[field] = invalid
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.parse_result(json.dumps(value).encode(), "stable-thread")

    def test_exec_error_cannot_be_a_permission_failure_or_missing_seam(self):
        for field, invalid in (("exec_failure_raw_os_error", errno.EACCES),
                               ("exec_preexec_seams_observed", ["B"]), ("owner_reusable_after_error", False)):
            value = result("exec-error")
            value[field] = invalid
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.parse_result(json.dumps(value).encode(), "exec-error")

    def test_wire_seam_requires_exact_size_phase_and_pid(self):
        channel = Mock()
        channel.recv.return_value = b"A\0\0\0" + struct.pack(">I", 123)
        self.assertEqual(probe.receive_seam(channel, "A", 123), 123)
        for packet in (b"A", b"B\0\0\0" + struct.pack(">I", 123),
                       b"A\0\0\0" + struct.pack(">I", 124), b"A\0\0\0" + struct.pack(">I", 0)):
            channel.recv.return_value = packet
            with self.subTest(packet=packet), self.assertRaises(probe.ProbeError):
                probe.receive_seam(channel, "A", 123)

if __name__ == "__main__":
    unittest.main()
