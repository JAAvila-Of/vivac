"""Loader ownership regressions; no native processes are started."""

import argparse
import array
from contextlib import ExitStack, nullcontext
from pathlib import Path
import signal
import stat
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

import loader_probe as loader


class LoaderTests(unittest.TestCase):
    def test_post_release_exec_error_is_received_before_native_exit_classification(self):
        value = loader.OwnedLoader(Mock())
        value.process, value.master = Mock(), 42
        value.process.poll.return_value = 1
        control = Mock()
        value.executor_control = control
        control.recv.return_value = b"{"
        diagnostic = {"phase": "executor:exec-native", "error_type": "FileNotFoundError",
                      "errno": 2, "code": "os-error"}
        with patch.dict(sys.modules, {"termios": Mock()}), \
                patch.object(loader.select, "select", return_value=([control], [], [])), \
                patch.object(loader.life, "receive_json", return_value={"kind": "worker-error", "diagnostic": diagnostic}):
            with self.assertRaises(loader.WorkerFailed) as caught:
                value.wait_prompt()
        self.assertEqual(loader.safe_error(caught.exception, "loader-await-prompt"), diagnostic)

    def test_exec_failure_reports_on_open_cloexec_control_without_os_message(self):
        args = argparse.Namespace(expected_parent=12, slave_fd=42, cipher_fd=43, control_fd=44,
                                  home="root/home", agent_socket="root/agent.sock")
        control = Mock()
        order = []
        control.close.side_effect = lambda: order.append("close")
        def execute(*unused):
            self.assertIn((44, False), order)
            self.assertNotIn("close", order)
            raise FileNotFoundError(2, "private-path-and-transcript")
        def send(channel, frame):
            if frame["kind"] == "worker-error":
                self.assertNotIn("close", order)
                order.append(frame)
        with patch.dict(sys.modules, {"fcntl": Mock(), "termios": Mock(TIOCSCTTY=1)}), \
                patch.object(loader, "require_parent"), patch.object(loader.native, "child_setup"), \
                patch.object(loader.os, "setsid", create=True), patch.object(loader.os, "dup2"), \
                patch.object(loader.os, "close"), \
                patch.object(loader.os, "set_inheritable", side_effect=lambda fd, inherited: order.append((fd, inherited))), \
                patch.object(loader.life, "configure_parent_death", return_value="armed"), \
                patch.object(loader.life, "send_json", side_effect=send), \
                patch.object(loader.life, "receive_command", return_value=b"G"), \
                patch.object(loader.socket, "socket", return_value=control), \
                patch.object(loader.shutil, "which", return_value="/bin/ssh-add"), \
                patch.object(loader.os, "execve", side_effect=execute):
            with self.assertRaises(FileNotFoundError):
                loader.loader_exec(args)
        self.assertEqual(order[-2:], [{"kind": "worker-error", "diagnostic": {
            "phase": "executor:exec-native", "error_type": "FileNotFoundError", "errno": 2, "code": "os-error"}}, "close"])

    def test_exec_control_eof_allows_native_prompt_observation(self):
        value = loader.OwnedLoader(Mock())
        value.process, value.master = Mock(), 42
        value.process.poll.return_value = None
        control = Mock()
        value.executor_control = control
        control.recv.return_value = b""
        terminal = Mock(ECHO=8)
        terminal.tcgetattr.return_value = [0, 0, 0, 0]
        def output():
            value.output.extend(b"Enter passphrase for fixture: ")
        with patch.dict(sys.modules, {"termios": terminal}), \
                patch.object(loader.select, "select", side_effect=[([control], [], []), ([42], [], [])]) as select_call, \
                patch.object(value, "read_output", side_effect=output):
            value.wait_prompt()
        self.assertTrue(value.prompt_seen)
        self.assertEqual(select_call.call_args_list[1].args[0], [42])

    def setUp(self):
        bundles = patch.object(loader, "UNCERTAIN_BUNDLES", [])
        uncertain = patch.object(loader, "CLEANUP_UNCERTAIN", False)
        report = patch.object(loader, "CURRENT_REPORT", None)
        bundles.start()
        uncertain.start()
        report.start()
        self.addCleanup(bundles.stop)
        self.addCleanup(uncertain.stop)
        self.addCleanup(report.stop)

    def session(self):
        return loader.LoadingSession(Path("writer"), Path("verifier"), Path("package"), Path("root"), Mock())

    def test_failed_loader_stop_still_attempts_agent_stop_without_releasing_leases(self):
        value = loader.LoadingSession.__new__(loader.LoadingSession)
        value.loader = Mock()
        value.loader.stop.side_effect = loader.ProbeError("uncertain loader")
        value._stop_agent = Mock()
        with patch.object(loader.session.Session, "_release") as release:
            with self.assertRaises(loader.ProbeError):
                value._release()
        value._stop_agent.assert_called_once()
        release.assert_not_called()
        value.loader.close.assert_not_called()

    def test_failed_agent_stop_preserves_loader_descriptors_and_leases(self):
        value = self.session()
        value.loader = Mock()
        value._stop_agent = Mock(side_effect=OSError("uncertain agent"))
        with patch.object(loader.session.Session, "_release") as release:
            with self.assertRaises(loader.ProbeError):
                value.lock()
        value.loader.stop.assert_called_once()
        value.loader.close.assert_not_called()
        release.assert_not_called()
        self.assertEqual(value.state, "CLOSING")
        self.assertFalse(value.close_acknowledged)

    def test_release_closes_resources_only_after_both_native_exits(self):
        value = self.session()
        value.loader = Mock()
        order = []
        value.loader.stop.side_effect = lambda: order.append("loader-exit")
        value._stop_agent = Mock(side_effect=lambda: order.append("agent-exit"))
        value.loader.close.side_effect = lambda: order.append("pty-close")
        with patch.object(loader.session.Session, "_release", side_effect=lambda: order.append("lease-close")):
            value.lock()
        self.assertEqual(order, ["loader-exit", "agent-exit", "pty-close", "lease-close"])
        self.assertEqual(value.state, "CLOSED")
        self.assertTrue(value.close_acknowledged)

    def test_descriptor_ownership_is_invalidated_even_if_one_close_fails(self):
        value = loader.OwnedLoader(Mock())
        value.master, value.slave, value.pidfd = 40, 41, 42
        def close(descriptor):
            self.assertNotIn(descriptor, (value.master, value.slave, value.pidfd))
            if descriptor == 40:
                raise OSError("close failed")
        with patch.object(loader.os, "close", side_effect=close) as close_call:
            with self.assertRaises(OSError):
                value.close()
        self.assertEqual([call.args[0] for call in close_call.call_args_list], [40, 41, 42])
        self.assertEqual((value.master, value.slave, value.pidfd), (None, None, None))

    def test_live_loader_keeps_its_descriptors(self):
        value = loader.OwnedLoader(Mock())
        value.process, value.master = Mock(), 40
        with patch.object(loader.os, "close") as close:
            with self.assertRaises(loader.ProbeError):
                value.close()
        close.assert_not_called()
        self.assertEqual(value.master, 40)

    def test_handoff_requires_prompt_and_echo_disabled(self):
        value = loader.OwnedLoader(Mock())
        with patch.object(value, "suspend") as suspend, patch.object(loader.os, "write") as write:
            with self.assertRaises(loader.ProbeError):
                value.handoff()
        suspend.assert_not_called()
        write.assert_not_called()

    def test_secret_is_written_once_only_after_observed_suspend(self):
        value = loader.OwnedLoader(Mock())
        value.prompt_seen = value.echo_disabled = True
        value.master, value.pidfd = 40, 41
        order = []
        def suspended():
            value.suspended = value.handoff_suspended = True
            order.append("stopped")
        def write(descriptor, payload):
            self.assertTrue(value.suspended)
            self.assertEqual(descriptor, 40)
            self.assertEqual(payload, loader.native.PASSWORD + b"\n")
            order.append("written")
            return len(payload)
        with patch.object(value, "suspend", side_effect=suspended), \
                patch.object(loader.os, "write", side_effect=write) as write_call, \
                patch.object(loader.life, "pidfd_ready", return_value=False):
            value.handoff()
            with self.assertRaises(loader.ProbeError):
                value.handoff()
        self.assertEqual(order, ["stopped", "written"])
        write_call.assert_called_once()
        self.assertEqual(value.secret_writes, 1)

    def test_stop_observation_does_not_accept_loader_exit_as_suspension(self):
        value = loader.OwnedLoader(Mock())
        value.process, value.pidfd = Mock(pid=12), 40
        with patch.object(loader.life, "pidfd_signal"), \
                patch.object(loader.signal, "SIGSTOP", 19, create=True), \
                patch.object(loader.os, "waitpid", return_value=(12, 0), create=True), \
                patch.object(loader.os, "WNOHANG", 1, create=True), \
                patch.object(loader.os, "WUNTRACED", 2, create=True), \
                patch.object(loader.os, "WIFSTOPPED", return_value=False, create=True), \
                patch.object(loader.os, "WIFEXITED", return_value=True, create=True), \
                patch.object(loader.os, "waitstatus_to_exitcode", return_value=0, create=True):
            with self.assertRaises(loader.LoaderFailed):
                value.suspend()
        self.assertFalse(value.suspended)
        self.assertEqual(value.process.returncode, 0)

    def test_suspend_has_a_deadline(self):
        value = loader.OwnedLoader(Mock())
        value.process, value.pidfd = Mock(pid=12), 40
        with patch.object(loader.life, "pidfd_signal"), \
                patch.object(loader.signal, "SIGSTOP", 19, create=True), \
                patch.object(loader.time, "monotonic", side_effect=[0, 4]):
            with self.assertRaisesRegex(loader.ProbeError, "timed out"):
                value.suspend()

    def fd_patches(self, descriptors, *, payload=b"F", flags=0):
        connection = Mock()
        connection.recvmsg.return_value = (payload, [(loader.socket.SOL_SOCKET, 1,
                                                      array.array("i", descriptors).tobytes())], flags, None)
        return connection

    def test_multiple_or_truncated_descriptors_are_closed_and_refused(self):
        for descriptors, flags in (([81, 82], 0), ([81], 8)):
            with self.subTest(descriptors=descriptors, flags=flags):
                connection = self.fd_patches(descriptors, flags=flags)
                with patch.object(loader.socket, "SCM_RIGHTS", 1, create=True), \
                        patch.object(loader.socket, "MSG_CTRUNC", 8, create=True), \
                        patch.object(loader.socket, "CMSG_SPACE", return_value=64, create=True), \
                        patch.object(loader.os, "close") as close:
                    with self.assertRaises(loader.ProbeError):
                        loader.receive_fd(connection)
                self.assertEqual([call.args[0] for call in close.call_args_list], descriptors)

    def test_received_master_is_private_and_not_closed_on_acceptance(self):
        connection = self.fd_patches([81])
        with patch.object(loader.socket, "SCM_RIGHTS", 1, create=True), \
                patch.object(loader.socket, "MSG_CTRUNC", 8, create=True), \
                patch.object(loader.socket, "CMSG_SPACE", return_value=64, create=True), \
                patch.object(loader.os, "set_inheritable") as inherit, \
                patch.object(loader.os, "fstat", return_value=Mock(st_mode=stat.S_IFCHR)), \
                patch.object(loader.os, "isatty", return_value=True), \
                patch.object(loader.os, "ttyname", return_value="/dev/pts/ptmx", create=True), \
                patch.object(loader.os, "close") as close:
            self.assertEqual(loader.receive_fd(connection), 81)
        inherit.assert_called_once_with(81, False)
        close.assert_not_called()

    def test_rejected_descriptor_cleanup_attempts_every_received_fd(self):
        connection = self.fd_patches([81, 82])
        def close(descriptor):
            if descriptor == 81:
                raise OSError("first close failed")
        with patch.object(loader.socket, "SCM_RIGHTS", 1, create=True), \
                patch.object(loader.socket, "MSG_CTRUNC", 8, create=True), \
                patch.object(loader.socket, "CMSG_SPACE", return_value=64, create=True), \
                patch.object(loader.os, "close", side_effect=close) as close_call:
            with self.assertRaises(OSError):
                loader.receive_fd(connection)
        self.assertEqual([call.args[0] for call in close_call.call_args_list], [81, 82])

    def test_unclaimed_child_descriptor_is_closed_without_signalling(self):
        with patch.object(loader.os, "pidfd_open", return_value=81, create=True), \
                patch.object(loader.life, "process_start", return_value=99), \
                patch.object(loader.os, "close") as close, \
                patch.object(loader.life, "pidfd_signal") as signal_call:
            with self.assertRaises(loader.ProbeError):
                loader.claim_child(12, 100, 13)
        close.assert_called_once_with(81)
        signal_call.assert_not_called()

    def observer_bundle(self):
        process = Mock()
        process.poll.return_value = 0
        return {"process": process, "control": Mock(), "connection": Mock(),
                "agent_pidfd": 80, "loader_pidfd": 81, "parent_pidfd": 82, "pty_master": 83,
                "package": Path("package"), "package_identity": (1, 2),
                "root": Path("root"), "root_identity": (3, 4)}

    def test_uncertain_child_exit_retains_master_and_roots_but_attempts_both_stops(self):
        bundle = self.observer_bundle()
        backend = Mock()
        def stop(descriptor):
            if descriptor == 80:
                raise loader.life.ProbeError("agent remains alive")
        with patch.object(loader.life, "pidfd_ready", return_value=True), \
                patch.object(loader.life, "stop_agent", side_effect=stop) as stops, \
                patch.object(loader.session, "package_backend", return_value=backend), \
                patch.object(loader.os, "close") as close:
            with self.assertRaises(loader.ProbeError):
                loader.cleanup_observer(bundle)
        self.assertEqual([call.args[0] for call in stops.call_args_list], [80, 81])
        self.assertTrue(loader.CLEANUP_UNCERTAIN)
        self.assertIn(bundle, loader.UNCERTAIN_BUNDLES)
        close.assert_not_called()
        bundle["control"].close.assert_not_called()
        backend.cleanup_owned_tree.assert_not_called()
        self.assertEqual(bundle["pty_master"], 83)

    def test_uncertain_supervisor_exit_still_attempts_both_child_stops(self):
        bundle = self.observer_bundle()
        bundle["process"].poll.return_value = None
        bundle["process"].wait.side_effect = loader.subprocess.TimeoutExpired("worker", 3)
        with patch.object(loader.life, "stop_agent") as stops, patch.object(loader.os, "close") as close:
            with self.assertRaises(loader.ProbeError):
                loader.cleanup_observer(bundle)
        self.assertEqual([call.args[0] for call in stops.call_args_list], [80, 81])
        close.assert_not_called()
        self.assertTrue(loader.CLEANUP_UNCERTAIN)

    def test_confirmed_exits_release_all_descriptors_before_tree_cleanup(self):
        bundle = self.observer_bundle()
        backend = Mock()
        order = []
        with patch.object(loader.life, "pidfd_ready", return_value=True), \
                patch.object(loader.life, "stop_agent", side_effect=lambda fd: order.append(("exit", fd))), \
                patch.object(loader.os, "close", side_effect=lambda fd: order.append(("close", fd))), \
                patch.object(loader.session, "package_backend", return_value=backend):
            loader.cleanup_observer(bundle)
        self.assertEqual(order[:2], [("exit", 80), ("exit", 81)])
        self.assertEqual([value for kind, value in order if kind == "close"], [80, 81, 82, 83])
        self.assertFalse(loader.CLEANUP_UNCERTAIN)
        self.assertEqual(loader.UNCERTAIN_BUNDLES, [])
        self.assertEqual(backend.cleanup_owned_tree.call_count, 2)

    def test_non_pty_descriptor_is_closed_without_acceptance(self):
        connection = self.fd_patches([81])
        with patch.object(loader.socket, "SCM_RIGHTS", 1, create=True), \
                patch.object(loader.socket, "MSG_CTRUNC", 8, create=True), \
                patch.object(loader.socket, "CMSG_SPACE", return_value=64, create=True), \
                patch.object(loader.os, "set_inheritable"), \
                patch.object(loader.os, "fstat", return_value=Mock(st_mode=stat.S_IFREG)), \
                patch.object(loader.os, "close") as close:
            with self.assertRaises(loader.ProbeError):
                loader.receive_fd(connection)
        close.assert_called_once_with(81)

    def test_parent_mismatch_refuses_before_child_setup_or_exec(self):
        args = argparse.Namespace(expected_parent=12, control_fd=44)
        with patch.dict(sys.modules, {"fcntl": Mock(), "termios": Mock()}), \
                patch.object(loader.socket, "socket", return_value=Mock()), \
                patch.object(loader.life, "send_json"), \
                patch.object(loader.os, "getppid", return_value=13), \
                patch.object(loader.native, "child_setup") as setup, \
                patch.object(loader.os, "execve") as execute:
            with self.assertRaises(loader.life.ParentChanged):
                loader.loader_exec(args)
        setup.assert_not_called()
        execute.assert_not_called()

    def test_native_exec_arguments_and_environment_do_not_contain_password(self):
        args = argparse.Namespace(expected_parent=12, slave_fd=42, cipher_fd=43, control_fd=44,
                                  home="root/home", agent_socket="root/agent.sock")
        control = Mock()
        with patch.dict(sys.modules, {"fcntl": Mock(), "termios": Mock(TIOCSCTTY=1)}), \
                patch.object(loader, "require_parent"), patch.object(loader.native, "child_setup"), \
                patch.object(loader.os, "setsid", create=True), patch.object(loader.os, "dup2"), \
                patch.object(loader.os, "close"), patch.object(loader.os, "set_inheritable"), \
                patch.object(loader.life, "configure_parent_death", return_value="armed"), \
                patch.object(loader.life, "send_json"), \
                patch.object(loader.life, "receive_command", return_value=b"G"), \
                patch.object(loader.socket, "socket", return_value=control), \
                patch.object(loader.shutil, "which", return_value="/bin/ssh-add"), \
                patch.object(loader.os, "execve", side_effect=RuntimeError("exec intercepted")) as execute:
            with self.assertRaisesRegex(RuntimeError, "intercepted"):
                loader.loader_exec(args)
        executable, arguments, environment = execute.call_args.args
        self.assertEqual(arguments, [executable, "-k", "/proc/self/fd/43"])
        self.assertNotIn(loader.native.PASSWORD.decode("ascii"), json_text(arguments, environment))
        control.close.assert_called_once()

    def test_parent_race_after_arming_prevents_native_exec(self):
        args = argparse.Namespace(expected_parent=12, slave_fd=42, cipher_fd=43, control_fd=44,
                                  home="root/home", agent_socket="root/agent.sock")
        with patch.dict(sys.modules, {"fcntl": Mock(), "termios": Mock(TIOCSCTTY=1)}), \
                patch.object(loader.socket, "socket", return_value=Mock()), \
                patch.object(loader.life, "send_json"), \
                patch.object(loader.os, "getppid", side_effect=[12, 12, 13]), \
                patch.object(loader.native, "child_setup"), \
                patch.object(loader.os, "setsid", create=True), patch.object(loader.os, "dup2"), \
                patch.object(loader.os, "close"), \
                patch.object(loader.life, "configure_parent_death", return_value="armed") as arm, \
                patch.object(loader.os, "execve") as execute:
            with self.assertRaises(loader.life.ParentChanged):
                loader.loader_exec(args)
        arm.assert_called_once()
        execute.assert_not_called()

    def test_cancel_at_prompt_does_not_handoff_a_secret(self):
        value = self.session()
        value._validate = Mock()
        value.agent = Mock(pid=10)
        value.agent_start = 1
        value.loader = Mock(master=40, start_time=1, echo_disabled=True)
        value.loader.process.pid = 11
        with patch.object(loader, "send_fd"), patch.object(loader.life, "send_json"), \
                patch.object(loader, "receive", return_value={"command": "cancel"}):
            with self.assertRaises(loader.Cancelled):
                value._load_agent()
        value.loader.handoff.assert_not_called()
        self.assertEqual(value.last_seam, "prompt")

    def test_early_failure_cannot_satisfy_negative_case(self):
        final = {"kind": "final", "outcome": "failed", "selected_seam": "prompt",
                 "readiness_published": False, "close_acknowledged": True,
                 "resources_closed_after_exit": True}
        with self.assertRaises(loader.ProbeError):
            loader.require_final(final, "cancel-at-prompt", "prompt", "cancelled")

    def test_primary_failure_survives_secondary_observer_cleanup_failure(self):
        primary = loader.ProbeError("native loader prompt left terminal echo enabled")
        with patch.object(loader, "CURRENT_REPORT", {}), \
                patch.object(loader, "cleanup_observer", side_effect=loader.ProbeError("uncertain cleanup")):
            loader.finish_observer({}, primary, "await-master")
            self.assertEqual(loader.CURRENT_REPORT["primary_failure"]["code"], "prompt-echo-enabled")
            self.assertEqual(loader.CURRENT_REPORT["cleanup_failure"]["error_type"], "ProbeError")

    def test_worker_error_before_master_is_not_misparsed_as_a_descriptor(self):
        connection = Mock()
        connection.recvmsg.return_value = (b"E", [], 0, None)
        diagnostic = {"phase": "executor:setsid", "error_type": "PermissionError", "errno": 1, "code": "os-error"}
        with patch.object(loader.socket, "CMSG_SPACE", return_value=64, create=True), \
                patch.object(loader.socket, "MSG_CTRUNC", 8, create=True), \
                patch.object(loader.life, "receive_json", return_value={"kind": "worker-error", "diagnostic": diagnostic}):
            with self.assertRaises(loader.WorkerFailed) as failure:
                loader.receive_fd(connection)
        self.assertEqual(failure.exception.diagnostic, diagnostic)

    def test_diagnostic_does_not_copy_os_error_message_or_unknown_worker_text(self):
        diagnostic = loader.safe_error(PermissionError(13, "private-content-not-for-report"), "executor:setsid")
        self.assertNotIn("private-content", json_text(diagnostic))
        self.assertEqual(diagnostic["errno"], 13)
        diagnostic["code"] = "private-content-not-for-report"
        with self.assertRaises(loader.ProbeError):
            loader.validate_diagnostic(diagnostic)

    def test_startup_diagnostic_runs_only_prompt_cancel_and_never_observes_a_matrix(self):
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as patches:
            root = Path(temporary) / "root"
            root.mkdir()
            backend = Mock()
            sentinel = Mock()
            sentinel.poll.return_value = None
            parsed = {"public_blob": b"public"}
            for name in ("require_isolation", "require_unprivileged", "wait_socket"):
                patches.enter_context(patch.object(loader.life, name))
            patches.enter_context(patch.object(loader.life, "prctl", return_value=1))
            patches.enter_context(patch.object(loader.life, "require_resources", return_value={"bounded": True}))
            patches.enter_context(patch.object(loader.session, "require_store"))
            patches.enter_context(patch.object(loader.session, "package_backend", return_value=backend))
            patches.enter_context(patch.object(loader.tempfile, "mkdtemp", return_value=str(root)))
            patches.enter_context(patch.object(loader.life, "create_fixture", return_value=(
                "agent", "add", "comment", parsed, {}, b"message", {})))
            patches.enter_context(patch.object(loader.life, "owned_sentinel", return_value=nullcontext((sentinel, 10))))
            patches.enter_context(patch.object(loader.life, "pidfd_ready", return_value=False))
            patches.enter_context(patch.object(loader.native, "pty_run", return_value=(0, b"")))
            patches.enter_context(patch.object(loader.native, "sign", return_value=b"signature"))
            patches.enter_context(patch.object(loader.native, "verify"))
            patches.enter_context(patch.object(loader.native, "proof", return_value={}))
            cases = patches.enter_context(patch.object(loader, "run_case", return_value={"case": "cancel-at-prompt"}))
            report = loader.run_probe(Path("writer"), Path("verifier"), Path("/store/run"), diagnostic_startup=True)
        self.assertEqual(cases.call_args.args[-1], "cancel-at-prompt")
        cases.assert_called_once()
        self.assertEqual(report["status"], "incomplete")
        self.assertTrue(report["diagnostic_only"])
        self.assertFalse(report["loader_lifecycle_tested"])
        self.assertTrue(report["diagnostic_startup_completed"])


def json_text(*values):
    import json
    return json.dumps(values)


if __name__ == "__main__":
    unittest.main()
