"""Concurrent admission regressions without native executables."""

from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

import concurrent_probe as probe


class GateTests(unittest.TestCase):
    def resource(self):
        resource = Mock(state="READY", root=Path("root"))
        resource._validate = Mock()
        resource.close_acknowledged = False
        def close():
            resource.state = "CLOSED"
            resource.close_acknowledged = True
            return True
        resource.lock.side_effect = close
        return resource

    def test_lock_waits_for_active_request_before_releasing_descriptors(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        reached, release = threading.Event(), threading.Event()
        result = []
        in_use = [False]
        original_close = resource.lock.side_effect
        def checked_close():
            self.assertFalse(in_use[0])
            return original_close()
        resource.lock.side_effect = checked_close
        def work():
            in_use[0] = True
            reached.set()
            try:
                self.assertTrue(release.wait(2))
                return {"proof": True}
            finally:
                in_use[0] = False
        def sign():
            try:
                gate.sign(work=work)
            except probe.Cancelled:
                result.append("cancelled")
        signer = threading.Thread(target=sign)
        signer.start()
        self.assertTrue(reached.wait(2))
        locker = threading.Thread(target=gate.lock)
        locker.start()
        self.assertTrue(gate.closing.wait(2))
        self.assertTrue(gate.drain_wait_started.wait(2))
        resource.lock.assert_not_called()
        with self.assertRaises(probe.AdmissionClosed):
            gate.sign(work=Mock())
        release.set()
        signer.join(2)
        locker.join(2)
        self.assertFalse(signer.is_alive() or locker.is_alive())
        self.assertEqual(result, ["cancelled"])
        self.assertEqual(gate.state, "CLOSED")
        self.assertTrue(gate.responses.empty())
        resource.lock.assert_called_once()

    def test_publication_before_lock_remains_available(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        proof = {"verified": True}
        gate.sign(work=lambda: proof)
        with self.assertRaises(probe.Busy):
            gate.sign(work=Mock())
        gate.lock()
        self.assertEqual(gate.take_proof(), proof)
        self.assertEqual(gate.snapshot()["proofs_published"], 1)
        with self.assertRaises(probe.AdmissionClosed):
            gate.sign(work=Mock())

    def test_parallel_lock_callers_share_one_resource_close(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        reached, release = threading.Event(), threading.Event()
        def work():
            reached.set()
            self.assertTrue(release.wait(2))
            return {"proof": True}
        signer = probe.Call(lambda: gate.sign(work=work)).start()
        self.assertTrue(reached.wait(2))
        lockers = [probe.Call(gate.lock).start() for _ in range(2)]
        gate.wait_lock_callers(2)
        self.assertTrue(gate.drain_wait_started.wait(2))
        self.assertFalse(any(call.done.is_set() for call in lockers))
        resource.lock.assert_not_called()
        release.set()
        probe.join_calls([signer] + lockers)
        self.assertIsInstance(signer.error, probe.Cancelled)
        self.assertTrue(all(call.error is None and call.result is True for call in lockers))
        resource.lock.assert_called_once()
        self.assertEqual(gate.snapshot()["lock_acknowledgments"], 2)

    def test_failed_stop_does_not_acknowledge_and_retry_observes_real_close(self):
        resource = self.resource()
        resource._stop_agent.side_effect = [probe.ProbeError("still alive"), None]
        gate = probe.Gate(resource)
        with self.assertRaises(probe.ProbeError):
            gate.lock()
        self.assertEqual(gate.state, "CLOSING")
        self.assertFalse(gate.close_acknowledged)
        resource.lock.assert_not_called()
        self.assertTrue(gate.lock())
        self.assertTrue(gate.close_acknowledged)

    def test_drain_timeout_preserves_resources_and_rejects_ack(self):
        resource = self.resource()
        gate = probe.Gate(resource, timeout=0.01)
        reached, release = threading.Event(), threading.Event()
        def work():
            reached.set()
            self.assertTrue(release.wait(2))
            return {"proof": True}
        signer = probe.Call(lambda: gate.sign(work=work)).start()
        self.assertTrue(reached.wait(2))
        try:
            with self.assertRaisesRegex(probe.ProbeError, "did not drain"):
                gate.lock()
            self.assertEqual(gate.state, "CLOSING")
            self.assertFalse(gate.close_acknowledged)
            resource.lock.assert_not_called()
        finally:
            release.set()
            probe.join_calls([signer])
        self.assertIsInstance(signer.error, probe.Cancelled)
        self.assertTrue(gate.lock())

    def test_resource_return_without_closed_state_cannot_acknowledge(self):
        resource = self.resource()
        resource.lock.side_effect = None
        resource.lock.return_value = True
        gate = probe.Gate(resource)
        with self.assertRaises(probe.ProbeError):
            gate.lock()
        self.assertEqual(gate.state, "CLOSING")
        self.assertFalse(gate.close_acknowledged)

    def test_backend_error_is_preserved_and_closes_without_publication(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        failure = ValueError("changed revision")
        with self.assertRaises(ValueError) as raised:
            gate.sign(work=Mock(side_effect=failure))
        self.assertIs(raised.exception, failure)
        self.assertEqual(gate.state, "CLOSED")
        self.assertTrue(gate.responses.empty())

    def test_uncertain_verifier_never_marks_work_drained(self):
        resource = self.resource()
        gate = probe.Gate(resource, timeout=0.01)
        with patch.object(probe, "WORK_UNCERTAIN", False):
            with self.assertRaises(probe.VerifierUncertain):
                gate.sign(work=Mock(side_effect=probe.VerifierUncertain("still running")))
            self.assertTrue(probe.WORK_UNCERTAIN)
            self.assertTrue(gate.snapshot()["active"])
            self.assertFalse(gate.close_acknowledged)
            resource.lock.assert_not_called()

    def test_active_signer_cannot_wait_for_itself(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        with self.assertRaisesRegex(probe.ProbeError, "cannot wait for itself"):
            gate.sign(work=gate.lock)
        self.assertEqual(gate.state, "CLOSED")

    def test_verifier_does_not_use_preexec_or_capture_unbounded_output(self):
        process = Mock()
        process.wait.return_value = 0
        process.poll.return_value = 0
        with tempfile.TemporaryDirectory() as temporary, patch.object(probe, "THREAD_PARENT_READY", True), \
                patch.object(probe.subprocess, "Popen", return_value=process) as launch:
            probe.threaded_verify(Path("verifier"), Path(temporary), {}, {"proof": True}, "proof", True)
        self.assertNotIn("preexec_fn", launch.call_args.kwargs)
        self.assertEqual(launch.call_args.kwargs["stdout"], subprocess.DEVNULL)
        self.assertEqual(launch.call_args.kwargs["stderr"], subprocess.DEVNULL)
        process.wait.assert_called_once_with(timeout=probe.TIMEOUT)

    def test_verifier_that_survives_kill_is_uncertain(self):
        process = Mock()
        process.wait.side_effect = subprocess.TimeoutExpired("verifier", 1)
        process.poll.return_value = None
        with tempfile.TemporaryDirectory() as temporary, patch.object(probe, "THREAD_PARENT_READY", True), \
                patch.object(probe.subprocess, "Popen", return_value=process):
            with self.assertRaises(probe.VerifierUncertain):
                probe.threaded_verify(Path("verifier"), Path(temporary), {}, {"proof": True}, "proof", True)
        process.kill.assert_called_once()

    def test_wrong_negative_reason_cannot_pass_a_case(self):
        call = probe.Call(lambda: None)
        call.error = probe.ProbeError("early startup failure")
        with self.assertRaises(probe.ProbeError):
            probe.require_error(call, probe.Cancelled)

    def test_join_timeout_refuses_a_successful_matrix(self):
        call = Mock()
        call.thread.is_alive.return_value = True
        with self.assertRaisesRegex(probe.ProbeError, "did not exit"):
            probe.join_calls([call])

    def test_contended_state_mutex_times_out_without_claiming_to_close(self):
        resource = self.resource()
        gate = probe.Gate(resource, timeout=0.01)
        reached, release = threading.Event(), threading.Event()
        def hold():
            with gate.condition:
                reached.set()
                self.assertTrue(release.wait(2))
        holder = probe.Call(hold).start()
        self.assertTrue(reached.wait(2))
        try:
            with self.assertRaisesRegex(probe.ProbeError, "state acquisition"):
                gate.lock()
            self.assertEqual(gate.state, "READY")
            self.assertFalse(gate.closing.is_set())
            self.assertFalse(gate.close_acknowledged)
            resource._stop_agent.assert_not_called()
        finally:
            release.set()
            probe.join_calls([holder])

    def test_native_stop_consumes_the_same_lock_deadline(self):
        resource = self.resource()
        gate = probe.Gate(resource, timeout=1)
        clock = [0.0]
        resource._stop_agent.side_effect = lambda: clock.__setitem__(0, 2.0)
        with patch.object(probe.time, "monotonic", side_effect=lambda: clock[0]):
            with self.assertRaisesRegex(probe.ProbeError, "total deadline"):
                gate.lock()
        self.assertEqual(gate.state, "CLOSING")
        self.assertFalse(gate.close_acknowledged)
        resource.lock.assert_not_called()

    def test_cleanup_after_deadline_cannot_acknowledge_even_if_native_exit_succeeded(self):
        resource = self.resource()
        gate = probe.Gate(resource, timeout=1)
        clock = [0.0]
        original_close = resource.lock.side_effect
        def delayed_close():
            original_close()
            clock[0] = 2.0
            return True
        resource.lock.side_effect = delayed_close
        with patch.object(probe.time, "monotonic", side_effect=lambda: clock[0]):
            with self.assertRaisesRegex(probe.ProbeError, "total deadline"):
                gate.lock()
        self.assertEqual(resource.state, "CLOSED")
        self.assertEqual(gate.state, "CLOSING")
        self.assertFalse(gate.close_acknowledged)

    def test_commit_validation_does_not_hold_the_state_mutex(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        validations = [0]
        observed = []
        def validate():
            validations[0] += 1
            if validations[0] == 2:
                acquired = threading.Event()
                def inspect_state():
                    with gate.condition:
                        acquired.set()
                observer = probe.Call(inspect_state).start()
                observed.append(acquired.wait(1))
                probe.join_calls([observer])
        resource._validate.side_effect = validate
        gate.sign(work=lambda: {"proof": True})
        self.assertEqual(observed, [True])
        self.assertEqual(gate.take_proof(), {"proof": True})

    def test_cancelled_pending_response_is_not_consumed_after_the_barrier(self):
        resource = self.resource()
        gate = probe.Gate(resource)
        connection = Mock()
        def close_admission(stage):
            if stage == "native-response-pending":
                with gate.condition:
                    gate.state = "CLOSING"
                    gate.epoch += 1
        with patch.object(probe.socket, "socket", return_value=connection), \
                patch.object(probe.socket, "AF_UNIX", 1, create=True), \
                patch.object(probe.native, "receive_exact") as receive:
            with self.assertRaises(probe.Cancelled):
                gate._native_sign(b"public", b"challenge", 0, close_admission)
        receive.assert_not_called()
        connection.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
