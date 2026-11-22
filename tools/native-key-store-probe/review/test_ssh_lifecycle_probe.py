import errno
import signal
import unittest
from unittest.mock import Mock, patch
from pathlib import Path

import ssh_lifecycle_probe as probe


class ChannelTests(unittest.TestCase):
    def test_eof_is_a_disconnect_without_a_control_cancel(self):
        channel = probe.SSHChannel(Mock(), 0)
        with patch.object(probe.select, "select", return_value=([0], [], [])), \
                patch.object(probe.os, "read", return_value=b""):
            with self.assertRaises(probe.SSHDisconnected) as caught:
                channel.command()
        self.assertEqual(caught.exception.cause, "eof")

    def test_eio_is_a_disconnect(self):
        channel = probe.SSHChannel(Mock(), 0)
        with patch.object(probe.select, "select", return_value=([0], [], [])), \
                patch.object(probe.os, "read", side_effect=OSError(errno.EIO, "not copied")):
            with self.assertRaises(probe.SSHDisconnected) as caught:
                channel.command()
        self.assertEqual(caught.exception.cause, "eio")

    def test_hup_handler_only_sets_a_flag(self):
        channel = probe.SSHChannel(Mock(), 0)
        channel.on_hup(1, None)
        channel.control.assert_not_called()
        with self.assertRaises(probe.SSHDisconnected) as caught:
            channel.command()
        self.assertEqual(caught.exception.cause, "sighup")

    def test_normal_control_continue_does_not_disconnect(self):
        control = Mock()
        channel = probe.SSHChannel(control, 0)
        with patch.object(probe.select, "select", return_value=([control], [], [])), \
                patch.object(probe, "receive", return_value={"command": "continue"}):
            self.assertEqual(channel.command(), {"command": "continue"})

    def test_terminal_data_is_refused_not_treated_as_a_command(self):
        channel = probe.SSHChannel(Mock(), 0)
        with patch.object(probe.select, "select", return_value=([0], [], [])), \
                patch.object(probe.os, "read", return_value=b"cancel"):
            with self.assertRaises(probe.ProbeError):
                channel.command()

    def test_disconnect_wins_over_simultaneous_control_continuation(self):
        control = Mock()
        channel = probe.SSHChannel(control, 0)
        with patch.object(probe.select, "select", return_value=([control, 0], [], [])), \
                patch.object(probe.os, "read", return_value=b""), \
                patch.object(probe, "receive") as receive:
            with self.assertRaises(probe.SSHDisconnected):
                channel.command()
        receive.assert_not_called()


class ObservationTests(unittest.TestCase):
    def test_ssh_agent_launch_requests_its_own_terminal_session(self):
        value = probe.SSHSession(Path("writer"), Path("verifier"), Path("package"), Path("root"), Mock(), Mock())
        with patch.object(probe.session.Session, "_launch_agent") as launch:
            value._launch_agent()
        launch.assert_called_once_with(isolate_terminal=True)

    def test_native_child_with_inherited_ssh_session_is_refused(self):
        with patch.object(probe.os, "getsid", return_value=50, create=True), \
                patch.object(probe.os, "getpgid", return_value=101, create=True):
            with self.assertRaises(probe.ProbeError):
                probe.require_private_terminal_session(101, 77)

    def test_worker_primary_and_cleanup_diagnostics_both_survive(self):
        primary = {"phase": "prompt", "error_type": "ProbeError", "errno": None, "code": "probe-error"}
        cleanup = {"phase": "prompt", "error_type": "OSError", "errno": 13, "code": "os-error"}
        error = probe.loader.WorkerFailed(primary)
        error.cleanup_diagnostic = cleanup
        report = {}
        with patch.object(probe, "CURRENT_REPORT", report), \
                patch.object(probe, "release_observed"):
            probe.finish_case({}, error, "observer-case")
        self.assertEqual(report["primary_failure"], primary)
        self.assertEqual(report["worker_cleanup_failure"], cleanup)

    def test_disconnect_before_selected_seam_is_refused(self):
        with self.assertRaises(probe.ProbeError):
            probe.require_stage({"kind": "final", "outcome": "ssh_disconnected"}, "prompt")

    def test_unknown_error_text_is_not_in_diagnostic(self):
        result = probe.safe_error(OSError(13, "private terminal transcript"), "observer-case")
        self.assertNotIn("private", str(result))
        self.assertEqual(result["errno"], 13)

    def test_cleanup_retains_resources_until_all_exits(self):
        bundle = {"supervisor_pidfd": 4, "agent_pidfd": 5, "loader_pidfd": 6}
        with patch.object(probe.life, "pidfd_ready", return_value=False), \
                patch.object(probe.os, "close") as close:
            with self.assertRaises(probe.ProbeError):
                probe.release_observed(bundle)
        close.assert_not_called()

    def test_worker_claim_rejects_unrelated_ancestry(self):
        connection = Mock()
        import struct
        connection.getsockopt.return_value = struct.pack("3i", 101, 65534, 65534)
        with patch.object(probe.socket, "SO_PEERCRED", 17, create=True), \
                patch.object(probe.os, "geteuid", return_value=65534, create=True), \
                patch.object(probe.os, "getegid", return_value=65534, create=True), \
                patch.object(probe.life, "process_start", return_value=3), \
                patch.object(probe.life, "inspect_agent"), \
                patch.object(probe.life, "agent_credentials", return_value={"PPid": [1]}), \
                patch.object(probe.os, "pidfd_open", return_value=9, create=True), \
                patch.object(probe.os, "close") as close:
            with self.assertRaises(probe.ProbeError):
                probe.claim_worker(connection, {"kind": "worker", "pid": 101, "start": 3, "parent": 1, "stdin_is_tty": True}, 77, 10, True)
        close.assert_called_once_with(9)

    def test_wrong_ssh_terminal_mode_is_refused_before_pidfd_claim(self):
        metadata = {"kind": "worker", "pid": 101, "start": 3, "parent": 77, "stdin_is_tty": False}
        with patch.object(probe.os, "pidfd_open", create=True) as claim:
            with self.assertRaises(probe.ProbeError):
                probe.claim_worker(Mock(), metadata, 77, 10, True)
        claim.assert_not_called()

    def test_received_worker_diagnostics_are_validated_separately(self):
        primary = {"phase": "prompt", "error_type": "ProbeError", "errno": None, "code": "probe-error"}
        cleanup = {"phase": "prompt", "error_type": "OSError", "errno": 13, "code": "os-error"}
        with patch.object(probe.life, "receive_json", return_value={
                "kind": "worker-error", "diagnostic": primary, "cleanup_diagnostic": cleanup}):
            with self.assertRaises(probe.loader.WorkerFailed) as caught:
                probe.receive(Mock())
        self.assertEqual(caught.exception.diagnostic, primary)
        self.assertEqual(caught.exception.cleanup_diagnostic, cleanup)

    def test_received_cleanup_diagnostic_cannot_contain_terminal_text(self):
        primary = {"phase": "prompt", "error_type": "ProbeError", "errno": None, "code": "probe-error"}
        cleanup = dict(primary, code="private terminal contents")
        with patch.object(probe.life, "receive_json", return_value={
                "kind": "worker-error", "diagnostic": primary, "cleanup_diagnostic": cleanup}):
            with self.assertRaises(probe.loader.ProbeError):
                probe.receive(Mock())


if __name__ == "__main__":
    unittest.main()
