"""Heap measurement controls without native programs or containers."""

import base64
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, Mock, patch

import heap_probe as probe

with patch.object(os, "geteuid", return_value=0, create=True):
    import package_probe as package_fixture


def observation(**changes):
    value = {"version": 1, "measurement": "additional-live-rust-requested-heap", "accepted": False,
             "baseline_bytes": 1024, "peak_additional_bytes": 1000, "live_after_bytes": 1024,
             "budget_bytes": 2097152, "within_budget": True, "allocator_valid": True,
             "realloc_conservative": True}
    value.update(changes)
    return value


class MeasurementTests(unittest.TestCase):
    def test_one_byte_above_the_heap_ceiling_cannot_use_latency_tolerance(self):
        with self.assertRaises(probe.ProbeError):
            probe.validate_observation(observation(peak_additional_bytes=2097153, within_budget=False), False, 2)

    def test_exact_hard_ceiling_is_accepted(self):
        probe.validate_observation(observation(peak_additional_bytes=2097152), False, 0)

    def test_false_allocator_or_scope_flags_cannot_credit_measurement(self):
        for changes in ({"allocator_valid": False}, {"realloc_conservative": False},
                        {"budget_bytes": 2097153}, {"accepted": True}, {"within_budget": False}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.validate_observation(observation(**changes), False, 0)

    def test_signal_exit_is_not_an_ordinary_measurement(self):
        with self.assertRaises(probe.ProbeError):
            probe.validate_observation(observation(), False, -9)

    def test_live_count_cannot_exceed_recorded_high_water(self):
        with self.assertRaises(probe.ProbeError):
            probe.validate_observation(observation(live_after_bytes=2025), False, 0)

    def test_boolean_or_negative_counters_are_refused(self):
        for invalid in (True, -1, 1.2, float("nan")):
            with self.subTest(invalid=invalid), self.assertRaises(probe.ProbeError):
                probe.parse_observation(json.dumps(observation(peak_additional_bytes=invalid)).encode())

    def test_unknown_or_duplicate_fields_do_not_reach_a_report(self):
        for output in (json.dumps(observation(private_contents="not permitted")).encode(),
                       json.dumps(observation()).replace('"version": 1', '"version": 1, "version": 1').encode()):
            with self.subTest(output=output), self.assertRaises(probe.ProbeError):
                probe.parse_observation(output)

    def test_malformed_or_oversized_output_is_refused(self):
        for output in (b"", b"x" * 4097, b"not json", b"\xff"):
            with self.subTest(size=len(output)), self.assertRaises(probe.ProbeError):
                probe.parse_observation(output)


def decoded_fixture():
    wire = probe.native.wire_string
    return (b"openssh-key-v1\0" + wire(b"aes256-gcm@openssh.com") + wire(b"bcrypt")
            + wire(wire(b"s" * 16) + struct.pack(">I", 64))
            + struct.pack(">I", 1) + wire(b"public-fixture") + wire(b"encrypted-fixture") + b"tag")


def decoded_pem(value):
    return base64.b64decode(b"".join(value.splitlines()[1:-1]), validate=True)


def body_reader(value):
    reader = probe.native.Reader(decoded_pem(value))
    reader.take(15)
    for _ in range(3):
        reader.string(1024)
    reader.number()
    reader.string(128)
    return reader


def valid_outer_package():
    wire = probe.native.wire_string
    public = b"p" * 32
    public_blob = wire(b"ssh-ed25519") + wire(public)
    decoded = (b"openssh-key-v1\0" + wire(b"aes256-gcm@openssh.com") + wire(b"bcrypt")
               + wire(wire(b"s" * 16) + struct.pack(">I", 64)) + struct.pack(">I", 1)
               + wire(public_blob) + wire(b"x" * 32) + b"t" * 16)
    contents = package_fixture.encode_native(decoded)
    manifest = {"version": 1, "profile": package_fixture.PROFILE,
                "device_id": "00000000000000000000000001", "credential_id": "00000000000000000000000002",
                "public_key": public.hex(), "key_sha256": hashlib.sha256(contents).hexdigest(),
                "key_size": len(contents)}
    return contents, manifest


class MutationTests(unittest.TestCase):
    def test_intentional_oversize_fixture_reaches_the_checker_after_bounded_readback(self):
        backend = probe.session.package_backend()
        contents, original = valid_outer_package()
        original_fstat = os.fstat

        def private_stat(value):
            return SimpleNamespace(**{name: getattr(value, name) for name in
                ("st_dev", "st_ino", "st_gid", "st_nlink", "st_size", "st_mtime_ns")},
                # This semantic-fixture test adapts Unix metadata on Windows.
                # Its real path/handle ctime views can differ while other
                # identity fields agree. Separate tests retain strict ctime
                # refusal; the POSIX integration test uses unmodified metadata.
                st_ctime_ns=0 if sys.platform == "win32" else value.st_ctime_ns,
                st_uid=0, st_mode=stat.S_IFREG | 0o600)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base, destination = root / "base", root / "oversize"
            base.mkdir()
            (base / "key.openssh").write_bytes(contents)
            (base / "manifest.json").write_bytes(backend.manifest_bytes(original))

            def read_manifest(path):
                data = (path / "manifest.json").read_bytes()
                return backend.read_manifest_bytes(data)

            with patch.object(backend, "read_manifest", side_effect=read_manifest), \
                    patch.object(probe.native, "parse_key", side_effect=lambda path: probe.native.parse_key_bytes(path.read_bytes())), \
                    patch.object(probe.native, "private_write", side_effect=lambda path, data: path.write_bytes(data)), \
                    patch.object(Path, "lstat", autospec=True,
                                 side_effect=lambda path: private_stat(os.stat(path, follow_symlinks=False))), \
                    patch.object(os, "fstat", side_effect=lambda fd: private_stat(original_fstat(fd))), \
                    patch.object(os, "geteuid", return_value=0, create=True), \
                    patch.object(os, "O_NOFOLLOW", getattr(os, "O_NOFOLLOW", 0), create=True), \
                    patch.object(os, "O_CLOEXEC", getattr(os, "O_CLOEXEC", 0), create=True):
                result = probe.create_variant(base, destination, "file-65537-oversize")
                report = {"cases": []}
                with patch.object(backend, "check", return_value=(1, b"refused")) as check, \
                        patch.object(probe, "observe", return_value=(0, observation())):
                    probe.collect_case(report, {}, "file-65537-oversize", "writer", "heap", result, {})
                check.assert_called_once_with("writer", destination, {})
            self.assertEqual((destination / "key.openssh").stat().st_size, 65537)
            manifest = json.loads((destination / "manifest.json").read_bytes())
            self.assertEqual(manifest["key_size"], 65537)
            self.assertEqual(manifest["key_sha256"], hashlib.sha256(b"x" * 65537).hexdigest())
            with self.assertRaises(package_fixture.ProbeError):
                backend.read_manifest_bytes((destination / "manifest.json").read_bytes())
        self.assertEqual(len(report["cases"][0]["observations"]), 3)

    @unittest.skipUnless(sys.platform == "linux", "requires real POSIX private-file metadata")
    def test_posix_oversize_publication_keeps_semantic_rejection_separate(self):
        backend = probe.session.package_backend()
        contents, manifest = valid_outer_package()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            base, destination = root / "base", root / "oversize"
            base.mkdir(mode=0o700)
            probe.native.private_write(base / "key.openssh", contents)
            probe.native.private_write(base / "manifest.json", backend.manifest_bytes(manifest))
            self.assertEqual(backend.read_manifest(base)[0], manifest)
            self.assertEqual(probe.native.parse_key(base / "key.openssh")["rounds"], 64)
            result = probe.create_variant(base, destination, "file-65537-oversize")
            self.assertEqual(result, destination)
            self.assertEqual((destination / "key.openssh").stat().st_size, 65537)
            raw_manifest = (destination / "manifest.json").read_bytes()
            with self.assertRaises(package_fixture.ProbeError):
                backend.read_manifest_bytes(raw_manifest)
            with self.assertRaises(package_fixture.ProbeError):
                backend.read_manifest(destination)

    def test_body_length_limits_are_encoded_without_allocating_declared_size(self):
        for size in (1048575, 1048576, 4294967295):
            contents = probe.mutated_key(decoded_fixture(), "declared-body-" + str(size))
            self.assertLess(len(contents), 65536)
            reader = body_reader(contents)
            self.assertEqual(reader.number(), size)
            self.assertEqual(reader.take(256), b"x" * 256)
            reader.finish()

    def test_near_cap_input_contains_the_maximum_field_declaration(self):
        contents = probe.mutated_key(decoded_fixture(), "near-cap-large-field")
        self.assertGreaterEqual(len(contents), 65000)
        self.assertLessEqual(len(contents), 65536)
        reader = body_reader(contents)
        self.assertEqual(reader.number(), 1048575)
        self.assertGreater(len(reader.take(len(reader.data) - reader.position)), 47000)

    def test_file_boundaries_are_exact(self):
        self.assertEqual(len(probe.mutated_key(decoded_fixture(), "file-65536-malformed")), 65536)
        self.assertEqual(len(probe.mutated_key(decoded_fixture(), "file-65537-oversize")), 65537)

    def test_round_counter_and_key_count_do_not_modify_other_header_fields(self):
        value = decoded_pem(probe.mutated_key(decoded_fixture(), "rounds-u32max"))
        reader = probe.native.Reader(value)
        reader.take(15)
        self.assertEqual(reader.string(1024), b"aes256-gcm@openssh.com")
        self.assertEqual(reader.string(1024), b"bcrypt")
        options = probe.native.Reader(reader.string(1024))
        self.assertEqual(options.string(32), b"s" * 16)
        self.assertEqual(options.number(), 4294967295)
        options.finish()
        self.assertEqual(reader.number(), 1)
        self.assertEqual(reader.string(128), b"public-fixture")
        value = decoded_pem(probe.mutated_key(decoded_fixture(), "key-count-u32max"))
        prefix, _, _ = probe.header(decoded_fixture())
        self.assertEqual(value[:len(prefix)], prefix)
        self.assertEqual(struct.unpack(">I", value[len(prefix):len(prefix) + 4])[0], 4294967295)

    def test_salt_prefix_is_truncated_inside_a_bounded_options_field(self):
        value = decoded_pem(probe.mutated_key(decoded_fixture(), "declared-salt-1048575"))
        reader = probe.native.Reader(value)
        reader.take(15)
        reader.string(1024)
        reader.string(1024)
        options = reader.string(1024)
        self.assertEqual(struct.unpack(">I", options[:4])[0], 1048575)
        self.assertEqual(len(options), 264)

    def test_variant_publication_updates_digest_and_size_and_reads_them_back(self):
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
                    patch.object(probe, "verify_fixture_bytes", side_effect=lambda path, data, **options: self.assertEqual(path.read_bytes(), data)):
                probe.create_variant(base, destination, "declared-body-1048575")
            contents = (destination / "key.openssh").read_bytes()
            manifest = json.loads((destination / "manifest.json").read_bytes())
        self.assertEqual(manifest["key_size"], len(contents))
        self.assertEqual(manifest["key_sha256"], hashlib.sha256(contents).hexdigest())
        self.assertNotEqual(manifest["key_sha256"], original["key_sha256"])


class FixtureReadbackTests(unittest.TestCase):
    def metadata(self, **changes):
        value = dict(st_dev=1, st_ino=2, st_mode=stat.S_IFREG | 0o600, st_uid=0, st_gid=0,
                     st_nlink=1, st_size=3, st_mtime_ns=4, st_ctime_ns=5)
        value.update(changes)
        return SimpleNamespace(**value)

    def test_fixture_reader_does_not_expand_beyond_the_single_oversize_control(self):
        path = Mock()
        with self.assertRaises(probe.ProbeError):
            probe.verify_fixture_bytes(path, b"x" * 65538)
        path.lstat.assert_not_called()
        self.assertEqual(probe.native.KEY_LIMIT, 65536)

    def test_manifest_readback_keeps_its_distinct_2k_bound(self):
        path = Mock()
        with self.assertRaises(probe.ProbeError):
            probe.verify_fixture_bytes(path, b"x" * 2049, limit=2048)
        path.lstat.assert_not_called()

    def test_unsafe_fixture_metadata_is_refused_before_opening(self):
        for changes in ({"st_mode": stat.S_IFLNK | 0o600}, {"st_nlink": 2}, {"st_uid": 1},
                        {"st_mode": stat.S_IFREG | 0o640}, {"st_size": 4}):
            with self.subTest(changes=changes), patch.object(os, "geteuid", return_value=0, create=True), \
                    patch.object(os, "open") as opened:
                path = Mock()
                path.lstat.return_value = self.metadata(**changes)
                with self.assertRaises(probe.ProbeError):
                    probe.verify_fixture_bytes(path, b"abc")
                opened.assert_not_called()

    def readback(self, before, opened, after, path_after, data=b"abc"):
        source = Mock()
        source.fileno.return_value = 42
        source.read.return_value = data
        manager = MagicMock()
        manager.__enter__.return_value = source
        path = Mock()
        path.lstat.side_effect = [before, path_after]
        with patch.object(os, "geteuid", return_value=0, create=True), \
                patch.object(os, "O_NOFOLLOW", getattr(os, "O_NOFOLLOW", 0), create=True), \
                patch.object(os, "O_CLOEXEC", getattr(os, "O_CLOEXEC", 0), create=True), \
                patch.object(os, "open", return_value=42), patch.object(os, "fdopen", return_value=manager), \
                patch.object(os, "fstat", side_effect=[opened, after]):
            probe.verify_fixture_bytes(path, b"abc")
        source.read.assert_called_once_with(3)
        manager.__exit__.assert_called_once()

    def test_opening_a_different_inode_is_refused(self):
        before = self.metadata()
        with self.assertRaises(probe.ProbeError):
            self.readback(before, self.metadata(st_ino=99), before, before)

    def test_ctime_changes_at_every_readback_seam_are_refused(self):
        before = self.metadata()
        for position in range(3):
            observations = [before, before, before]
            observations[position] = self.metadata(st_ctime_ns=6)
            with self.subTest(position=position), self.assertRaises(probe.ProbeError):
                self.readback(before, *observations)

    def test_growth_or_path_replacement_during_readback_is_refused(self):
        before = self.metadata()
        for after, path_after in ((self.metadata(st_size=4), before), (before, self.metadata(st_ino=99))):
            with self.subTest(after=after, path_after=path_after), self.assertRaises(probe.ProbeError):
                self.readback(before, before, after, path_after)

    def test_byte_mismatch_is_refused(self):
        before = self.metadata()
        with self.assertRaises(probe.ProbeError):
            self.readback(before, before, before, before, b"abd")

    def test_stable_private_fixture_uses_an_exact_bounded_read(self):
        before = self.metadata()
        self.readback(before, before, before, before)


class RetentionTests(unittest.TestCase):
    def test_over_budget_observation_is_retained_before_rejection(self):
        report, bundle = {"cases": []}, {}
        excessive = observation(peak_additional_bytes=2097153, within_budget=False)
        with patch.object(probe.session, "package_backend") as backend, \
                patch.object(probe, "observe", return_value=(2, excessive)):
            backend.return_value.check.return_value = (1, b"")
            with self.assertRaises(probe.ProbeError):
                probe.collect_case(report, bundle, "rounds-u32max", "writer", "heap", Path("unused"), {})
        self.assertEqual(report["cases"][0]["observations"], [excessive])
        self.assertEqual(report["cases"][0]["peak_additional_bytes"], 2097153)

    def test_second_failed_measurement_keeps_the_first_and_failed_observation(self):
        report, bundle = {"cases": []}, {}
        first, failed = observation(), observation(allocator_valid=False)
        with patch.object(probe.session, "package_backend") as backend, \
                patch.object(probe, "observe", side_effect=[(0, first), (1, failed)]):
            backend.return_value.check.return_value = (1, b"")
            with self.assertRaises(probe.ProbeError):
                probe.collect_case(report, bundle, "rounds-u32max", "writer", "heap", Path("unused"), {})
        self.assertEqual(report["cases"][0]["observations"], [first, failed])

    def test_checker_semantic_disagreement_prevents_heap_dispatch(self):
        with patch.object(probe.session, "package_backend") as backend, \
                patch.object(probe, "observe") as observe:
            backend.return_value.check.return_value = (0, b"")
            with self.assertRaises(probe.ProbeError):
                probe.collect_case({"cases": []}, {}, "rounds-u32max", "writer", "heap", Path("unused"), {})
        observe.assert_not_called()

    def test_complete_case_has_three_independent_observations_and_exact_maximum(self):
        report = {"cases": []}
        values = [observation(peak_additional_bytes=size) for size in (900, 1100, 1000)]
        with patch.object(probe.session, "package_backend") as backend, \
                patch.object(probe, "observe", side_effect=[(0, value) for value in values]) as observe:
            backend.return_value.check.return_value = (1, b"")
            result = probe.collect_case(report, {}, "rounds-u32max", "writer", "heap", Path("unused"), {})
        self.assertEqual(observe.call_count, 3)
        self.assertEqual(result["observations"], values)
        self.assertEqual(result["peak_additional_bytes"], 1100)
        self.assertTrue(probe.validate_case(result))

    def test_final_control_cannot_be_credited_from_a_failed_instrumented_check(self):
        report = {"retained_control_accepted": False}
        rejected = observation(accepted=False)
        with patch.object(probe.session, "package_backend") as backend, \
                patch.object(probe, "observe", return_value=(0, rejected)):
            backend.return_value.check.return_value = (0, b"")
            with self.assertRaises(probe.ProbeError):
                probe.retain_control(report, {}, "writer", "heap", Path("unused"), {})
        self.assertFalse(report["retained_control_accepted"])
        self.assertEqual(report["retained_control_observation"], rejected)

    def test_failed_child_operation_leaves_exit_uncertain(self):
        bundle = {"process_exit_uncertain": False}
        with self.assertRaises(RuntimeError):
            probe.quiescent_call(bundle, Mock(side_effect=RuntimeError("failure")))
        self.assertTrue(bundle["process_exit_uncertain"])

    def test_summary_cannot_understate_raw_peak_or_shortened_case(self):
        value = {"case": "rounds-u32max", "expected_accepted": False, "baseline_checker_exit_code": 1,
                 "mutation_rehashed": True, "observations": [observation()] * 3, "peak_additional_bytes": 999}
        with self.assertRaises(probe.ProbeError):
            probe.validate_case(value)
        value["peak_additional_bytes"] = 1000
        value["observations"].pop()
        with self.assertRaises(probe.ProbeError):
            probe.validate_case(value)

    def test_uncertain_process_exit_keeps_provider_and_trees(self):
        provider = Mock()
        bundle = {"process_exit_uncertain": True, "provider": provider, "roots": []}
        with patch.object(probe, "CURRENT_REPORT", {}), patch.object(probe, "UNCERTAIN_BUNDLES", []):
            with self.assertRaises(probe.ProbeError):
                probe.finish(bundle, None)
        provider.close.assert_not_called()

    def test_primary_failure_survives_cleanup_failure_without_error_text(self):
        report = {}
        bundle = {"process_exit_uncertain": False, "provider": Mock(), "roots": []}
        bundle["provider"].close.side_effect = OSError(13, "private contents")
        with patch.object(probe, "CURRENT_REPORT", report), patch.object(probe, "UNCERTAIN_BUNDLES", []):
            probe.finish(bundle, RuntimeError("private primary"))
        self.assertIn("primary_failure", report)
        self.assertIn("cleanup_failure", report)
        self.assertNotIn("private", str(report))


if __name__ == "__main__":
    unittest.main()
