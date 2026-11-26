"""KDF measurement controls without native programs or containers."""

import base64
import hashlib
import json
import os
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import kdf_probe as probe

with patch.object(os, "geteuid", return_value=0, create=True):
    import package_probe as package_fixture


class DispatchTests(unittest.TestCase):
    def test_structural_refusal_cannot_reach_native_dispatch(self):
        callback = Mock()
        with patch.object(probe.session, "package_backend") as backend:
            backend.return_value.check.return_value = (1, b"refused")
            self.assertEqual(probe.checked_dispatch("writer", "package", {}, callback), 1)
        callback.assert_not_called()

    def test_control_acceptance_does_reach_the_callback(self):
        callback = Mock()
        with patch.object(probe.session, "package_backend") as backend:
            backend.return_value.check.return_value = (0, b"")
            self.assertEqual(probe.checked_dispatch("writer", "package", {}, callback), 0)
        callback.assert_called_once_with()

    def test_signal_exit_does_not_become_an_ordinary_refusal_or_dispatch(self):
        callback = Mock()
        with patch.object(probe.session, "package_backend") as backend:
            backend.return_value.check.return_value = (-9, b"")
            self.assertEqual(probe.checked_dispatch("writer", "package", {}, callback), -9)
        callback.assert_not_called()

    def test_callback_failure_is_not_recorded_as_success(self):
        with patch.object(probe.session, "package_backend") as backend:
            backend.return_value.check.return_value = (0, b"")
            with self.assertRaisesRegex(RuntimeError, "failure"):
                probe.checked_dispatch("writer", "package", {}, Mock(side_effect=RuntimeError("failure")))


class MeasurementTests(unittest.TestCase):
    def test_an_isolated_high_sample_stays_in_place(self):
        samples = [1.0, 2.0, 3.0, 89.0, 4.0, 5.0, 6.0]
        result = probe.sample_case("unlocked-sign-proof", samples)
        self.assertEqual(result["samples_ms"], samples)
        self.assertEqual(result["classification"], "above-band")
        self.assertIsNot(result["samples_ms"], samples)

    def test_tolerance_boundary_is_exact_and_does_not_round_down_a_miss(self):
        result = probe.sample_case("unlocked-sign-proof", [55.0] * 7)
        self.assertEqual(result["classification"], "within-tolerance")
        result = probe.sample_case("unlocked-sign-proof", [55.00001] * 7)
        self.assertEqual(result["classification"], "above-band")

    def test_nominal_boundary_is_not_reported_as_tolerance(self):
        result = probe.sample_case("package-create", [2000.0] * 7)
        self.assertEqual(result["classification"], "nominal")

    def test_nonfinite_negative_and_boolean_samples_are_refused(self):
        for bad in (float("nan"), float("inf"), -0.01, True, "1"):
            with self.subTest(value=bad):
                with self.assertRaises(probe.ProbeError):
                    probe.sample_case("package-create", [1.0] * 6 + [bad])

    def test_missing_samples_are_not_filled_or_dropped(self):
        for count in (0, 6, 8):
            with self.subTest(count=count):
                with self.assertRaises(probe.ProbeError):
                    probe.sample_case("package-create", [1.0] * count)

    def test_timer_preserves_the_operation_result_and_elapsed_unit(self):
        with patch.object(probe.time, "perf_counter_ns", side_effect=[123_000_000, 125_500_000]):
            elapsed, result = probe.timed(lambda: "accepted")
        self.assertEqual((elapsed, result), (2.5, "accepted"))

    def test_failed_operation_has_no_successful_sample(self):
        with patch.object(probe.time, "perf_counter_ns", return_value=123):
            with self.assertRaises(RuntimeError):
                probe.timed(Mock(side_effect=RuntimeError("operation failed")))

    def test_partial_series_retains_success_and_failed_attempt_with_original_error(self):
        report = {}
        original = RuntimeError("operation failed")
        with patch.object(probe, "CURRENT_REPORT", report), \
                patch.object(probe, "CURRENT_PHASE", "activation-proof"), \
                patch.object(probe.time, "perf_counter_ns", side_effect=[100, 200, 300, 450]):
            probe.timed(lambda: "accepted")
            with self.assertRaises(RuntimeError) as caught:
                probe.timed(Mock(side_effect=original))
        self.assertIs(caught.exception, original)
        self.assertEqual(report["raw_attempts"], [
            {"case": "activation-proof", "elapsed_ms": 0.0001, "returned": True},
            {"case": "activation-proof", "elapsed_ms": 0.00015, "returned": False},
        ])

    def test_measurement_failure_cannot_replace_the_operation_error(self):
        original = RuntimeError("operation failed")
        report = {}
        with patch.object(probe, "CURRENT_REPORT", report), \
                patch.object(probe.time, "perf_counter_ns", side_effect=[100, float("nan")]):
            with self.assertRaises(RuntimeError) as caught:
                probe.timed(Mock(side_effect=original))
        self.assertIs(caught.exception, original)
        self.assertIn("measurement_failure", report)
        self.assertNotIn("raw_attempts", report)


def decoded_fixture():
    wire = probe.native.wire_string
    return (b"openssh-key-v1\0" + wire(b"aes256-gcm@openssh.com") + wire(b"bcrypt")
            + wire(wire(b"s" * 16) + struct.pack(">I", 64))
            + struct.pack(">I", 1) + wire(b"public-fixture") + wire(b"encrypted-fixture") + b"tag")


def decode_pem(value):
    return base64.b64decode(b"".join(value.splitlines()[1:-1]), validate=True)


