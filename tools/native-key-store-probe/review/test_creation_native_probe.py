"""Protocol guards for native rooted draft observations."""
import copy
import hashlib
import json
import signal
import subprocess
import sys
import os
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import creation_draft_probe as draft
import creation_native_probe as probe


class PacketTests(unittest.TestCase):
    def test_operational_measurement_requires_normal_ssh_and_exact_bounded_durations(self):
        args = SimpleNamespace(measure_operational_stages=True, keeper_ssh_release="normal")
        self.assertTrue(probe.validate_operational_measurement_selection(args))
        self.assertFalse(probe.validate_operational_measurement_selection(SimpleNamespace()))
        for mode in (None, "disconnect"):
            with self.subTest(mode=mode), self.assertRaises(Exception):
                probe.validate_operational_measurement_selection(SimpleNamespace(
                    measure_operational_stages=True, keeper_ssh_release=mode))
        measurement = {"clock": "monotonic-ns",
                       "creation": dict.fromkeys(probe.CREATION_MEASUREMENT_FIELDS, 0),
                       "ssh": dict.fromkeys(probe.SSH_MEASUREMENT_FIELDS, 240_000_000_000)}
        self.assertEqual(probe.validate_operational_measurement(measurement), measurement)
        for section in ("creation", "ssh"):
            field = next(iter(measurement[section]))
            for invalid in (-1, 240_000_000_001, True, 1.0, "1", None):
                changed = copy.deepcopy(measurement)
                changed[section][field] = invalid
                with self.subTest(section=section, invalid=invalid), self.assertRaises(Exception):
                    probe.validate_operational_measurement(changed)
            for mutation in ("missing", "extra"):
                changed = copy.deepcopy(measurement)
                if mutation == "missing":
                    del changed[section][field]
                else:
                    changed[section]["extra"] = 0
                with self.subTest(section=section, mutation=mutation), self.assertRaises(Exception):
                    probe.validate_operational_measurement(changed)
        for changed in ({**measurement, "clock": "wall"}, {**measurement, "extra": 0}):
            with self.assertRaises(Exception):
                probe.validate_operational_measurement(changed)

    def test_keeper_ssh_release_requires_the_real_producer_and_exclusive_mode(self):
        args = SimpleNamespace(keeper_ssh_release="normal", publish_initial_opening_permit=True,
                               case="publish-complete", reader_check="stable", mount_check=None,
                               reader=Path("/reader"))
        self.assertEqual(probe.validate_keeper_ssh_release_selection(args), "normal")
        for changed in (dict(publish_initial_opening_permit=False), dict(case="publish-record-before-rename-failure"),
                        dict(consume_initial_opening_permit="normal"), dict(keeper_opening_release="normal"),
                        dict(endpoint_active=True), dict(opening_permit_check=True), dict(keeper_ssh_release="unknown")):
            with self.subTest(changed=changed), self.assertRaises(Exception):
                probe.validate_keeper_ssh_release_selection(SimpleNamespace(**{**vars(args), **changed}))

    def test_keeper_ssh_release_requires_complete_transport_and_reopening_evidence(self):
        for mode in ("normal", "disconnect", "unconfirmed"):
            evidence = dict(mode=mode, ssh_pty_verified=True, no_pty_permit_preserved=True, predecessor_signed=True,
                            supervisor_start_refused_foreign_inputs=True,
                            initial_permit_consumed=True, reconnect_refused_before_prompt=True,
                            descendant_exits_observed=True, clean_close_published=mode == "normal",
                            successor_signature_verified=mode == "normal",
                            disconnect_observed=mode == "disconnect",
                            unconfirmed_closure_refused=mode == "unconfirmed", authority=False)
            self.assertEqual(probe.validate_keeper_ssh_release(evidence), evidence)
            for field, original in evidence.items():
                with self.subTest(mode=mode, field=field), self.assertRaises(Exception):
                    probe.validate_keeper_ssh_release({key: value for key, value in evidence.items() if key != field})
                changed = dict(evidence, **{field: not original if type(original) is bool else "unknown"})
                with self.subTest(mode=mode, changed=field), self.assertRaises(Exception):
                    probe.validate_keeper_ssh_release(changed)
            with self.assertRaises(Exception):
                probe.validate_keeper_ssh_release(dict(evidence, extra=True))

    def test_keeper_opening_release_observation_requires_new_generation_and_missing_retry(self):
        args = SimpleNamespace(keeper_opening_release="normal", publish_initial_opening_permit=True,
                               case="publish-complete", reader_check="stable", mount_check=None,
                               reader=Path("/reader"))
        with tempfile.TemporaryDirectory() as directory, patch.object(draft, "USER", Path(directory)):
            with patch.object(probe, "observe_active_endpoint_case", return_value="a" * 64) as active, \
                 patch.object(probe, "observe_endpoint_proof", return_value={
                     "generation": "b" * 64, "signature_verified": True}) as proof, \
                 patch.object(probe, "observe_opening_refusal") as refusal:
                evidence = probe.observe_keeper_opening_release(args, "credential", b"secret", None, {})
                self.assertTrue(evidence["distinct_generation_verified"])
                self.assertEqual(active.call_args.args[-1], "permit-close")
                self.assertTrue(proof.call_args.kwargs["permitted"])
                self.assertEqual(refusal.call_args.args[-1], "NotFound")
                proof.return_value = {"generation": "a" * 64, "signature_verified": True}
                with self.assertRaisesRegex(Exception, "reused"):
                    probe.observe_keeper_opening_release(args, "credential", b"secret", None, {})
            for mode, case in (("missing", "permit-missing"), ("keeper-lost", "permit-keeper-lost")):
                args.keeper_opening_release = mode
                with patch.object(probe, "observe_active_endpoint_case", return_value="a" * 64) as active, \
                     patch.object(probe, "observe_endpoint_proof") as proof, \
                     patch.object(probe, "observe_opening_refusal") as refusal:
                    evidence = probe.observe_keeper_opening_release(args, "credential", b"secret", None, {})
                    self.assertFalse(evidence["clean_close_published"])
                    self.assertEqual(active.call_args.args[-1], case)
                    proof.assert_not_called()
                    self.assertEqual(refusal.call_args.args[-1], "NotFound")

    def test_keeper_opening_release_requires_exact_mode_and_verified_evidence(self):
        args = SimpleNamespace(keeper_opening_release="normal", publish_initial_opening_permit=True,
                               case="publish-complete", reader_check="stable", mount_check=None,
                               reader=Path("/reader"))
        self.assertEqual(probe.validate_keeper_opening_release_selection(args), "normal")
        for changed in (dict(publish_initial_opening_permit=False), dict(consume_initial_opening_permit="normal"),
                        dict(endpoint_active=True), dict(keeper_opening_release="unknown")):
            with self.subTest(changed=changed), self.assertRaises(Exception):
                probe.validate_keeper_opening_release_selection(SimpleNamespace(**{**vars(args), **changed}))
        for mode in ("normal", "missing", "keeper-lost"):
            evidence = dict(mode=mode, predecessor_signed=True, consumed_before_input=True,
                            terminal_restored=True, missing_permit_refused=True,
                            clean_close_published=mode == "normal", successor_signature_verified=mode == "normal",
                            distinct_generation_verified=mode == "normal",
                            keeper_death_observed=mode == "keeper-lost", authority=False)
            self.assertEqual(probe.validate_keeper_opening_release(evidence), evidence)
            for field in evidence:
                with self.subTest(mode=mode, field=field), self.assertRaises(Exception):
                    probe.validate_keeper_opening_release({key: value for key, value in evidence.items() if key != field})
            with self.assertRaises(Exception):
                probe.validate_keeper_opening_release(dict(evidence, authority=True))

    @unittest.skipUnless(sys.platform == "linux", "fsync filter requires Linux")
    def test_fsync_failure_filter_is_scoped_to_the_child(self):
        import pty

        script = """
import errno, os, sys, tempfile
import creation_native_probe as probe
probe.setup_fsync_failure_terminal(int(sys.argv[1]))
with tempfile.TemporaryFile() as stream:
    stream.write(b'public fixture')
    stream.flush()
    try:
        os.fsync(stream.fileno())
    except OSError as error:
        assert error.errno == errno.EIO
    else:
        raise AssertionError('fsync was not refused')
    os.fdatasync(stream.fileno())
    stream.seek(0)
    assert stream.read() == b'public fixture'
"""
        master, slave = pty.openpty()
        try:
            result = subprocess.run([sys.executable, "-B", "-c", script, str(slave)],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, pass_fds=(slave,),
                                    cwd=Path(probe.__file__).parent, timeout=5, check=False)
        finally:
            os.close(slave)
            os.close(master)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, b"", b""))
        with tempfile.TemporaryFile() as stream:
            os.fsync(stream.fileno())

    def test_consumed_opening_requires_selected_real_publication_and_exact_evidence(self):
        args = SimpleNamespace(consume_initial_opening_permit="normal", publish_initial_opening_permit=True,
                               case="publish-complete", reader_check="stable", mount_check=None,
                               reader=Path("/reader"))
        self.assertEqual(probe.validate_consumed_opening_selection(args), "normal")
        for changed in (dict(publish_initial_opening_permit=False), dict(case="publish-record-before-rename-failure"),
                        dict(endpoint_active=True), dict(consume_initial_opening_permit="unknown")):
            with self.subTest(changed=changed), self.assertRaises(Exception):
                probe.validate_consumed_opening_selection(SimpleNamespace(**{**vars(args), **changed}))
        for mode in ("normal", "sync-error"):
            evidence = dict(mode=mode, invalid_permit_refused=mode == "normal", consumed_before_input=True,
                            missing_permit_refused=True, terminal_restored=True, authority=False,
                            signature_verified=mode == "normal", sync_error_refused=mode == "sync-error")
            self.assertEqual(probe.validate_consumed_opening(evidence), evidence)
            for field in evidence:
                with self.subTest(field=field), self.assertRaises(Exception):
                    probe.validate_consumed_opening({key: value for key, value in evidence.items() if key != field})
            with self.assertRaises(Exception):
                probe.validate_consumed_opening(dict(evidence, authority=True))

    def test_initial_opening_permit_evidence_requires_the_checkpoint_and_final_reader(self):
        for case, final in (("publish-complete", "first-open"),
                            ("publish-record-before-rename-failure", "refused")):
            evidence = dict(permit_durable_before_admission=True, package_lock_contended=True,
                            orphan_refused=True, permit_unchanged=True, producer_exit_observed=True,
                            final_reader=final, authority=False)
            self.assertEqual(probe.validate_initial_opening_permit(evidence, case), evidence)
            for field in evidence:
                changed = dict(evidence)
                del changed[field]
                with self.subTest(case=case, field=field), self.assertRaises(Exception):
                    probe.validate_initial_opening_permit(changed, case)
            for changed in (dict(evidence, authority=True), dict(evidence, package_lock_contended=False),
                            dict(evidence, final_reader="clean-close")):
                with self.subTest(changed=changed), self.assertRaises(Exception):
                    probe.validate_initial_opening_permit(changed, case)

    def test_initial_opening_permit_selection_requires_explicit_supported_publication(self):
        args = SimpleNamespace(publish_initial_opening_permit=True, case="publish-complete",
                               reader_check="stable", mount_check=None, reader=Path("/reader"))
        self.assertTrue(probe.validate_initial_opening_selection(args))
        self.assertTrue(probe.validate_initial_opening_selection(
            SimpleNamespace(**{**vars(args), "case": "publish-record-before-rename-failure"})))
        for changed in (dict(case="complete"), dict(reader_check="launch-validate"),
                        dict(endpoint_active=True), dict(opening_permit_check=True), dict(reader=None)):
            with self.subTest(changed=changed), self.assertRaises(Exception):
                probe.validate_initial_opening_selection(SimpleNamespace(**{**vars(args), **changed}))

    def test_initial_opening_checkpoint_preserves_the_complete_closure(self):
        for case in ("publish-complete", "publish-record-before-rename-failure"):
            checkpoint = dict(self.final(case), stage="opening-permit-durable", preparation_dropped=False)
            for field in probe.PUBLICATION_FIELDS:
                checkpoint.pop(field)
            self.assertEqual(probe.parse_packet(draft.canonical(checkpoint), case, 42,
                                                "opening-permit-durable"), checkpoint)
            with self.assertRaises(Exception):
                probe.parse_packet(draft.canonical(dict(checkpoint, closure_acknowledged=False)),
                                   case, 42, "opening-permit-durable")

    def test_opening_permit_evidence_requires_every_case_and_no_authority(self):
        outcomes = dict.fromkeys((
            "missing-collection", "symlink-collection", "missing-permit", "symlink-permit",
            "hardlink-permit", "wrong-mode", "wrong-digest", "wrong-version", "wrong-width",
            "wrong-kind", "reserved-byte"), "refused")
        outcomes.update({"first-open": "first-open", "clean-close": "clean-close"})
        value = dict(cases=outcomes, record_unchanged=True, authority=False)
        self.assertEqual(probe.validate_opening_permit_check(value), value)
        for changed in (dict(value, authority=True), dict(value, record_unchanged=False),
                        dict(value, cases=dict(outcomes, **{"wrong-digest": "first-open"})),
                        dict(value, cases={key: result for key, result in outcomes.items() if key != "wrong-width"})):
            with self.subTest(changed=changed), self.assertRaises(Exception):
                probe.validate_opening_permit_check(changed)

    def test_opening_permit_selection_requires_stable_complete_without_endpoints(self):
        args = SimpleNamespace(opening_permit_check=True, case="publish-complete",
                               reader_check="stable", mount_check=None, reader=Path("/reader"))
        self.assertTrue(probe.validate_opening_permit_selection(args))
        for changed in (dict(case="complete"), dict(reader_check="launch-validate"),
                        dict(endpoint_active=True), dict(reader=None)):
            with self.subTest(changed=changed), self.assertRaises(Exception):
                probe.validate_opening_permit_selection(SimpleNamespace(**{**vars(args), **changed}))

    def test_endpoint_packets_require_verified_roles_and_keep_the_secret_private(self):
        secret = b"b" * 64
        packet = {
            "endpoint_role": "custodian",
            "generation": "a" * 64,
            "signature_verified": True,
            "session_closed": True,
        }

        def frame(value):
            return json.dumps(value).encode("ascii") + b"\n"

        self.assertEqual(probe.endpoint_packet(frame(packet), "custodian", secret), packet)
        verifier = dict(packet, endpoint_role="verifier", session_closed=False)
        self.assertEqual(probe.endpoint_packet(frame(verifier), "verifier", secret), verifier)
        for key, value in (
            ("endpoint_role", "verifier"),
            ("generation", "0" * 64),
            ("generation", "b" * 64),
            ("signature_verified", False),
            ("session_closed", False),
        ):
            with self.subTest(key=key, value=value), self.assertRaises(Exception):
                probe.endpoint_packet(frame(dict(packet, **{key: value})), "custodian", secret)

    def test_endpoint_proof_requires_all_observed_cleanup_and_process_evidence(self):
        proof = {
            "generation": "a" * 64,
            "signature_verified": True,
            "distinct_processes_verified": True,
            "process_exits_observed": True,
            "session_closed": True,
            "private_secret_pipe_verified": True,
            "standard_streams_clean": True,
        }
        self.assertEqual(probe.validate_endpoint_proof(proof), proof)
        for field in proof:
            with self.subTest(field=field), self.assertRaisesRegex(Exception, "evidence missing"):
                probe.validate_endpoint_proof(dict(proof, **{field: False}))

    def test_attended_endpoint_evidence_requires_terminal_checks_and_exact_selection(self):
        proof = dict(generation="a" * 64, signature_verified=True, distinct_processes_verified=True,
                     process_exits_observed=True, session_closed=True, standard_streams_clean=True,
                     attended_terminal_verified=True, terminal_restored=True)
        claim = dict(attended_endpoint_proof_requested=True, attended_endpoint_proof=proof, reader_check="stable",
                     attended_endpoint_wrong_secret_refused=True)
        self.assertEqual(probe.validate_attended_endpoint_proof(proof), proof)
        probe.validate_endpoint_observation(claim, "publish-complete")
        for field in proof:
            wrong = dict(proof)
            del wrong[field]
            for changed in (wrong, dict(proof, **{field: 1})):
                with self.subTest(field=field, changed=changed), self.assertRaisesRegex(Exception, "evidence missing"):
                    probe.validate_attended_endpoint_proof(changed)
        with self.assertRaisesRegex(Exception, "evidence missing"):
            probe.validate_attended_endpoint_proof(dict(proof, private_secret_pipe_verified=True))
        for change in (dict(endpoint_proof_requested=True), dict(attended_endpoint_proof_requested=1),
                       dict(attended_endpoint_wrong_secret_refused=1), dict(attended_endpoint_wrong_secret_refused=False),
                       dict(reader_check="launch-validate"), dict(live_mount=None)):
            with self.subTest(change=change), self.assertRaisesRegex(Exception, "selection differs"):
                probe.validate_endpoint_observation(dict(claim, **change), "publish-complete")
        for case in ("complete", "multi-unrelated", "death-before-durable-reply"):
            with self.subTest(case=case), self.assertRaisesRegex(Exception, "selection differs"):
                probe.validate_observation(dict(claim), case)
        missing = dict(claim)
        del missing[probe.ENDPOINT_WRONG_SECRET_REFUSED]
        with self.assertRaisesRegex(Exception, "selection differs"):
            probe.validate_endpoint_observation(missing, "publish-complete")
        with self.assertRaisesRegex(Exception, "selection differs"):
            probe.validate_endpoint_observation(dict(attended_endpoint_wrong_secret_refused=True), "publish-complete")

    def test_active_endpoint_evidence_requires_both_signed_generations_and_exact_selection(self):
        proof = dict(
            predecessor_signed=True,
            missing_closure_refused=True,
            incomplete_closure_refused=True,
            different_generation_closure_refused=True,
            lost_connection_closure_refused=True,
            reservation_reacquired_after_keeper_exit=True,
            live_keeper_death_observed=True,
            terminal_restored=True,
            distinct_generation_verified=True,
            successor_signature_verified=True,
            sessions_closed=True,
            process_exits_observed=True,
            predecessor_generation="a" * 64,
            successor_generation="b" * 64,
        )
        claim = dict(active_endpoint_proof_requested=True, active_endpoint_proof=proof,
                     reader_check="stable")
        self.assertEqual(probe.validate_active_endpoint_proof(proof), proof)
        probe.validate_endpoint_observation(claim, "publish-complete")
        for field in probe.ACTIVE_ENDPOINT_PROOF_FLAGS:
            with self.subTest(field=field), self.assertRaisesRegex(Exception, "evidence missing"):
                probe.validate_active_endpoint_proof(dict(proof, **{field: False}))
        for changed in (
            dict(proof, unexpected=True),
            {key: value for key, value in proof.items() if key != "sessions_closed"},
            {key: value for key, value in proof.items() if key != "lost_connection_closure_refused"},
            {key: value for key, value in proof.items() if key != "live_keeper_death_observed"},
            dict(proof, predecessor_generation="0" * 64),
            dict(proof, successor_generation="a" * 64),
            dict(proof, predecessor_generation="z" * 64),
        ):
            with self.subTest(changed=changed), self.assertRaisesRegex(Exception, "evidence missing"):
                probe.validate_active_endpoint_proof(changed)
        for changed in (
            dict(claim, active_endpoint_proof_requested=1),
            dict(claim, endpoint_proof_requested=True),
            dict(claim, attended_endpoint_proof_requested=True),
            dict(claim, mount_check="reader-key"),
            dict(claim, reader_check="launch-validate"),
        ):
            with self.subTest(changed=changed), self.assertRaisesRegex(Exception, "selection differs"):
                probe.validate_endpoint_observation(changed, "publish-complete")

    def test_active_endpoint_observation_checks_connection_loss_before_the_successor(self):
        with patch.object(probe, "observe_active_endpoint_case") as observe:
            observe.side_effect = ["c" * 64] * 5 + [("a" * 64, "b" * 64)]
            proof = probe.observe_active_endpoint_proof(None, "credential", b"secret", None, {})
        self.assertEqual([call.args[-1] for call in observe.call_args_list],
                         ["missing", "incomplete", "different-generation", "connection-lost", "keeper-lost", "normal"])
        self.assertIs(proof["lost_connection_closure_refused"], True)
        self.assertIs(proof["live_keeper_death_observed"], True)

    def test_active_keeper_cut_requires_one_exact_packet_and_an_independent_pid(self):
        pid = max(os.getpid(), 50) + 1
        payload = json.dumps({"active_keeper_cut": True, "supervisor_pid": pid}).encode() + b"\n"
        self.assertEqual(probe.parse_active_keeper_cut(payload, 20), pid)
        for wrong_pid in (True, 0, 1, 20, os.getpid(), "22", 2147483648):
            packet = json.dumps({"active_keeper_cut": True, "supervisor_pid": wrong_pid}).encode() + b"\n"
            with self.subTest(pid=wrong_pid), self.assertRaisesRegex(Exception, "identity differs"):
                probe.parse_active_keeper_cut(packet, 20)
        for packet in (payload.rstrip(), payload * 2, b"{}\n",
                       json.dumps({"active_keeper_cut": 1, "supervisor_pid": pid}).encode() + b"\n"):
            with self.subTest(packet=packet), self.assertRaises(Exception):
                probe.parse_active_keeper_cut(packet, 20)

    def test_wrong_endpoint_secret_refusal_requires_one_exact_sanitized_failure(self):
        original = b"a" * 64
        incorrect = bytearray(b"0" + original[1:])
        refusals = (b'{"endpoint_failure":"attended-unlock","kind":"Uncategorized"}\n',
                    b'{"endpoint_failure":"attended-unlock","kind":"TimedOut"}\n')
        for payload in refusals:
            self.assertIs(probe.validate_wrong_endpoint_secret_refusal(1, payload, original, incorrect), True)
        for code in (0, -9, True, None):
            with self.subTest(code=code), self.assertRaisesRegex(Exception, "stream or exit differs"):
                probe.validate_wrong_endpoint_secret_refusal(code, refusals[0], original, incorrect)
        for payload in (refusals[0] * 2, refusals[0].rstrip(), b'"' + original + b'"\n',
                        b'"' + incorrect + b'"\n'):
            with self.subTest(payload=payload), self.assertRaisesRegex(Exception, "stream or exit differs"):
                probe.validate_wrong_endpoint_secret_refusal(1, payload, original, incorrect)
        for payload in (b'{"endpoint_failure":"keeper-ready","kind":"Uncategorized"}\n',
                        b'{"endpoint_failure":"attended-unlock","kind":"Unknown"}\n',
                        b'{"endpoint_failure":"attended-unlock","kind":"Other","extra":false}\n'):
            with self.subTest(payload=payload), self.assertRaisesRegex(Exception, "diagnostic differs"):
                probe.validate_wrong_endpoint_secret_refusal(1, payload, original, incorrect)

    def test_endpoint_selection_requires_one_mode_and_a_stable_complete_publication(self):
        args = dict(case="publish-complete", reader_check="stable", mount_check=None, reader="reader")
        self.assertIsNone(probe.validate_endpoint_selection(SimpleNamespace(**args)))
        self.assertEqual(probe.validate_endpoint_selection(SimpleNamespace(**args, endpoint_proof=True)), "pipe")
        self.assertEqual(probe.validate_endpoint_selection(SimpleNamespace(**args, endpoint_attended=True)), "attended")
        self.assertEqual(probe.validate_endpoint_selection(SimpleNamespace(**args, endpoint_active=True)), "active")
        with self.assertRaisesRegex(Exception, "mutually exclusive"):
            probe.validate_endpoint_selection(SimpleNamespace(**args, endpoint_proof=True, endpoint_attended=True))
        for modes in (("endpoint_proof", "endpoint_attended"),
                      ("endpoint_proof", "endpoint_active"),
                      ("endpoint_attended", "endpoint_active"),
                      ("endpoint_proof", "endpoint_attended", "endpoint_active")):
            with self.subTest(modes=modes), self.assertRaisesRegex(Exception, "mutually exclusive"):
                probe.validate_endpoint_selection(SimpleNamespace(**args, **dict.fromkeys(modes, True)))
        for mode in ("endpoint_proof", "endpoint_attended", "endpoint_active"):
            for field, value in (("case", "complete"), ("reader_check", "launch-validate"),
                                 ("mount_check", "reader-key"), ("reader", None)):
                changed = dict(args, **{mode: True, field: value})
                with self.subTest(mode=mode, field=field), self.assertRaisesRegex(Exception, "stable complete publication"):
                    probe.validate_endpoint_selection(SimpleNamespace(**changed))

    @unittest.skipUnless(sys.platform == "linux", "controlling PTY requires Linux")
    def test_attended_endpoint_pty_is_the_only_input_and_restores_terminal(self):
        import pty
        import termios

        child_code = r"""
import os
import termios

assert sorted(os.listdir("/proc/self/fd")) == ["0", "1", "2", "3"]
with open("/dev/tty", "r+b", buffering=0) as tty:
    original = termios.tcgetattr(tty)
    hidden = termios.tcgetattr(tty)
    hidden[3] &= ~sum(getattr(termios, name, 0) for name in
                      ("ECHO", "ECHONL", "ECHOE", "ECHOK", "ECHOKE", "ECHOCTL", "ECHOPRT"))
    termios.tcsetattr(tty, termios.TCSANOW, hidden)
    tty.write(b"Device credential unlock secret: ")
    value = tty.readline()
    termios.tcsetattr(tty, termios.TCSANOW, original)
    assert len(value) == 65 and value[-1:] == b"\n"
    assert all(byte == value[0] for byte in value[:-1])
print("terminal input observed")
"""
        master, slave = pty.openpty()
        process = None
        try:
            os.set_blocking(master, False)
            original = termios.tcgetattr(slave)
            process = subprocess.Popen([sys.executable, "-B", "-c", child_code], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, close_fds=True,
                                       preexec_fn=lambda: probe.setup_endpoint_terminal(slave), pass_fds=(slave,))
            probe.send_attended_endpoint_input(process, master, slave, b"a" * 64, time.monotonic() + 3, {})
            self.assertEqual(process.communicate(timeout=3), (b"terminal input observed\n", b""))
            self.assertEqual(process.returncode, 0)
            probe.verify_attended_endpoint_terminal(master, slave, original)
        finally:
            if process is not None:
                probe.generated.reap(process)
                for stream in (process.stdin, process.stdout, process.stderr):
                    stream.close()
            os.close(master)
            os.close(slave)

    def test_mount_selection_requires_matching_native_case_and_reader_check(self):
        for check in probe.MOUNT_CHECKS:
            writer = check.startswith("writer-")
            case = "publish-invalidate-before-package" if writer else "publish-complete"
            reader = "stable" if writer else "mount-" + check.removeprefix("reader-")
            args = SimpleNamespace(case=case, reader_check=reader, mount_check=check)
            self.assertEqual(probe.validate_mount_selection(args), check)
            for field, replacement in (("case", "complete"), ("case", "multi-unrelated"),
                                       ("case", "death-before-durable-reply"),
                                       ("reader_check", "key-in-place"), ("mount_check", "unknown")):
                changed = SimpleNamespace(**vars(args)); setattr(changed, field, replacement)
                with self.subTest(check=check, field=field, replacement=replacement), self.assertRaises(Exception):
                    probe.validate_mount_selection(changed)
        self.assertIsNone(probe.validate_mount_selection(SimpleNamespace(case="complete", reader_check="stable")))
        with self.assertRaises(Exception):
            probe.validate_mount_selection(SimpleNamespace(case="publish-complete", reader_check="mount-key"))
        for case in ("complete", "multi-unrelated", "death-before-durable-reply"):
            with self.subTest(report_case=case), self.assertRaisesRegex(Exception, "mount observation uses an unrelated native case"):
                probe.validate_observation({"mount_check": "writer-stage"}, case)

    def test_package_mutation_callback_cannot_be_silently_ignored_by_an_unrelated_case(self):
        for case, callback in (("complete", lambda *_: None), ("death-before-durable-reply", lambda *_: None),
                               ("publish-invalidate-before-package", object())):
            with self.subTest(case=case), self.assertRaisesRegex(Exception, "native package mutation callback differs"):
                probe.observe(SimpleNamespace(case=case), {}, [], None, None, None, package_mutator=callback)

    def test_mount_sources_are_only_required_when_a_mount_check_is_selected(self):
        args = SimpleNamespace(writer="writer", reader="reader")
        normal = probe.source_paths(args)
        args.mount_check = "writer-stage"
        mounted = probe.source_paths(args)
        additions = {"mounts_source", "mount_common_source", "mount_helper_source", "live_mount_controller_source"}
        self.assertEqual(set(mounted) - set(normal), additions)
        self.assertEqual({key: mounted[key] for key in normal}, normal)

    def packet(self):
        record = dict(version=2, installation_id="0" * 25 + "1", creation_id="0" * 25 + "2",
                      device_id="0" * 25 + "3", credential_id="0" * 25 + "4",
                      public_key="a" * 64, ciphertext_sha256=hashlib.sha256(b"key").hexdigest(),
                      manifest_sha256=hashlib.sha256(b"manifest").hexdigest(), ciphertext_len=3,
                      manifest_len=8, creation_profile_revision=1, secret_policy_revision=1)
        return dict(version=1, stage="loader-ready", case="complete", helper_pid=42,
                    agent_pid=43, loader_pid=44, creator_tid=42,
                    record_canonical=draft.canonical(record).decode("ascii"),
                    native_binding_verified=False, **dict.fromkeys(probe.FLAGS, False))

    def test_accepts_exact_startup(self):
        value = self.packet()
        self.assertEqual(probe.parse_packet(draft.canonical(value), "complete", 42, "loader-ready"), value)

    def test_rejects_false_authority_and_changed_process_identity(self):
        for field, replacement in (("creation_admission", True), ("helper_pid", 41),
                                   ("agent_pid", True), ("creator_tid", 43),
                                   ("native_binding_verified", True), ("extra", False)):
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.parse_packet(draft.canonical(dict(self.packet(), **{field: replacement})),
                                   "complete", 42, "loader-ready")

    def test_rejects_changed_stage_record_and_missing_fields(self):
        original = self.packet()
        for field in original:
            changed = copy.deepcopy(original)
            del changed[field]
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.parse_packet(draft.canonical(changed), "complete", 42, "loader-ready")
        with self.assertRaises(Exception):
            probe.parse_packet(draft.canonical(dict(original, stage="bound")), "complete", 42, "loader-ready")

    def final(self, case):
        positive = case == "complete" or case in probe.PUBLICATION_CASES
        value = dict(self.packet(), stage="final", case=case, native_binding_verified=positive)
        record = draft.parse_record(value["record_canonical"])
        challenge = probe.binding.DOMAIN + hashlib.sha256(draft.canonical(record)).digest() + b"n" * 32
        value.update(challenge=challenge.hex() if positive else None,
                     signature="a" * 128 if positive else None,
                     closure_acknowledged=True, resources_closed_after_drain=True, preparation_dropped=True,
                     agent_exit_code=None, agent_exit_signal=probe.LINUX_SIGKILL,
                     loader_exit_code=0, loader_exit_signal=None,
                     **dict.fromkeys(probe.COUNTS, 1))
        value["proofs_published"] = int(positive)
        if case in probe.PUBLICATION_CASES:
            outcome, package, record, ack = probe.PUBLICATION_OUTCOMES[case]
            value.update(publication_outcome=outcome, package_published=package,
                         record_published=record, experimental_durable_publication_ack=ack)
        return value

    def test_final_requires_exact_counts_and_exit_causes(self):
        for case in ("complete", "invalidate-before-proof"):
            value = self.final(case)
            self.assertEqual(probe.parse_packet(draft.canonical(value), case, 42, "final"), value)
            for field, replacement in (("closure_acknowledged", False), ("proofs_verified", 0),
                                       ("native_requests_sent", True), ("agent_exit_signal", 15),
                                       ("loader_exit_code", True), ("producer_origin_checked", True)):
                with self.subTest(case=case, field=field), self.assertRaises(Exception):
                    probe.parse_packet(draft.canonical(dict(value, **{field: replacement})), case, 42, "final")

    def test_linux_exit_protocol_does_not_require_host_sigkill(self):
        value = self.final("complete")
        self.assertEqual(value["agent_exit_signal"], 9)
        with patch.dict(signal.__dict__):
            signal.__dict__.pop("SIGKILL", None)
            self.assertEqual(probe.parse_packet(draft.canonical(value), "complete", 42, "final"), value)

    def test_negative_cannot_publish_and_abandoned_cannot_claim_closed(self):
        negative = self.final("invalidate-before-proof")
        for field, replacement in (("signature", "a" * 128), ("challenge", "a" * 196), ("proofs_published", 1)):
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.parse_packet(draft.canonical(dict(negative, **{field: replacement})),
                                   "invalidate-before-proof", 42, "final")
        abandoned = dict(self.packet(), case="abandon-after-load", stage="abandoned",
                         producer_context_dropped=True, closure_acknowledged=False, admission_stopped=True)
        self.assertEqual(probe.parse_packet(draft.canonical(abandoned), "abandon-after-load", 42, "abandoned"), abandoned)
        with self.assertRaises(Exception):
            probe.parse_packet(draft.canonical(dict(abandoned, closure_acknowledged=True)),
                               "abandon-after-load", 42, "abandoned")
        with self.assertRaises(Exception):
            probe.parse_packet(draft.canonical(dict(negative, case="abandon-after-load")),
                               "abandon-after-load", 42, "final")

    def test_failure_frames_disclose_only_basenames_and_line_numbers(self):
        try:
            raise PermissionError("synthetic private diagnostic")
        except PermissionError as error:
            frames = probe.failure_frames(error)
        self.assertTrue(frames)
        for frame in frames:
            self.assertEqual(set(frame), {"file", "line"})
            self.assertEqual(frame["file"], "test_creation_native_probe.py")
            self.assertIs(type(frame["line"]), int)
        self.assertNotIn("synthetic private diagnostic", repr(frames))

    def test_environment_permission_denied_never_verifies_absence(self):
        def read(path):
            if path.name == "environ":
                raise PermissionError("synthetic denial")
            return b"ssh-agent\0-D\0"
        value = probe.process_metadata(43, b"synthetic-secret", read=read)
        self.assertIs(value["cmdline_secret_absence_verified"], True)
        self.assertEqual(value["environment_observation"], "permission-denied")
        self.assertIs(value["environment_secret_absence_verified"], False)

    def test_metadata_requires_readable_secret_free_cmdline_and_only_tolerates_environment_denial(self):
        for error in (PermissionError, FileNotFoundError, OSError):
            def read(path):
                if path.name == "cmdline":
                    raise error("synthetic failure")
                return b""
            with self.subTest(error=error), self.assertRaises(Exception):
                probe.process_metadata(43, b"synthetic-secret", read=read)
        for contents in (b"synthetic-secret", b"x" * 65537):
            with self.subTest(size=len(contents)), self.assertRaises(Exception):
                probe.process_metadata(43, b"synthetic-secret", read=lambda _: contents)
        def missing_environment(path):
            if path.name == "environ":
                raise FileNotFoundError("synthetic failure")
            return b"ssh-agent\0"
        with self.assertRaises(Exception):
            probe.process_metadata(43, b"synthetic-secret", read=missing_environment)

    def test_readable_environment_is_verified_only_after_secret_scan(self):
        value = probe.process_metadata(43, b"synthetic-secret", read=lambda _: b"LANG=C\0")
        self.assertEqual(value, dict(cmdline_secret_absence_verified=True, environment_observation="verified",
                                     environment_secret_absence_verified=True))
        def secret_environment(path):
            return b"synthetic-secret" if path.name == "environ" else b"ssh-agent\0"
        with self.assertRaises(Exception):
            probe.process_metadata(43, b"synthetic-secret", read=secret_environment)

    def test_metadata_schema_cannot_convert_denial_into_verified_absence(self):
        verified = dict(cmdline_secret_absence_verified=True, environment_observation="verified",
                        environment_secret_absence_verified=True)
        denied = dict(cmdline_secret_absence_verified=True, environment_observation="permission-denied",
                      environment_secret_absence_verified=False)
        self.assertIs(probe.validate_process_metadata(dict(agent=verified, loader=verified)), True)
        self.assertIs(probe.validate_process_metadata(dict(agent=denied, loader=verified)), False)
        for item in (dict(denied, environment_secret_absence_verified=True),
                     dict(denied, environment_secret_absence_verified=0),
                     dict(verified, cmdline_secret_absence_verified=1),
                     dict(verified, environment_observation="unknown"),
                     dict(verified, extra=False)):
            with self.subTest(item=item), self.assertRaises(Exception):
                probe.validate_process_metadata(dict(agent=item, loader=verified))
        with self.assertRaises(Exception):
            probe.validate_process_metadata(dict(agent=verified))

    def test_identity_schema_preserves_denied_executable_observation(self):
        credentials = dict(Uid=[65534] * 4, Gid=[65534] * 4, NoNewPrivs=[1], PPid=[42])
        denied = dict(credentials=credentials, executable_observation="exe_link_denied_cmdline_matches")
        verified = dict(credentials=credentials, executable_observation="verified")
        probe.validate_process_identity(dict(agent=denied, loader=verified), 42)
        for item in (dict(denied, executable_observation=True), dict(denied, extra=True),
                     dict(denied, credentials=dict(credentials, PPid=[43])),
                     dict(denied, credentials=dict(credentials, NoNewPrivs=[True]))):
            with self.subTest(item=item), self.assertRaises(Exception):
                probe.validate_process_identity(dict(agent=item, loader=verified), 42)

    def test_abandonment_requires_retention_while_producer_alive_without_claiming_agent_survival(self):
        value = dict(abandoned_stage_retained_without_closure=True,
                     native_agent_alive_before_abandonment=True,
                     abandoned_stage_retained_while_producer_alive=True)
        probe.validate_abandonment(value, True)
        probe.validate_abandonment(dict.fromkeys(value, False), False)
        for field in value:
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.validate_abandonment(dict(value, **{field: False}), True)
        for retired in (False, True):
            with self.subTest(retired=retired), self.assertRaises(Exception):
                probe.validate_abandonment(dict(value, abandoned_stage_retained_while_agent_alive=retired), True)

    def test_publication_cases_preserve_native_proof_and_truthful_durability(self):
        self.assertEqual(len(probe.PUBLICATION_CASES), 8)
        for case in probe.PUBLICATION_CASES:
            final = self.final(case)
            with self.subTest(case=case):
                self.assertEqual(probe.parse_packet(draft.canonical(final), case, 42, "final"), final)
                for field, replacement in (("native_binding_verified", False), ("signature", None),
                                           ("proofs_published", 0), ("package_published", not final["package_published"]),
                                           ("record_published", not final["record_published"]),
                                           ("experimental_durable_publication_ack", not final["experimental_durable_publication_ack"]),
                                           ("publication_outcome", "unknown")):
                    with self.subTest(field=field), self.assertRaises(Exception):
                        probe.parse_packet(draft.canonical(dict(final, **{field: replacement})), case, 42, "final")

    def test_retained_checkpoint_rejects_other_cases_early_destruction_and_forged_ack(self):
        for case in probe.PUBLICATION_CASES:
            checkpoint = {field: value for field, value in self.final(case).items()
                          if field not in probe.PUBLICATION_FIELDS}
            checkpoint.update(stage="publication-retained", preparation_dropped=False)
            if case not in probe.RETAINED_CASES:
                with self.subTest(case=case), self.assertRaises(Exception):
                    probe.parse_packet(draft.canonical(checkpoint), case, 42, "publication-retained")
                continue
            with self.subTest(case=case):
                self.assertEqual(probe.parse_packet(draft.canonical(checkpoint), case, 42, "publication-retained"), checkpoint)
            for field, replacement in (("preparation_dropped", True), ("closure_acknowledged", False),
                                       ("experimental_durable_publication_ack", True), ("signature", "invalid"),
                                       ("creation_admission", True)):
                with self.subTest(case=case, field=field), self.assertRaises(Exception):
                    probe.parse_packet(draft.canonical(dict(checkpoint, **{field: replacement})),
                                       case, 42, "publication-retained")
            changed = self.final(case)
            changed.update(publication_outcome="durable", experimental_durable_publication_ack=True)
            with self.subTest(case=case), self.assertRaises(Exception):
                probe.parse_packet(draft.canonical(changed), case, 42, "final")

    @unittest.skipUnless(sys.platform == "linux", "requires Linux descriptor-relative observation")
    def test_retained_mutation_evidence_detects_later_byte_changes_and_equal_byte_replacement(self):
        for replacement in (False, True):
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "credential").mkdir()
                member = root / "credential" / "record.json"
                member.write_bytes(b"!mutated record")
                parent = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
                evidence = probe.RetainedPublicationMutation(parent, "credential", "record.json")
                try:
                    evidence.validate()
                    if replacement:
                        member.unlink()
                        member.write_bytes(b"!mutated record")
                    else:
                        with member.open("r+b") as output:
                            output.write(b"?")
                    with self.subTest(replacement=replacement), self.assertRaises(Exception):
                        evidence.validate()
                finally:
                    evidence.close()
                    os.close(parent)

    def test_publication_checkpoints_require_closed_native_resources_and_retained_preparation(self):
        for case in probe.PUBLICATION_CASES:
            final = self.final(case)
            closed = {field: value for field, value in final.items() if field not in probe.PUBLICATION_FIELDS}
            closed.update(stage="closed-before-publication", preparation_dropped=False)
            with self.subTest(case=case):
                self.assertEqual(probe.parse_packet(draft.canonical(closed), case, 42, "closed-before-publication"), closed)
                for field, replacement in (("closure_acknowledged", False), ("preparation_dropped", True),
                                           ("agent_exit_signal", None), ("resources_closed_after_drain", False)):
                    with self.subTest(field=field), self.assertRaises(Exception):
                        probe.parse_packet(draft.canonical(dict(closed, **{field: replacement})),
                                           case, 42, "closed-before-publication")
                package = dict(closed, stage="package-durable")
                if case in ("publish-package-failure", "publish-invalidate-before-package"):
                    with self.assertRaises(Exception):
                        probe.parse_packet(draft.canonical(package), case, 42, "package-durable")
                else:
                    self.assertEqual(probe.parse_packet(draft.canonical(package), case, 42, "package-durable"), package)
        old = dict(self.final("complete"), stage="closed-before-publication", preparation_dropped=False)
        with self.assertRaises(Exception):
            probe.parse_packet(draft.canonical(old), "complete", 42, "closed-before-publication")


if __name__ == "__main__":
    unittest.main()
