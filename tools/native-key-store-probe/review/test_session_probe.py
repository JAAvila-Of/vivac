"""Session admission regressions without native processes or containers."""

from pathlib import Path
import unittest
from unittest.mock import Mock, patch

import session_probe as probe


class SessionTests(unittest.TestCase):
    def session(self):
        session = probe.Session(Path("writer"), Path("verifier"), Path("package"), Path("root"))
        session._open_package = Mock()
        session._launch_agent = Mock()
        session._load_agent = Mock()
        session._validate = Mock()
        session._proof = Mock(return_value={"verified": True})
        session._release = Mock()
        return session

    def test_ready_is_not_published_before_verified_proof(self):
        session = self.session()
        session._proof.side_effect = probe.ProbeError("invalid proof")
        with self.assertRaises(probe.ProbeError):
            session.start()
        self.assertFalse(session.ready_published)
        self.assertEqual(session.state, "CLOSED")
        session._release.assert_called_once()

    def test_changed_lease_before_sign_never_calls_signer(self):
        session = self.session()
        session.start()
        session._proof.reset_mock()
        session._validate.side_effect = probe.ProbeError("changed lease")
        with self.assertRaises(probe.ProbeError):
            session.sign()
        session._proof.assert_not_called()
        self.assertEqual(session.published_proofs, 0)
        self.assertEqual(session.state, "CLOSED")

    def test_changed_lease_after_native_signature_discards_response(self):
        session = self.session()
        session.start()
        session._proof.reset_mock()
        def invalidate(_session):
            session._validate.side_effect = probe.ProbeError("changed after sign")
        with self.assertRaises(probe.ProbeError):
            session.sign(before_publish=invalidate)
        session._proof.assert_called_once()
        self.assertEqual(session.published_proofs, 0)
        self.assertEqual(session.state, "CLOSED")

    def test_lock_during_startup_prevents_ready_and_native_proof(self):
        session = self.session()
        with self.assertRaises(probe.ProbeError):
            session.start(before_ready=lambda value: value.lock())
        session._proof.assert_not_called()
        self.assertFalse(session.ready_published)
        self.assertEqual(session.state, "CLOSED")

    def test_failed_close_never_acknowledges_closed(self):
        session = self.session()
        session.start()
        session._release.side_effect = probe.ProbeError("still alive")
        with self.assertRaises(probe.ProbeError):
            session.lock()
        self.assertEqual(session.state, "CLOSING")
        self.assertFalse(session.close_acknowledged)
        with self.assertRaises(probe.ProbeError):
            session.sign()

    def test_closed_session_cannot_reopen_or_sign(self):
        session = self.session()
        session.start()
        session.lock()
        session.lock()
        session._proof.reset_mock()
        with self.assertRaises(probe.ProbeError):
            session.start()
        with self.assertRaises(probe.ProbeError):
            session.sign()
        session._proof.assert_not_called()
        self.assertTrue(session.close_acknowledged)

    def test_failed_agent_acquisition_stops_child_and_closes_channels(self):
        session = self.session()
        del session._launch_agent
        del session._release
        parent, child = Mock(), Mock()
        child.fileno.return_value = 7
        process = Mock(pid=123)
        process.poll.return_value = None
        with patch.object(probe.socket, "socketpair", return_value=(parent, child)), \
                patch.object(probe.subprocess, "Popen", return_value=process), \
                patch.object(probe.os, "pidfd_open", side_effect=OSError("pidfd unavailable"), create=True), \
                patch.object(probe.life, "unlink_owned_socket"):
            with self.assertRaises(OSError):
                session.start()
        process.kill.assert_called_once()
        process.wait.assert_called_once()
        parent.close.assert_called_once()
        child.close.assert_called()
        self.assertEqual(session.state, "CLOSED")

    def test_failed_agent_stop_still_closes_other_owned_resources(self):
        session = self.session()
        del session._release
        session.state = "READY"
        session.pidfd, session.memfd, session.snapshot = 10, 11, {"owned": True}
        session._stop_agent = Mock(side_effect=probe.ProbeError("agent survived"))
        backend = Mock()
        snapshot = session.snapshot
        with patch.object(probe, "package_backend", return_value=backend), \
                patch.object(probe.os, "close") as close, \
                patch.object(probe.life, "unlink_owned_socket") as unlink:
            with self.assertRaises(probe.ProbeError):
                session.lock()
        self.assertEqual({call.args[0] for call in close.call_args_list}, {10, 11})
        backend.close_retained_package.assert_called_once_with(snapshot)
        unlink.assert_not_called()
        self.assertEqual(session.state, "CLOSING")
        self.assertFalse(session.close_acknowledged)

    def test_retry_after_failed_stop_never_reuses_a_closed_descriptor(self):
        session = self.session()
        del session._release
        session.state = "READY"
        session.pidfd, session.memfd, session.snapshot = 10, 11, {"owned": True}
        session.agent = Mock()
        session.agent.poll.return_value = None
        backend = Mock()
        with patch.object(probe, "package_backend", return_value=backend), \
                patch.object(probe.os, "close") as close, \
                patch.object(probe.life, "stop_agent", side_effect=probe.ProbeError("agent survived")) as stop, \
                patch.object(probe.life, "unlink_owned_socket"):
            with self.assertRaises(probe.ProbeError):
                session.lock()
            self.assertIsNone(session.pidfd)
            self.assertIsNone(session.memfd)
            self.assertIsNone(session.snapshot)
            self.assertEqual(session.state, "CLOSING")
            self.assertFalse(session.close_acknowledged)
            session.lock()
        stop.assert_called_once_with(10)
        self.assertEqual([call.args[0] for call in close.call_args_list], [10, 11])
        session.agent.kill.assert_called_once()
        session.agent.wait.assert_called_once()
        backend.close_retained_package.assert_called_once()
        self.assertEqual(session.state, "CLOSED")

    def test_store_outside_dedicated_mount_is_refused_before_backend_or_writer(self):
        with patch.object(probe, "package_backend") as backend, \
                patch.object(probe.subprocess, "Popen") as process:
            with self.assertRaisesRegex(probe.ProbeError, "dedicated fixture mount"):
                probe.require_store(Path("/other/run"))
        backend.assert_not_called()
        process.assert_not_called()

    def test_store_symlink_is_refused_before_filesystem_inspection(self):
        backend = Mock()
        with patch.object(probe, "package_backend", return_value=backend), \
                patch.object(probe.Path, "is_symlink", return_value=True), \
                patch.object(probe.subprocess, "Popen") as process:
            with self.assertRaisesRegex(probe.ProbeError, "symlink"):
                probe.require_store(Path("/store/run"))
        backend.filesystem_magic.assert_not_called()
        process.assert_not_called()

    def test_store_without_mountpoint_is_refused_before_filesystem_inspection(self):
        backend = Mock()
        backend.is_mountpoint.return_value = False
        with patch.object(probe, "package_backend", return_value=backend), \
                patch.object(probe.Path, "is_symlink", return_value=False), \
                patch.object(probe.subprocess, "Popen") as process:
            with self.assertRaisesRegex(probe.ProbeError, "mountpoint"):
                probe.require_store(Path("/store/run"))
        backend.filesystem_magic.assert_not_called()
        process.assert_not_called()

    def test_early_unrelated_failure_does_not_pass_a_negative_case(self):
        class ExpectedLeaseFailure(Exception):
            pass
        operation = Mock(side_effect=probe.ProbeError("loader failed before mutation"))
        with self.assertRaisesRegex(probe.ProbeError, "loader failed"):
            probe.expect_refusal(operation, ExpectedLeaseFailure)

    def test_same_error_class_with_wrong_gate_does_not_pass(self):
        operation = Mock(side_effect=probe.ProbeError("loader failed"))
        with self.assertRaisesRegex(probe.ProbeError, "wrong rejection gate"):
            probe.expect_refusal(operation, probe.ProbeError, message="owned agent is not alive")


if __name__ == "__main__":
    unittest.main()
