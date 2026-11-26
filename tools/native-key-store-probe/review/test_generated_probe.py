"""Metadata and disclosure controls without native programs."""

import unittest
import stat
from types import SimpleNamespace
from unittest.mock import Mock, patch

import generated_probe as probe


class DisclosureTests(unittest.TestCase):
    def test_partial_disclosure_does_not_release_input(self):
        sample = probe.SECRET_MARKER + b"0" * 63
        self.assertIsNone(probe.parse_disclosure(sample))

    def test_complete_disclosure_requires_exact_lowercase_encoding(self):
        for value in (b"a" * 63, b"A" * 64, b"g" * 64, b"a" * 65):
            with self.subTest(length=len(value)), self.assertRaises(probe.ProbeError):
                probe.parse_disclosure(probe.SECRET_MARKER + value + probe.REENTRY_MARKER)

    def test_duplicate_disclosure_cannot_be_credited(self):
        sample = probe.SECRET_MARKER + b"a" * 64 + probe.REENTRY_MARKER
        with self.assertRaises(probe.ProbeError):
            probe.parse_disclosure(sample + sample)

    def test_disclosure_value_is_transient_not_metadata(self):
        sample = probe.SECRET_MARKER + b"a" * 64 + probe.REENTRY_MARKER
        secret = probe.parse_disclosure(sample)
        self.assertEqual(secret, bytearray(b"a" * 64))
        probe.wipe(secret)
        self.assertEqual(secret, bytearray(64))

    def test_pty_carriage_return_does_not_change_the_secret(self):
        value = probe.parse_disclosure(probe.SECRET_MARKER + b"0" * 64 + b"\r" + probe.REENTRY_MARKER)
        self.assertEqual(value, bytearray(b"0" * 64))
        probe.wipe(value)


