"""Lifecycle regression controls using metadata and mocks, never native programs."""

from contextlib import ExitStack
from pathlib import Path
import json
import signal
import socket
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import lifecycle_probe as probe


class LifecycleTests(unittest.TestCase):
    def test_executable_alias_is_accepted_only_for_the_same_strict_target(self):
        credentials = {"PPid": [12], "NoNewPrivs": [1], "Uid": [7] * 4, "Gid": [8] * 4}
        targets = {"/bin/ssh-add": Path("/usr/bin/ssh-add"),
                   "/usr/bin/ssh-add": Path("/usr/bin/ssh-add"),
                   "/usr/bin/other": Path("/usr/bin/other")}
        def resolve(path, *, strict=False):
            self.assertTrue(strict)
            return targets[path.as_posix()]
        with patch.object(probe, "process_start", return_value=5), \
                patch.object(probe, "executable", return_value="/usr/bin/ssh-add") as executable, \
                patch.object(probe.Path, "resolve", autospec=True, side_effect=resolve), \
                patch.object(probe, "agent_credentials", return_value=credentials), \
                patch.object(probe.os, "geteuid", return_value=7, create=True), \
                patch.object(probe.os, "getegid", return_value=8, create=True):
            self.assertEqual(probe.inspect_agent(13, 5, "/bin/ssh-add", 12)["executable_observation"], "verified")
            executable.return_value = "/usr/bin/other"
            with self.assertRaisesRegex(probe.ProbeError, "executable identity changed"):
                probe.inspect_agent(13, 5, "/bin/ssh-add", 12)

    def setUp(self):
        self.addCleanup(patch.stopall)
        patch.object(probe.signal, "SIGKILL", 9, create=True).start()
        patch.object(probe.os, "WNOHANG", 1, create=True).start()

    def test_barrier_sets_a_deadline_before_reading(self):
        channel = Mock()
        channel.recv.side_effect = socket.timeout()
        with self.assertRaises(socket.timeout):
            probe.receive_command(channel)
        channel.settimeout.assert_called_once_with(probe.TIMEOUT)

    def test_reap_refuses_a_child_that_never_exits(self):
        with patch.object(probe.os, "waitpid", return_value=(0, 0), create=True), \
                patch.object(probe.Path, "exists", return_value=True), \
                patch.object(probe.time, "monotonic", side_effect=[0, 0, 4]), \
                patch.object(probe.time, "sleep"):
            with self.assertRaisesRegex(probe.ProbeError, "reap.*timed out"):
                probe.reap_owned(123)

    def test_handshake_failure_stops_supervisor_and_closes_channels(self):
        observer, supervisor = Mock(), Mock()
        process = Mock(pid=123)
        process.poll.return_value = None
        with patch.object(probe.socket, "socketpair", return_value=(observer, supervisor)), \
                patch.object(probe.os, "set_inheritable"), \
                patch.object(probe.subprocess, "Popen", return_value=process), \
                patch.object(probe, "receive_json", side_effect=probe.ProbeError("handshake")):
            with self.assertRaisesRegex(probe.ProbeError, "handshake"):
                probe.launch_supervisor(Path("fixture"), Path("socket"), "case")
        process.kill.assert_called_once()
        observer.close.assert_called()
        supervisor.close.assert_called()

    def test_pidfd_failure_stops_supervisor_and_keeps_closing_resources(self):
        observer, supervisor = Mock(), Mock()
        process = Mock(pid=123)
        process.poll.return_value = None
        with patch.object(probe.socket, "socketpair", return_value=(observer, supervisor)), \
                patch.object(probe.os, "set_inheritable"), \
                patch.object(probe.subprocess, "Popen", return_value=process), \
                patch.object(probe, "receive_json", return_value={"kind": "prearm", "pid": 124, "parent": 123}), \
                patch.object(probe, "process_start", return_value=7), \
                patch.object(probe.os, "pidfd_open", side_effect=OSError("no fd"), create=True), \
                patch.object(probe, "reap_owned"):
            with self.assertRaises(OSError):
                probe.launch_supervisor(Path("fixture"), Path("socket"), "case")
        process.kill.assert_called_once()
        observer.close.assert_called()

    def test_direct_death_requires_sigkill_report_from_the_live_parent(self):
        control = Mock()
        with patch.object(probe, "receive_json", return_value={"kind": "agent-status", "pid": 124, "status": 9}), \
                patch.object(probe.os, "WIFSIGNALED", return_value=True, create=True), \
                patch.object(probe.os, "WTERMSIG", return_value=signal.SIGKILL, create=True):
            self.assertEqual(probe.parent_agent_status(control, 124), 9)
        control.sendall.assert_called_once_with(b"R")
        with patch.object(probe, "receive_json", return_value={"kind": "agent-status", "pid": 125, "status": 9}):
            with self.assertRaises(probe.ProbeError):
                probe.parent_agent_status(control, 124)

    def resource_fixture(self, root, version=2):
        proc = root / "proc"
        (proc / "self").mkdir(parents=True)
        group = root / "cgroup"
        group.mkdir()
        if version == 2:
            (proc / "self" / "cgroup").write_text("0::/\n", encoding="ascii")
            (proc / "self" / "mountinfo").write_text(
                f"1 0 0:1 / {group.as_posix()} rw - cgroup2 cgroup rw\n", encoding="ascii")
            values = {"memory.max": "536870912", "memory.swap.max": "0",
                      "pids.max": "128", "cpu.max": "100000 100000"}
        else:
            (proc / "self" / "cgroup").write_text("1:memory,cpu,pids:/docker/fixture\n", encoding="ascii")
            (proc / "self" / "mountinfo").write_text(
                f"1 0 0:1 /docker/fixture {group.as_posix()} rw - cgroup cgroup rw,memory,cpu,pids\n", encoding="ascii")
            values = {"memory.limit_in_bytes": "536870912", "memory.memsw.limit_in_bytes": "536870912",
                      "pids.max": "128", "cpu.cfs_quota_us": "100000", "cpu.cfs_period_us": "100000"}
        for name, value in values.items():
            (group / name).write_text(value, encoding="ascii")
        return proc, group

    def resource_patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(probe.os, "getpid", return_value=1))
        stack.enter_context(patch.object(probe.os, "statvfs", return_value=SimpleNamespace(
            f_blocks=32768, f_frsize=4096), create=True))
        return stack

    def test_bounded_v1_and_v2_metadata_are_accepted(self):
        for version in (1, 2):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as temporary:
                proc, _ = self.resource_fixture(Path(temporary), version)
                with self.resource_patches():
                    result = probe.require_resources(proc_root=proc)
                self.assertEqual(result["memory_bytes"], 536870912)
                self.assertEqual(result["pids"], 128)

    def test_session_deadline_script_is_explicitly_allowed(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc, _ = self.resource_fixture(Path(temporary))
            with self.resource_patches():
                probe.require_resources(proc_root=proc, expected_script="session_probe.py")
                probe.require_resources(proc_root=proc, expected_script="concurrent_probe.py")
                probe.require_resources(proc_root=proc, expected_script="loader_probe.py")
                probe.require_resources(proc_root=proc, expected_script="rust_abandon_probe.py")
                probe.require_resources(proc_root=proc, expected_script="rust_isolation_probe.py")
                probe.require_resources(proc_root=proc, expected_script="rust_ipc_probe.py")
                probe.require_resources(proc_root=proc, expected_script="rust_provision_probe.py")
                with self.assertRaisesRegex(probe.ProbeError, "allowlist"):
                    probe.require_resources(proc_root=proc, expected_script="unreviewed.py")

    def test_fixed_namespace_deadlines_bind_phase_without_relaxing_credentials(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc, _ = self.resource_fixture(Path(temporary))
            parent = proc / "1"
            parent.mkdir()
            executable = parent / "exe"
            executable.write_bytes(b"deadline fixture")
            (parent / "status").write_text("Uid:\t65534 65534 65534 65534\nNoNewPrivs:\t1\n", encoding="ascii")
            for phase, deadline in (("setup", "120s"), ("reader", "120s"), ("mutate", "30s")):
                arguments = [str(executable), "--signal=TERM", "--kill-after=5s", deadline,
                             "python3", "-B", "/probe/rust_fixed_root_probe.py", "--phase", phase]
                with self.resource_patches(), patch.object(probe.os, "getpid", return_value=2), \
                        patch.object(probe.os, "getppid", return_value=1), \
                        patch.object(probe.os, "geteuid", return_value=65534, create=True), \
                        patch.object(probe.shutil, "which", return_value=str(executable)):
                    (parent / "cmdline").write_bytes(b"\0".join(item.encode("ascii") for item in arguments) + b"\0")
                    probe.require_resources(proc_root=proc, expected_script="rust_fixed_root_probe.py")
                    arguments[3] = "240s"
                    (parent / "cmdline").write_bytes(b"\0".join(item.encode("ascii") for item in arguments) + b"\0")
                    with self.assertRaisesRegex(probe.ProbeError, "deadline command"):
                        probe.require_resources(proc_root=proc, expected_script="rust_fixed_root_probe.py")

    def test_resident_deadlines_bind_phase_without_relaxing_credentials(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc, _ = self.resource_fixture(Path(temporary))
            parent = proc / "1"
            parent.mkdir()
            executable = parent / "exe"
            executable.write_bytes(b"deadline fixture")
            (parent / "status").write_text("Uid:\t65534 65534 65534 65534\nNoNewPrivs:\t1\n", encoding="ascii")
            for phase, deadline in (("setup", "120s"), ("reader", "120s"), ("mutate", "30s")):
                arguments = [str(executable), "--signal=TERM", "--kill-after=5s", deadline,
                             "python3", "-B", "/probe/rust_resident_probe.py", "--phase", phase]
                with self.resource_patches(), patch.object(probe.os, "getpid", return_value=2), \
                        patch.object(probe.os, "getppid", return_value=1), \
                        patch.object(probe.os, "geteuid", return_value=65534, create=True), \
                        patch.object(probe.shutil, "which", return_value=str(executable)):
                    (parent / "cmdline").write_bytes(b"\0".join(item.encode("ascii") for item in arguments) + b"\0")
                    probe.require_resources(proc_root=proc, expected_script="rust_resident_probe.py")
                    arguments[3] = "240s"
                    (parent / "cmdline").write_bytes(b"\0".join(item.encode("ascii") for item in arguments) + b"\0")
                    with self.assertRaisesRegex(probe.ProbeError, "deadline command"):
                        probe.require_resources(proc_root=proc, expected_script="rust_resident_probe.py")

    def test_unbounded_limits_are_refused_before_any_native_program(self):
        for name, value in (("memory.max", "max"), ("memory.swap.max", "1"),
                            ("cpu.max", "max 100000"), ("pids.max", "129")):
            with self.subTest(limit=name), tempfile.TemporaryDirectory() as temporary:
                proc, group = self.resource_fixture(Path(temporary))
                (group / name).write_text(value, encoding="ascii")
                with self.resource_patches(), patch.object(probe.subprocess, "Popen") as popen:
                    with self.assertRaises(probe.ProbeError):
                        probe.require_resources(proc_root=proc)
                    popen.assert_not_called()

    def test_observer_must_be_pid_one(self):
        with patch.object(probe.os, "getpid", return_value=77), \
                patch.object(probe.os, "getppid", return_value=88):
            with self.assertRaisesRegex(probe.ProbeError, "PID"):
                probe.require_resources()

    def test_exact_private_timeout_parent_is_accepted(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc, _ = self.resource_fixture(Path(temporary))
            (proc / "1").mkdir()
            command = [b"/usr/bin/timeout", b"--signal=TERM", b"--kill-after=5s", b"240s",
                       b"python3", b"-B", b"/probe/lifecycle_probe.py", b"--writer", b"/writer"]
            (proc / "1" / "cmdline").write_bytes(b"\0".join(command) + b"\0")
            (proc / "1" / "status").write_text("Uid:\t65534 65534 65534 65534\nNoNewPrivs:\t1\n", encoding="ascii")
            with self.resource_patches(), \
                    patch.object(probe.os, "getpid", return_value=2), \
                    patch.object(probe.os, "getppid", return_value=1), \
                    patch.object(probe.os, "geteuid", return_value=65534, create=True), \
                    patch.object(probe.shutil, "which", return_value="/bin/timeout"), \
                    patch.object(probe.Path, "resolve", return_value=Path("/usr/bin/timeout")):
                probe.require_resources(proc_root=proc)
                (proc / "1" / "cmdline").write_bytes(b"\0".join(command[:3] + [b"999s"] + command[4:]))
                with self.assertRaisesRegex(probe.ProbeError, "deadline command"):
                    probe.require_resources(proc_root=proc)

    def test_failed_cleanup_still_closes_pidfd_and_socket(self):
        control = Mock()
        with patch.object(probe, "stop_supervisor", side_effect=probe.ProbeError("stop")), \
                patch.object(probe, "stop_agent"), \
                patch.object(probe.os, "close") as close, \
                patch.object(probe, "reap_owned") as reap, \
                patch.object(probe, "unlink_owned_socket") as unlink:
            with self.assertRaisesRegex(probe.ProbeError, "stop"):
                probe.cleanup_supervisor(Mock(), control, 12, 13, Path("socket"))
        control.close.assert_called_once()
        close.assert_called_once_with(13)
        reap.assert_called_once_with(12)
        unlink.assert_called_once_with(Path("socket"))

    def test_sentinel_pidfd_failure_stops_the_just_created_process(self):
        with patch.object(probe.subprocess, "Popen") as popen, \
                patch.object(probe.os, "pidfd_open", side_effect=OSError("no fd"), create=True), \
                patch.object(probe.native, "stop") as stop, \
                patch.object(probe, "unlink_owned_socket") as unlink:
            with self.assertRaises(OSError):
                with probe.owned_sentinel("agent", Path("fixture"), Path("socket")):
                    self.fail("failed pidfd must not transfer ownership")
        stop.assert_called_once_with(popen.return_value)
        unlink.assert_called_once_with(Path("socket"))

    def test_failure_report_keeps_completed_cases(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            writer, verifier, report = root / "writer", root / "verifier", root / "report"
            writer.touch()
            verifier.touch()

            def fail_after_first_case(*_):
                probe.CURRENT_REPORT = {"cases": [{"case": "normal-return", "agent_dead": True}]}
                raise probe.ProbeError("later case failed")

            with patch.object(probe.sys, "argv", ["probe", "--writer", str(writer), "--verifier", str(verifier),
                                                 "--report", str(report)]), \
                    patch.object(probe.sys, "platform", "linux"), \
                    patch.dict(probe.os.environ, {"VIVAC_OPENSSH_DISPOSABLE": "1"}), \
                    patch.object(probe, "require_isolation"), patch.object(probe, "require_unprivileged"), \
                    patch.object(probe, "prctl", return_value=1), patch.object(probe.os, "access", return_value=True), \
                    patch.object(probe, "run_probe", side_effect=fail_after_first_case), \
                    patch.object(probe.sys, "stderr"):
                self.assertEqual(probe.main(), 1)
            payload = json.loads(report.read_text(encoding="ascii"))
            self.assertEqual(payload["status"], "incomplete")
            self.assertEqual(payload["cases"], [{"case": "normal-return", "agent_dead": True}])
            self.assertFalse(payload["custody_accredited"])

    def test_only_the_unlinked_control_skips_parent_death_signal(self):
        for unlinked in (False, True):
            with self.subTest(unlinked=unlinked):
                def read_setting(option, target, *_):
                    self.assertEqual(option, probe.PR_GET_PDEATHSIG)
                    target._obj.value = 0 if unlinked else signal.SIGKILL
                    return 0

                with patch.object(probe, "prctl") as setting, \
                        patch.object(probe.ctypes, "CDLL") as libc:
                    libc.return_value.prctl.side_effect = read_setting
                    kind = probe.configure_parent_death(unlinked_control=unlinked)
                setting.assert_any_call(probe.PR_SET_NO_NEW_PRIVS, 1)
                if unlinked:
                    self.assertEqual(kind, "unlinked")
                    self.assertEqual(setting.call_count, 1)
                else:
                    self.assertEqual(kind, "armed")
                    setting.assert_any_call(probe.PR_SET_PDEATHSIG, signal.SIGKILL)

    def unlinked_observation_patches(self, *, dead=False):
        stack = ExitStack()
        stack.enter_context(patch.object(probe, "pidfd_ready", return_value=dead))
        retained = stack.enter_context(patch.object(probe, "signature_on_connection", return_value=b"r" * 64))
        new = stack.enter_context(patch.object(probe, "open_signed_connection", return_value=(Mock(), b"n" * 64)))
        verifier = stack.enter_context(patch.object(probe.native, "verify"))
        proof = stack.enter_context(patch.object(probe.native, "proof", side_effect=lambda _, signature: signature))
        return stack, retained, new, verifier, proof

    def observe_unlinked(self):
        return probe.observe_unlinked_agent(Path("fixture"), "native-unlinked-parent-kill", 13, Mock(),
                                            Path("agent.sock"), b"public", "comment", b"message",
                                            {"verifier": Path("verifier")}, {}, {})

    def test_unlinked_survival_requires_both_cryptographic_verifications(self):
        stack, retained, new, verifier, proof = self.unlinked_observation_patches()
        with stack:
            result = self.observe_unlinked()
        self.assertTrue(result["native_agent_survived_parent"])
        self.assertTrue(result["unsafe_for_custody"])
        self.assertEqual(retained.call_count, 1)
        self.assertEqual(new.call_count, 1)
        self.assertEqual(verifier.call_count, 2)
        self.assertEqual([call.args[4] for call in verifier.call_args_list],
                         ["native-unlinked-parent-kill-retained-after", "native-unlinked-parent-kill-new-after"])
        self.assertEqual([call.args[1] for call in proof.call_args_list], [b"r" * 64, b"n" * 64])

    def test_unlinked_control_refuses_dead_agent(self):
        stack, retained, _, verifier, _ = self.unlinked_observation_patches(dead=True)
        with stack:
            with self.assertRaisesRegex(probe.ProbeError, "did not survive"):
                self.observe_unlinked()
        retained.assert_not_called()
        verifier.assert_not_called()

    def test_unlinked_control_refuses_rejected_or_invalid_signature(self):
        for failure in ("transport", "verification"):
            with self.subTest(failure=failure):
                stack, retained, _, verifier, _ = self.unlinked_observation_patches()
                if failure == "transport":
                    retained.side_effect = probe.ProbeError("signature refused")
                else:
                    verifier.side_effect = probe.native.ProbeError("invalid signature")
                with stack:
                    with self.assertRaises((probe.ProbeError, probe.native.ProbeError)):
                        self.observe_unlinked()

    def test_signature_type_byte_without_payload_is_not_a_signature(self):
        with patch.object(probe, "exchange", return_value=b"\x0e"):
            with self.assertRaises((probe.ProbeError, probe.native.ProbeError)):
                probe.signature_on_connection(Mock(), b"public", b"message")

    def test_unlinked_control_refuses_an_inherited_parent_death_signal(self):
        def unexpected_setting(_, target, *__):
            target._obj.value = signal.SIGKILL
            return 0

        with patch.object(probe, "prctl"), patch.object(probe.ctypes, "CDLL") as libc:
            libc.return_value.prctl.side_effect = unexpected_setting
            with self.assertRaisesRegex(probe.ProbeError, "unexpectedly"):
                probe.configure_parent_death(unlinked_control=True)

    def test_unlinked_switch_is_only_forwarded_for_the_explicit_control(self):
        for unlinked in (False, True):
            with self.subTest(unlinked=unlinked):
                channels = Mock(), Mock()
                process = Mock(pid=123)
                with patch.object(probe.socket, "socketpair", return_value=channels), \
                        patch.object(probe.subprocess, "Popen", return_value=process) as popen, \
                        patch.object(probe.os, "pidfd_open", return_value=13, create=True), \
                        patch.object(probe, "process_start", return_value=7), \
                        patch.object(probe, "receive_json", side_effect=[
                            {"kind": "prearm", "pid": 124, "parent": 123},
                            {"kind": "executor", "pid": 124, "parent": 123}]):
                    probe.launch_supervisor(Path("fixture"), Path("socket"), "case", unlinked_control=unlinked)
                self.assertEqual("--unlinked-control" in popen.call_args.args[0], unlinked)

    def test_unwritable_report_directory_refuses_before_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            writer, verifier, report = root / "writer", root / "verifier", root / "report"
            writer.touch()
            verifier.touch()
            with patch.object(probe.sys, "argv", ["probe", "--writer", str(writer), "--verifier", str(verifier),
                                                 "--report", str(report)]), \
                    patch.object(probe.sys, "platform", "linux"), \
                    patch.dict(probe.os.environ, {"VIVAC_OPENSSH_DISPOSABLE": "1"}), \
                    patch.object(probe, "require_isolation"), patch.object(probe, "require_unprivileged"), \
                    patch.object(probe, "prctl", return_value=1), \
                    patch.object(probe.os, "access", side_effect=lambda _, mode: mode == probe.os.X_OK), \
                    patch.object(probe, "run_probe") as run, patch.object(probe.sys, "stderr"):
                self.assertEqual(probe.main(), 1)
            run.assert_not_called()
            self.assertIn("not writable", json.loads(report.read_text())["failure"])

    def test_top_level_unlinked_switch_is_refused(self):
        with patch.object(probe.sys, "argv", ["probe", "--unlinked-control"]), \
                patch.object(probe.sys, "platform", "linux"), \
                patch.dict(probe.os.environ, {"VIVAC_OPENSSH_DISPOSABLE": "1"}), \
                patch.object(probe, "require_isolation"), patch.object(probe, "require_unprivileged"), \
                patch.object(probe, "prctl", return_value=1), \
                patch.object(probe, "run_probe") as run, patch.object(probe.sys, "stderr"):
            self.assertEqual(probe.main(), 1)
        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