class MutationTests(unittest.TestCase):
    def test_maximum_round_counter_is_encoded_without_derivation(self):
        changed = decode_pem(probe.mutated_key(decoded_fixture(), "reject-rounds-4294967295"))
        reader = probe.native.Reader(changed)
        self.assertEqual(reader.take(15), b"openssh-key-v1\0")
        self.assertEqual(reader.string(1024), b"aes256-gcm@openssh.com")
        self.assertEqual(reader.string(1024), b"bcrypt")
        options = probe.native.Reader(reader.string(1024))
        self.assertEqual(options.string(32), b"s" * 16)
        self.assertEqual(options.number(), 4294967295)
        options.finish()

    def test_valid_64_rounds_cannot_be_used_as_a_negative_fixture(self):
        with self.assertRaises(probe.ProbeError):
            probe.mutated_key(decoded_fixture(), "reject-rounds-64")

    def test_large_declared_field_is_truncated_and_still_under_the_input_cap(self):
        contents = probe.mutated_key(decoded_fixture(), "reject-large-field")
        self.assertLess(len(contents), 65536)
        reader = probe.native.Reader(decode_pem(contents))
        reader.take(15)
        for _ in range(3):
            reader.string(1024)
        self.assertEqual(reader.number(), 1)
        reader.string(128)
        self.assertEqual(reader.number(), 1024 * 1024)
        self.assertEqual(reader.take(256), b"x" * 256)
        reader.finish()

    def test_mutation_updates_both_digest_and_size_in_the_manifest(self):
        backend = probe.session.package_backend()
        original = {"version": 1, "key_size": 7, "key_sha256": "0" * 64}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base, destination = root / "base", root / "variant"
            base.mkdir()

            def read_manifest(path):
                if path == base:
                    return original, b""
                data = (path / "manifest.json").read_bytes()
                return json.loads(data), data

            with patch.object(backend, "read_manifest", side_effect=read_manifest), \
                    patch.object(probe.native, "parse_key", return_value={"decoded": decoded_fixture()}), \
                    patch.object(probe.native, "private_write", side_effect=lambda path, data: path.write_bytes(data)), \
                    patch.object(probe.native, "read_private_file", side_effect=lambda path: path.read_bytes()):
                probe.create_variant(base, destination, "reject-rounds-63")
            contents = (destination / "key.openssh").read_bytes()
            manifest = json.loads((destination / "manifest.json").read_bytes())
        self.assertEqual(manifest["key_size"], len(contents))
        self.assertEqual(manifest["key_sha256"], hashlib.sha256(contents).hexdigest())
        self.assertNotEqual(manifest["key_sha256"], original["key_sha256"])


class CleanupTests(unittest.TestCase):
    def test_uncertain_exit_retains_every_resource_and_primary_diagnostic(self):
        first, second = Mock(), Mock()
        first.stop.side_effect = RuntimeError("private failure")
        provider = Mock()
        bundle = {"credentials": [first, second], "provider": provider, "roots": []}
        report, retained = {}, []
        with patch.object(probe, "CURRENT_REPORT", report), \
                patch.object(probe, "UNCERTAIN_BUNDLES", retained):
            probe.finish(bundle, OSError(13, "private transcript"))
        second.stop.assert_called_once_with()
        first.release.assert_not_called()
        second.release.assert_not_called()
        provider.close.assert_not_called()
        self.assertEqual(retained, [bundle])
        self.assertEqual(report["primary_failure"]["errno"], 13)
        self.assertIn("cleanup_failure", report)
        self.assertNotIn("private", str(report))

    def test_every_native_exit_precedes_any_resource_release(self):
        events = []
        first, second, provider = Mock(), Mock(), Mock()
        first.stop.side_effect = lambda: events.append("stop-first")
        second.stop.side_effect = lambda: events.append("stop-second")
        first.release.side_effect = lambda: events.append("release-first")
        second.release.side_effect = lambda: events.append("release-second")
        provider.close.side_effect = lambda: events.append("close-provider")
        probe.finish({"credentials": [first, second], "provider": provider, "roots": []}, None)
        self.assertEqual(events, ["stop-first", "stop-second", "release-first", "release-second", "close-provider"])

    def test_uncertain_exit_without_primary_does_not_return_success(self):
        credential = Mock()
        credential.stop.side_effect = RuntimeError("uncertain")
        with patch.object(probe, "CURRENT_REPORT", {}), patch.object(probe, "UNCERTAIN_BUNDLES", []):
            with self.assertRaises(probe.ProbeError):
                probe.finish({"credentials": [credential], "provider": Mock(), "roots": []}, None)

    def test_credential_refuses_fd_release_before_native_exit(self):
        value = object.__new__(probe.Credential)
        value.agent = SimpleNamespace(process=Mock(), exit_observed=False, close=Mock())
        value.memfd, value.snapshot = 19, {"retained": True}
        with patch.object(probe.os, "close") as close, patch.object(probe.session, "package_backend") as backend:
            with self.assertRaises(probe.ProbeError):
                value.release()
        close.assert_not_called()
        backend.assert_not_called()
        self.assertEqual(value.memfd, 19)

    def test_resource_release_failure_does_not_erase_the_primary(self):
        credential = Mock()
        credential.release.side_effect = OSError(5, "private cleanup")
        report = {}
        with patch.object(probe, "CURRENT_REPORT", report), patch.object(probe, "UNCERTAIN_BUNDLES", []):
            probe.finish({"credentials": [credential], "provider": Mock(), "roots": []}, RuntimeError("private primary"))
        self.assertEqual(report["primary_failure"]["error_type"], "RuntimeError")
        self.assertEqual(report["cleanup_failure"]["errno"], 5)
        self.assertNotIn("private", str(report))


if __name__ == "__main__":
    unittest.main()