class ObservationTests(unittest.TestCase):
    def test_success_requires_no_standard_stream_disclosure_and_checked_package(self):
        value = probe.case_observation("valid-generated-package", 0, True, True, 1, True, True, True)
        self.assertTrue(probe.validate_case(value))
        for field in ("terminal_restored", "echo_disabled", "foreground", "private_arguments", "standard_streams_clean"):
            changed = dict(value, **{field: False})
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_case(changed)

    def test_negative_case_cannot_publish_or_disclose_before_ack(self):
        value = probe.case_observation("wrong-ack", 1, False, True, 0, True, True, None)
        self.assertTrue(probe.validate_case(value))
        for changes in ({"package_created": True}, {"disclosures": 1}, {"exit_code": 0}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.validate_case(dict(value, **changes))

    def test_hangup_does_not_claim_terminal_restoration(self):
        value = probe.case_observation("terminal-hangup", 1, False, None, 1, True, True, True)
        self.assertTrue(probe.validate_case(value))
        self.assertIsNone(value["terminal_restored"])

    def test_errors_never_copy_exception_messages(self):
        error = RuntimeError("private-content-never-exported")
        self.assertNotIn("private-content", str(probe.safe_error(error, "case")))

    def test_headless_case_cannot_credit_unobserved_terminal_state(self):
        value = probe.case_observation("no-tty", 1, False, None, 0, None, None, None)
        self.assertTrue(probe.validate_case(value))
        for field in ("echo_disabled", "foreground", "private_arguments", "process_metadata_checked"):
            with self.subTest(field=field), self.assertRaises(probe.ProbeError):
                probe.validate_case(dict(value, **{field: True}))


class DeadlineTests(unittest.TestCase):
    def test_selected_deadline_matrix_keeps_positive_control_first(self):
        self.assertEqual(probe.selected_cases(None), probe.CASES)
        self.assertEqual(probe.selected_cases("acknowledgement"),
                         ("valid-generated-package", "deadline-acknowledgement"))
        self.assertEqual(probe.selected_cases("confirmation"),
                         ("valid-generated-package", "deadline-confirmation"))
        with self.assertRaises(probe.ProbeError):
            probe.selected_cases("unknown")

    def test_real_deadline_observation_requires_elapsed_time_and_refusal(self):
        for name, elapsed, disclosures, private in (
            ("deadline-acknowledgement", 60000.0, 0, None),
            ("deadline-confirmation", 180000.0, 1, True),
        ):
            value = dict(probe.case_observation(name, 1, False, True, disclosures, True, True, private),
                         delay_elapsed_ms=elapsed)
            self.assertTrue(probe.validate_case(value))
            for invalid in (None, True, float("nan"), float("inf"), elapsed - 1001, elapsed + 21000):
                with self.subTest(name=name, invalid=invalid), self.assertRaises(probe.ProbeError):
                    probe.validate_case(dict(value, delay_elapsed_ms=invalid))


class NativeControlTests(unittest.TestCase):
    def test_explicit_secret_response_is_not_added_to_arguments(self):
        provider = Mock()
        secret = bytearray(b"a" * 64)
        provider.execute.return_value = (0, b"ssh-ed25519 cHVibGljLWJsb2I=\n")
        bundle = {"process_exit_uncertain": False}
        with patch.object(probe.native, "parse_key", return_value={"public_blob": b"public-blob"}):
            self.assertTrue(probe.native_public_control(provider, bundle, probe.Path("/fixture"), secret, {}))
        call = provider.execute.call_args
        self.assertEqual(call.kwargs["responses"], [secret])
        self.assertNotIn(secret, call.args[1])
        self.assertFalse(bundle["process_exit_uncertain"])

    def test_native_echo_is_refused_without_copying_output_to_diagnostics(self):
        provider = Mock()
        secret = bytearray(b"a" * 64)
        provider.execute.return_value = (0, b"ssh-ed25519 cHVibGljLWJsb2I=\n" + secret)
        with patch.object(probe.native, "parse_key", return_value={"public_blob": b"public-blob"}):
            with self.assertRaises(probe.ProbeError) as caught:
                probe.native_public_control(provider, {"process_exit_uncertain": False}, probe.Path("/fixture"), secret, {})
        self.assertNotIn("a" * 64, str(caught.exception))

    def test_uncertain_native_exit_prevents_provider_and_tree_cleanup(self):
        provider = Mock()
        bundle = {"provider": provider, "roots": [], "process_exit_uncertain": True}
        with patch.object(probe, "CURRENT_REPORT", {}), patch.object(probe, "UNCERTAIN_BUNDLES", []) as retained:
            with self.assertRaises(probe.ProbeError):
                probe.finish(bundle, None)
            self.assertEqual(retained, [bundle])
        provider.close.assert_not_called()


class FailureMatrixTests(unittest.TestCase):
    def test_failure_matrix_is_separate_and_keeps_positive_control_first(self):
        expected = ("valid-generated-package", "terminal-drift-acknowledgement",
                    "terminal-drift-confirmation", "interrupted-key-synced",
                    "interrupted-manifest-synced", "interrupted-before-publish",
                    "interrupted-after-publish")
        self.assertEqual(probe.selected_cases(None, True), expected)
        with self.assertRaises(probe.ProbeError):
            probe.selected_cases("confirmation", True)

    def test_terminal_drift_requires_an_observed_change_and_no_publication(self):
        for name, disclosed, private in (("terminal-drift-acknowledgement", 0, None),
                                         ("terminal-drift-confirmation", 1, True)):
            value = dict(probe.case_observation(name, 1, False, True, disclosed, True, True, private),
                         attrs_changed=True)
            self.assertTrue(probe.validate_case(value))
            for field, invalid in (("attrs_changed", False), ("package_created", True),
                                   ("exit_code", 0), ("exit_code", 77), ("exit_code", 2)):
                with self.subTest(name=name, field=field), self.assertRaises(probe.ProbeError):
                    probe.validate_case(dict(value, **{field: invalid}))

    def test_interruption_preserves_the_committed_package_only_after_rename(self):
        for point in ("key-synced", "manifest-synced", "before-publish", "after-publish"):
            committed = point == "after-publish"
            value = dict(probe.case_observation("interrupted-" + point, 77, committed, True, 1, True, True, True),
                         fault_point=point, preparations_remaining=0 if committed else 1,
                         package_check_accepted=True if committed else None, filesystem_secret_absent=True)
            if committed:
                value["native_public_extraction_matches"] = True
            self.assertTrue(probe.validate_case(value))
            for changes in ({"exit_code": 1}, {"package_created": not committed},
                            {"preparations_remaining": 1 if committed else 0},
                            {"filesystem_secret_absent": False}):
                with self.subTest(point=point, changes=changes), self.assertRaises(probe.ProbeError):
                    probe.validate_case(dict(value, **changes))

    def test_fixture_scan_refuses_plaintext_and_unbounded_members(self):
        parent = Mock()
        file = Mock()
        file.name = "key.openssh"
        metadata = SimpleNamespace(st_mode=stat.S_IFREG | 0o600, st_uid=0, st_gid=0, st_nlink=1,
                                   st_dev=1, st_ino=2, st_size=5, st_mtime_ns=3, st_ctime_ns=4)
        file.lstat.return_value = metadata
        parent.iterdir.return_value = [file]
        package = Mock()
        package.validate_safe_tree_root.return_value = (1, 1)
        with patch.object(probe.session, "package_backend", return_value=package), \
                patch.object(probe.os, "geteuid", return_value=0, create=True), \
                patch.object(probe.native, "read_private_file", return_value=b"plain"):
            self.assertEqual(probe.inspect_interrupted_files(parent, (1, 1), b"absent"), 0)
            with self.assertRaises(probe.ProbeError):
                probe.inspect_interrupted_files(parent, (1, 1), b"plain")
            parent.iterdir.return_value = [file] * 9
            with self.assertRaises(probe.ProbeError):
                probe.inspect_interrupted_files(parent, (1, 1), b"absent")

    def test_terminal_drift_changes_only_input_and_preserves_disabled_echo(self):
        terminal = Mock(ICRNL=256, TCSANOW=0, ECHO=8, ECHONL=16)
        before = [256, 1, 2, 0, 3, 4, [b"x"]]
        changed = [0, 1, 2, 0, 3, 4, [b"x"]]
        terminal.tcgetattr.side_effect = [before, changed]
        self.assertTrue(probe.drift_terminal_input(42, terminal))
        terminal.tcsetattr.assert_called_once_with(42, 0, changed)
        self.assertEqual(before[0], 256)
        terminal.tcgetattr.side_effect = [before, [0, 1, 2, 8, 3, 4, [b"x"]]]
        with self.assertRaises(probe.ProbeError):
            probe.drift_terminal_input(42, terminal)

    def test_fixture_scan_refuses_oversize_or_changed_parent_before_reading(self):
        parent, member, package = Mock(), Mock(), Mock()
        member.lstat.return_value = SimpleNamespace(st_mode=stat.S_IFREG | 0o600, st_uid=0,
                                                   st_nlink=1, st_size=1024 * 1024 + 1)
        parent.iterdir.return_value = [member]
        package.validate_safe_tree_root.return_value = (1, 1)
        with patch.object(probe.session, "package_backend", return_value=package), \
                patch.object(probe.os, "geteuid", return_value=0, create=True), \
                patch.object(probe.native, "read_private_file") as read:
            with self.assertRaises(probe.ProbeError):
                probe.inspect_interrupted_files(parent, (1, 1), b"absent")
            read.assert_not_called()
            package.validate_safe_tree_root.return_value = (1, 99)
            with self.assertRaises(probe.ProbeError):
                probe.inspect_interrupted_files(parent, (1, 1), b"absent")
            parent.iterdir.assert_called_once()


class PublicationReturnTests(unittest.TestCase):
    def test_child_environment_selects_one_fault_channel_without_mutating_the_parent(self):
        env = {"VIVAC_OPENSSH_DISPOSABLE": "1", "VIVAC_OPENSSH_PACKAGE_EXIT_AT": "after-publish",
               "VIVAC_OPENSSH_PACKAGE_ERROR_AT": "before-publish"}
        original = dict(env)
        error = probe.child_environment(env, "returned-key-synced")
        self.assertEqual(error["VIVAC_OPENSSH_PACKAGE_ERROR_AT"], "key-synced")
        self.assertNotIn("VIVAC_OPENSSH_PACKAGE_EXIT_AT", error)
        interrupted = probe.child_environment(env, "interrupted-after-publish")
        self.assertEqual(interrupted["VIVAC_OPENSSH_PACKAGE_EXIT_AT"], "after-publish")
        self.assertNotIn("VIVAC_OPENSSH_PACKAGE_ERROR_AT", interrupted)
        normal = probe.child_environment(env, "valid-generated-package")
        self.assertNotIn("VIVAC_OPENSSH_PACKAGE_EXIT_AT", normal)
        self.assertNotIn("VIVAC_OPENSSH_PACKAGE_ERROR_AT", normal)
        self.assertNotIn("VIVAC_OPENSSH_DISPOSABLE", probe.child_environment(env, "unset-disposable"))
        self.assertEqual(env, original)

    def test_return_matrix_is_separate_and_preserves_the_positive_control(self):
        expected = ("valid-generated-package", "returned-key-synced", "returned-manifest-synced",
                    "returned-before-publish", "returned-after-publish")
        self.assertEqual(probe.selected_cases(None, publication_errors=True), expected)
        for deadline, failure in (("confirmation", False), (None, True)):
            with self.subTest(deadline=deadline, failure=failure), self.assertRaises(probe.ProbeError):
                probe.selected_cases(deadline, failure, True)

    def test_returned_error_preserves_only_the_already_committed_package(self):
        for point in ("key-synced", "manifest-synced", "before-publish", "after-publish"):
            committed = point == "after-publish"
            value = dict(probe.case_observation("returned-" + point, 2 if committed else 1,
                                               committed, True, 1, True, True, True),
                         error_point=point, preparations_remaining=0,
                         package_check_accepted=True if committed else None,
                         filesystem_secret_absent=True, indeterminate_durability_reported=committed)
            if committed:
                value["native_public_extraction_matches"] = True
            self.assertTrue(probe.validate_case(value))
            for changes in ({"exit_code": 77}, {"exit_code": 1 if committed else 2},
                            {"package_created": not committed}, {"preparations_remaining": 1},
                            {"indeterminate_durability_reported": not committed},
                            {"filesystem_secret_absent": False}):
                with self.subTest(point=point, changes=changes), self.assertRaises(probe.ProbeError):
                    probe.validate_case(dict(value, **changes))

    def test_publication_diagnostic_requires_exact_message_and_empty_stdout(self):
        refused = b"Experimental OpenSSH writer refused or failed.\n"
        indeterminate = (b"Experimental OpenSSH package committed; durability is indeterminate. "
                         b"Do not overwrite or retry automatically.\n")
        self.assertFalse(probe.publication_error_reported("returned-before-publish", refused, b""))
        self.assertTrue(probe.publication_error_reported("returned-after-publish", indeterminate, b""))
        for output, stdout in ((refused, b""), (indeterminate.rstrip(), b""),
                               (indeterminate, indeterminate), (indeterminate + b"extra", b"")):
            with self.subTest(stdout=bool(stdout)), self.assertRaises(probe.ProbeError):
                probe.publication_error_reported("returned-after-publish", output, stdout)

    def test_returned_error_scan_accepts_an_empty_owned_parent(self):
        parent, package = Mock(), Mock()
        parent.iterdir.return_value = []
        package.validate_safe_tree_root.return_value = (1, 1)
        with patch.object(probe.session, "package_backend", return_value=package):
            self.assertEqual(probe.inspect_interrupted_files(parent, (1, 1), b"public-fixture"), 0)
        self.assertEqual(package.validate_safe_tree_root.call_count, 2)


if __name__ == "__main__":
    unittest.main()
