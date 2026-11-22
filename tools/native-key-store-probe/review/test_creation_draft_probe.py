"""Public-schema guards for the disposable rooted draft observer."""
import copy
import hashlib
from pathlib import Path, PureWindowsPath
import tempfile
import unittest
from unittest.mock import patch

import creation_draft_probe as probe


class SchemaTests(unittest.TestCase):
    def record(self):
        return dict(version=2, installation_id="0" * 25 + "1", creation_id="0" * 25 + "2",
                    device_id="0" * 25 + "3", credential_id="0" * 25 + "4",
                    public_key="d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                    ciphertext_sha256=hashlib.sha256(b"cipher").hexdigest(),
                    manifest_sha256=hashlib.sha256(b"manifest").hexdigest(),
                    ciphertext_len=6, manifest_len=8, creation_profile_revision=1, secret_policy_revision=1)

    def packet(self, final=False):
        value = dict(version=1, stage="final" if final else "draft", pid=42,
                     record_canonical=probe.canonical(self.record()).decode("ascii"),
                     attended_confirmation_completed=True, **dict.fromkeys(probe.FALSE_FLAGS, False))
        if final:
            value.update(draft_revalidated=True, preparation_dropped=True)
        return value

    def test_exact_record_and_packet(self):
        record = self.record()
        self.assertEqual(probe.parse_record(probe.canonical(record).decode("ascii")), record)
        for final in (False, True):
            value = self.packet(final)
            self.assertEqual(probe.parse_packet(probe.canonical(value), 42, final=final), value)

    def test_rejects_duplicate_extra_missing_and_types(self):
        value = self.packet()
        invalid = [b'{"version":1,' + probe.canonical(value)[1:]]
        for field in value:
            changed = copy.deepcopy(value)
            del changed[field]
            invalid.append(probe.canonical(changed))
        for field, replacement in (("pid", True), ("version", True),
                                   ("attended_confirmation_completed", 1), ("creation_admission", 0)):
            changed = dict(value, **{field: replacement})
            invalid.append(probe.canonical(changed))
        invalid.append(probe.canonical(dict(value, authority=True)))
        for payload in invalid:
            with self.subTest(payload=payload), self.assertRaises(Exception):
                probe.parse_packet(payload, 42)

    def test_all_authority_flags_must_remain_false(self):
        for field in probe.FALSE_FLAGS:
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.parse_packet(probe.canonical(dict(self.packet(), **{field: True})), 42)

    def test_record_requires_exact_canonical_order_and_bounds(self):
        record = self.record()
        invalid = [probe.canonical(record).decode("ascii") + "\n",
                   probe.canonical(dict(reversed(list(record.items())))).decode("ascii")]
        for field, replacement in (("version", True), ("ciphertext_len", 65537),
                                   ("manifest_len", 2049), ("public_key", "A" * 64),
                                   ("installation_id", "z" * 26), ("secret_policy_revision", 2)):
            invalid.append(probe.canonical(dict(record, **{field: replacement})).decode("ascii"))
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(Exception):
                probe.parse_record(value)

    def test_final_requires_boolean_revalidation_and_drop(self):
        for field, replacement in (("draft_revalidated", 1), ("preparation_dropped", False)):
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.parse_packet(probe.canonical(dict(self.packet(True), **{field: replacement})), 42, final=True)

    def test_record_must_match_original_bytes(self):
        record = self.record()
        probe.check_hashes(record, b"cipher", b"manifest")
        for key, manifest in ((b"Cipher", b"manifest"), (b"cipher", b"Manifest")):
            with self.assertRaises(Exception):
                probe.check_hashes(record, key, manifest)

    def observation(self, case="valid"):
        value = dict(version=1, experiment="rooted-creation-draft", case=case, status="observed",
                     **dict.fromkeys(probe.FALSE_FLAGS, False))
        for field in ("process_exit_observed", "terminal_restored", "standard_streams_clean",
                      "helper_unchanged", "sources_unchanged", "root_binding_unchanged",
                      "process_identity_verified", "private_terminal_observed", "secret_boundaries_checked",
                      "original_package_verified", "preparation_dropped", "admissions_empty", "final_slot_absent"):
            value[field] = True
        for field in ("helper_sha256", "observer_sha256", "real_binding_sha256", "decoy_binding_sha256"):
            value[field] = "1" * 64
        value.update(record_canonical=probe.canonical(self.record()).decode("ascii"),
                     selected_installation_id=self.record()["installation_id"], draft_revalidated=True,
                     mutation_observed=False, invalidated_stage_preserved=False)
        return value

    def test_host_observation_requires_evidence_without_authority(self):
        value = self.observation()
        self.assertEqual(probe.validate_observation(value, "valid"), value)
        for field in ("process_exit_observed", "terminal_restored", "secret_boundaries_checked"):
            for replacement in (False, 1, None):
                with self.subTest(field=field, replacement=replacement), self.assertRaises(Exception):
                    probe.validate_observation(dict(value, **{field: replacement}), "valid")
        for field in probe.FALSE_FLAGS:
            with self.assertRaises(Exception):
                probe.validate_observation(dict(value, **{field: True}), "valid")

    def test_mutation_never_becomes_revalidated(self):
        for case in probe.MUTATIONS:
            value = self.observation(case)
            with self.subTest(case=case), self.assertRaises(Exception):
                probe.validate_observation(value, case)
            value.update(draft_revalidated=False, mutation_observed=True, invalidated_stage_preserved=True)
            self.assertEqual(probe.validate_observation(value, case), value)

    def test_refusal_requires_specific_external_circumstance(self):
        for case, evidence in (("missing-root", "missing_root_observed"), ("mount-packages", "mount_boundary_observed")):
            value = self.observation(case)
            del value["record_canonical"]
            value["refusal_before_disclosure"] = True
            if case == "missing-root":
                value["real_binding_sha256"] = None
            with self.subTest(case=case), self.assertRaises(Exception):
                probe.validate_observation(value, case)
            value[evidence] = True
            self.assertEqual(probe.validate_observation(value, case), value)

    def test_occupied_refusals_require_exact_boolean_evidence(self):
        for case in probe.OCCUPIED:
            self.assertIn(case, probe.CASES)
            self.assertIn(case, probe.REFUSALS)
            value = self.observation(case)
            del value["record_canonical"]
            value.update(refusal_before_disclosure=True, occupied_slot_observed=True, occupied_slot_preserved=True)
            self.assertEqual(probe.validate_observation(value, case), value)
            for field in ("occupied_slot_observed", "occupied_slot_preserved"):
                for replacement in (False, 1, "true", None):
                    with self.subTest(case=case, field=field, replacement=replacement), self.assertRaises(Exception):
                        probe.validate_observation(dict(value, **{field: replacement}), case)
                missing = dict(value)
                del missing[field]
                with self.assertRaises(Exception):
                    probe.validate_observation(missing, case)
            with self.assertRaises(Exception):
                probe.validate_observation(dict(value, record_canonical=probe.canonical(self.record()).decode("ascii")), case)

    def test_mount_boundary_requires_distinct_mount_on_same_device(self):
        parent = "1 0 8:1 / /var/lib/vivac rw - ext4 /dev/x rw\n"
        child = "2 1 8:1 /packages " + probe.PACKAGES.as_posix() + " rw - ext4 /dev/x rw\n"
        self.assertTrue(probe.mount_boundary(parent + child))
        with patch.object(probe, "PACKAGES", PureWindowsPath("/var/lib/vivac/custody-v1/users/65534/state/packages")):
            self.assertTrue(probe.mount_boundary(parent + child))
        for raw in (parent, parent + child.replace("2 1 8:1", "2 1 8:2"),
                    parent + child.replace("2 1 8:1", "1 1 8:1")):
            with self.assertRaises(Exception):
                probe.mount_boundary(raw)

    def test_missing_installation_allows_empty_mountpoint_and_rejects_anchor(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.assertTrue(probe.require_missing_installation(root))
            anchor = root / "custody-v1"
            anchor.mkdir()
            with self.assertRaises(Exception):
                probe.require_missing_installation(root)
            anchor.rmdir()
            anchor.symlink_to(root / "absent", target_is_directory=True)
            with self.assertRaises(Exception):
                probe.require_missing_installation(root)
            anchor.unlink()
            (root / "foreign").write_bytes(b"foreign")
            with self.assertRaises(Exception):
                probe.require_missing_installation(root)


if __name__ == "__main__":
    unittest.main()
