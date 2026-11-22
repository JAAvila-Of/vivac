"""Pure guards for explicitly retained disposable namespace entries."""
import hashlib
import json
import copy
import stat
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import creation_namespace_probe as probe


CREDENTIAL = "0" * 25 + "1"
OTHER = "0" * 25 + "2"
STAGE = ".vivac-openssh-preparing-" + "a" * 32
NEXT_STAGE = ".vivac-openssh-preparing-" + "b" * 32


class NamespaceGuards(unittest.TestCase):
    def test_only_explicit_canonical_names_are_preservable(self):
        self.assertEqual(probe.validated_names((CREDENTIAL, STAGE), (OTHER,)),
                         ((CREDENTIAL, STAGE), (OTHER,)))
        self.assertEqual(probe.validated_names(), ((), ()))
        for name in ("", ".", "..", "/tmp/entry", "a/b", "a\\b", "0" * 25 + "i",
                     "8" + "0" * 25, ".pending-" + "a" * 64,
                     ".vivac-openssh-preparing-" + "a" * 31,
                     ".vivac-openssh-preparing-" + "A" * 32, None, 1, True):
            with self.subTest(name=name), self.assertRaises(Exception):
                probe.validated_names((name,), ())
        with self.assertRaises(Exception):
            probe.validated_names((), (STAGE,))

    def test_duplicate_or_unbounded_selection_is_refused(self):
        for packages, admissions in (((CREDENTIAL, CREDENTIAL), ()), ((), (OTHER, OTHER)),
                                     (CREDENTIAL, ()), ((), OTHER), (None, ()),
                                     (tuple(f"{number:026d}" for number in range(17)), ())):
            with self.subTest(packages=packages, admissions=admissions), self.assertRaises(Exception):
                probe.validated_names(packages, admissions)

    def test_stage_selection_preserves_exact_foreign_membership(self):
        self.assertEqual(probe.select_stage([STAGE]), STAGE)
        self.assertEqual(probe.select_stage([CREDENTIAL, STAGE, NEXT_STAGE], (CREDENTIAL, STAGE)), NEXT_STAGE)
        for members, preserved in (([STAGE, OTHER], ()), ([STAGE, NEXT_STAGE], ()),
                                   ([STAGE], (CREDENTIAL,)), ([STAGE, OTHER], (CREDENTIAL,)),
                                   ([STAGE], (STAGE,)), ([CREDENTIAL, OTHER], (CREDENTIAL,)),
                                   ([CREDENTIAL, STAGE, STAGE], (CREDENTIAL,))):
            with self.subTest(members=members, preserved=preserved), self.assertRaises(Exception):
                probe.select_stage(members, preserved)

    def test_package_snapshot_returns_the_selected_stage_after_reading_both_members(self):
        draft = probe.draft
        key = b"synthetic encrypted package bytes"
        public_key = bytes.fromhex("11" * 32)
        manifest = draft.canonical(dict(version=1, profile="openssh-ed25519-aes256gcm-bcrypt-v1",
            device_id=CREDENTIAL, credential_id=OTHER, public_key=public_key.hex(),
            key_sha256=hashlib.sha256(key).hexdigest(), key_size=len(key)))
        record = dict(device_id=CREDENTIAL, credential_id=OTHER, public_key=public_key.hex(),
            ciphertext_sha256=hashlib.sha256(key).hexdigest(), ciphertext_len=len(key),
            manifest_sha256=hashlib.sha256(manifest).hexdigest(), manifest_len=len(manifest))

        def info(inode, mode, links, size):
            return SimpleNamespace(st_dev=1, st_ino=inode, st_mode=mode, st_uid=65534, st_gid=65534,
                                   st_nlink=links, st_size=size, st_mtime_ns=11, st_ctime_ns=12)

        metadata = {10: info(100, stat.S_IFDIR | 0o700, 2, 64),
                    20: info(200, stat.S_IFDIR | 0o700, 2, 64),
                    21: info(201, stat.S_IFREG | 0o600, 1, len(key)),
                    22: info(202, stat.S_IFREG | 0o600, 1, len(manifest))}
        descriptors = {(10, STAGE): 20, (20, "key.openssh"): 21, (20, "manifest.json"): 22}
        contents = {21: key, 22: manifest}
        parsed = dict(cipher="aes256-gcm@openssh.com", kdf="bcrypt", rounds=64, public_key=public_key)
        with patch.object(draft.os, "listdir", side_effect=lambda fd: [STAGE] if fd == 10 else ["key.openssh", "manifest.json"]), \
             patch.object(draft.os, "open", side_effect=lambda name, flags, *, dir_fd: descriptors[(dir_fd, name)]), \
             patch.object(draft.os, "fstat", side_effect=metadata.__getitem__), \
             patch.object(draft.os, "pread", side_effect=lambda fd, count, offset: contents[fd][:count]), \
             patch.object(draft.os, "listxattr", return_value=[]), \
             patch.object(draft.os, "stat", side_effect=lambda name, *, dir_fd, follow_symlinks: metadata[descriptors[(dir_fd, name)]]), \
             patch.object(draft.native, "parse_key_bytes", return_value=parsed):
            snapshot = draft.snapshot_package(10, record, b"unrelated-secret")
        self.assertEqual(snapshot[0], STAGE)
        self.assertEqual(snapshot[1], 20)
        self.assertEqual(set(snapshot[3]), {"key.openssh", "manifest.json"})

    def test_public_summary_hashes_bytes_without_disclosing_them(self):
        raw = b"synthetic encrypted member for namespace evidence"
        metadata = tuple(range(9))
        value = probe.entry_summary(CREDENTIAL, metadata, {"key.openssh": (17, raw, metadata)})
        self.assertEqual(set(value), {"name", "metadata", "members"})
        self.assertEqual(value["name"], CREDENTIAL)
        self.assertEqual(value["metadata"], list(metadata))
        self.assertEqual(value["members"], [{"name": "key.openssh", "metadata": list(metadata),
                                           "sha256": hashlib.sha256(raw).hexdigest()}])
        self.assertNotIn(raw.decode("ascii"), json.dumps(value))
        self.assertNotIn("fd", json.dumps(value))

    def test_summary_rejects_false_shapes_metadata_and_provenance(self):
        directory = [1, 2, stat.S_IFDIR | 0o700, 65534, 65534, 2, 64, 1, 2]
        member = [1, 3, stat.S_IFREG | 0o600, 65534, 65534, 1, 8, 1, 2]
        value = {"packages": [], "admissions": [{"name": CREDENTIAL, "metadata": directory,
                 "members": [{"name": "record.json", "metadata": member, "sha256": "a" * 64}]}]}
        self.assertIs(probe.validate_summary(value), value)
        for change in ("extra", "wrong-device", "owner", "hardlink", "digest", "stage-admission", "duplicate"):
            altered = copy.deepcopy(value)
            entry = altered["admissions"][0]
            if change == "extra":
                entry["raw"] = "unselected contents"
            elif change == "wrong-device":
                entry["members"][0]["metadata"][0] = 2
            elif change == "owner":
                entry["metadata"][3] = 0
            elif change == "hardlink":
                entry["members"][0]["metadata"][5] = 2
            elif change == "digest":
                entry["members"][0]["sha256"] = "A" * 64
            elif change == "stage-admission":
                entry["name"] = STAGE
            else:
                altered["admissions"].append(copy.deepcopy(entry))
            with self.subTest(change=change), self.assertRaises(Exception):
                probe.validate_summary(altered)


if __name__ == "__main__":
    unittest.main()
