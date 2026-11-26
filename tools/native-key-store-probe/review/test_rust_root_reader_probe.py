"""Candidate reader evidence and public fixture control-plane regressions."""
import copy
import io
import json
import stat
import tarfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import rust_root_reader_probe as probe
import run_lifecycle_probe as runner

CASES = {"valid", "wrong-root-owner", "writable-ancestor", "wrong-prefix-mode", "wrong-slot-owner",
         "binding-owner", "binding-mode", "binding-malformed", "binding-oversized", "binding-identity",
         "ancestor-symlink", "binding-symlink", "missing-binding", "state-mode", "state-acl",
         "snapshot-private-replacement", "unrelated-entry", "binding-hardlink", "binding-fifo"}
SCOPES = {"custody_accredited", "creation_admission", "producer_origin_checked", "root_bootstrap_accredited",
          "provisioning_accredited", "endpoint_authority_accredited", "product_recovery", "same_store_recovery",
          "mcp_integration", "server_integration", "lock_ack"}

def fixture(case, *, observation=False):
    value = dict.fromkeys(SCOPES, False)
    value.update(version=1, case=case, status="observed", identity={"host_pid":20,"creator_tid":20,"children":[]},
        reader_opened=case in {"valid", "snapshot-private-replacement", "unrelated-entry"},
        snapshot_invalidated=case == "snapshot-private-replacement", access_policy_checks=int(case == "state-acl"),
        resources_closed_after_exit=not observation, pressure_threads=4, pressure_iterations=0 if observation else 10)
    if not observation:
        value.update(process_identities_verified=True, host_retained_until_observation=True, host_exit_code=0,
            resources_closed_after_observed_exit=True, binding_unchanged=True,
            fixture_owners_verified=True,
            fixture_unchanged=case != "snapshot-private-replacement")
    return value

def observation():
    value = dict.fromkeys(SCOPES, False)
    value.update(version=1, experiment="rust-root-reader", status="observed", cases=[fixture(case) for case in sorted(CASES)],
        candidate_root_reader_observed=True, resources_closed_after_observed_exit=True,
        helper_sha256="1" * 64, observer_sha256="2" * 64, kernel_release="6.8.0-fixture",
        resource_limits={"memory_bytes":536870912,"swap_bytes":0,"pids":128,"cpu_quota":100000,
                         "cpu_period":100000,"temporary_bytes":134217728})
    return value

class EvidenceTests(unittest.TestCase):
    def test_reader_refusal_packet_is_retained_only_as_bounded_incomplete_diagnostic(self):
        context = {"case":"snapshot-private-replacement", "stage":"action-packet"}
        payload = json.dumps({"stage":"reader-refusal", "diagnostic":"directory-4: errno=22"}).encode("ascii")
        with self.assertRaises(probe.ReaderRefusal) as caught:
            probe.parse_action_packet(payload)
        self.assertEqual(probe.failure_diagnostic(caught.exception, context),
                         {"kind":"ReaderRefusal", "case":context["case"], "stage":"action-packet",
                          "diagnostic":"directory-4: errno=22"})
        action = fixture("valid", observation=True)
        self.assertEqual(probe.parse_action_packet(json.dumps({"stage":"observation", "payload":action}).encode("ascii")), action)
        for value in ({"stage":"reader-refusal", "diagnostic":"x" * 513},
                      {"stage":"reader-refusal", "diagnostic":"\u00f1"},
                      {"stage":"reader-refusal", "diagnostic":1},
                      {"stage":"reader-refusal", "diagnostic":"safe", "contents":"extra"},
                      {"stage":"other", "diagnostic":"safe"}):
            with self.subTest(value=value), self.assertRaises(ValueError) as caught:
                probe.parse_action_packet(json.dumps(value).encode("ascii"))
            self.assertNotIsInstance(caught.exception, probe.ReaderRefusal)
            self.assertNotIn("diagnostic", probe.failure_diagnostic(caught.exception, context))

    def test_failure_diagnostic_uses_only_safe_case_stage_and_kind_enums(self):
        diagnostic = {"case":"snapshot-private-replacement", "stage":"action-packet"}
        self.assertEqual(probe.failure_diagnostic(ValueError("private diagnostic"), diagnostic),
                         {"kind":"ValueError", "case":"snapshot-private-replacement", "stage":"action-packet"})
        class UnexpectedError(Exception): pass
        self.assertEqual(probe.failure_diagnostic(UnexpectedError("private diagnostic"),
                         {"case":"private path", "stage":"private contents"}),
                         {"kind":"Exception", "case":"none", "stage":"unknown"})
        value = observation()
        value["failure"] = probe.failure_diagnostic(ValueError(), diagnostic)
        with self.assertRaises(ValueError): probe.validate_observation(value)

    def test_complete_matrix_and_observation_stages(self):
        self.assertEqual(probe.CASES, CASES)
        probe.validate_observation(observation())
        for case in CASES:
            probe.validate_result(fixture(case, observation=True), case, observation=True)

    def test_report_refuses_missing_duplicate_or_unknown_cases_and_claims(self):
        for mutate in (lambda v: v["cases"].pop(), lambda v: v["cases"].__setitem__(0, v["cases"][1]),
                       lambda v: v.update(candidate_root_reader_observed=False),
                       lambda v: v.update(provisioning_accredited=True), lambda v: v.update(syscall_trace=True)):
            value = observation()
            mutate(value)
            with self.assertRaises(ValueError): probe.validate_observation(value)

    def test_case_refuses_inexact_counters_booleans_identity_and_scope(self):
        for field, changed in (("reader_opened", 1), ("snapshot_invalidated", True),
            ("pressure_threads", True), ("pressure_iterations", 0), ("host_exit_code", True),
            ("access_policy_checks", True), ("binding_unchanged", False), ("fixture_unchanged", False),
            ("resources_closed_after_observed_exit", False), ("creation_admission", True)):
            value = fixture("valid")
            value[field] = changed
            with self.subTest(field=field), self.assertRaises(ValueError): probe.validate_case(value)
        value = fixture("valid")
        value["identity"]["children"] = [{"pid":21}]
        with self.assertRaises(ValueError): probe.validate_case(value)

    def test_case_requires_boolean_verified_fixture_owners(self):
        for changed in (False, 1, None):
            value = fixture("valid")
            value["fixture_owners_verified"] = changed
            with self.subTest(changed=changed), self.assertRaises(ValueError): probe.validate_case(value)

    def test_replacement_and_acl_evidence_are_required(self):
        for case, field, changed in (("snapshot-private-replacement", "snapshot_invalidated", False),
            ("snapshot-private-replacement", "fixture_unchanged", True), ("state-acl", "access_policy_checks", 0),
            ("binding-fifo", "reader_opened", True)):
            value = fixture(case)
            value[field] = changed
            with self.subTest(case=case), self.assertRaises(ValueError): probe.validate_case(value)

    def test_limits_and_digests_require_exact_types_and_bounds(self):
        for mutate in (lambda v: v["resource_limits"].update(memory_bytes=536870913),
                       lambda v: v["resource_limits"].update(pids=True),
                       lambda v: v.update(helper_sha256="z" * 64), lambda v: v.update(kernel_release="bad\n")):
            value = observation()
            mutate(value)
            with self.assertRaises(ValueError): probe.validate_observation(value)

class FixtureTests(unittest.TestCase):
    def test_snapshot_does_not_read_fifo_or_follow_symbolic_aliases(self):
        class Entry:
            def __init__(self, name, mode, children=()):
                self.name, self.mode, self.children = name, mode, children
            def lstat(self):
                return SimpleNamespace(st_mode=self.mode, st_uid=0, st_gid=0, st_dev=1, st_ino=2, st_nlink=1)
            def iterdir(self): return iter(self.children)
            def __lt__(self, other): return self.name < other.name
        root = Entry("root", stat.S_IFDIR | 0o755, (
            Entry("binding.json", stat.S_IFIFO | 0o644), Entry("alias", stat.S_IFLNK | 0o777)))
        with patch.object(probe.os, "open", side_effect=AssertionError("special entry must not be opened")), \
             patch.object(probe.os, "readlink", return_value="binding.json"):
            snapshot = probe.fixture_snapshot(root)
        self.assertIsNone(snapshot["./binding.json"][1])
        self.assertEqual(snapshot["./alias"][1], "binding.json")

    def test_preseeded_store_guard_keeps_mount_filesystem_identity_and_acl_checks(self):
        package = Mock(EXT_MAGIC=123)
        package.filesystem_magic.return_value = 123
        package.is_mountpoint.return_value = True
        package.acl_xattrs.return_value = []
        metadata = SimpleNamespace(st_mode=0o40700, st_uid=65534, st_gid=65534)
        entries = [Path(case) for case in CASES]
        with patch.object(probe.session, "package_backend", return_value=package), \
             patch.object(Path, "is_symlink", return_value=False), \
             patch.object(Path, "lstat", return_value=metadata), \
             patch.object(Path, "iterdir", return_value=iter(entries)), \
             patch.object(probe.os, "geteuid", return_value=65534, create=True):
            probe.require_fixture_store(Path("/store/run"))
            for field, changed in (("st_uid",0), ("st_gid",0), ("st_mode",0o40755)):
                before = getattr(metadata, field)
                setattr(metadata, field, changed)
                with self.subTest(field=field), self.assertRaises(ValueError):
                    probe.require_fixture_store(Path("/store/run"))
                setattr(metadata, field, before)
            package.acl_xattrs.return_value = ["system.posix_acl_access"]
            with self.assertRaises(ValueError): probe.require_fixture_store(Path("/store/run"))

    def archive(self):
        with patch.object(runner, "command") as command:
            runner.initialize_root_reader_fixtures("a" * 64)
        self.assertEqual(command.call_args.args[0], ["docker","cp","-","a" * 64 + ":/store/run"])
        return tarfile.open(fileobj=io.BytesIO(command.call_args.kwargs["payload"]))

    def test_real_fixture_owners_are_checked_for_every_entry_and_exact_exception(self):
        with self.archive() as archive:
            members = list(archive)
        for case in CASES:
            snapshot = {}
            for member in members:
                if member.name.split('/')[0] != case: continue
                relative = "." + member.name[len(case):]
                snapshot[relative] = ((member.mode,member.uid,member.gid,1,2,1), None)
            probe.verify_fixture_owners(snapshot, case)
            for relative, entry in snapshot.items():
                for field in (1, 2):
                    corrupted = copy.deepcopy(snapshot)
                    metadata = list(entry[0])
                    metadata[field] = 0 if metadata[field] else 65534
                    corrupted[relative] = (tuple(metadata), entry[1])
                    with self.subTest(case=case, relative=relative, field=field), self.assertRaises(ValueError):
                        probe.verify_fixture_owners(corrupted, case)
            corrupted = {relative:((metadata[0],65534,65534,*metadata[3:]), contents)
                         for relative,(metadata,contents) in snapshot.items()}
            with self.subTest(case=case, all_user_owned=True), self.assertRaises(ValueError):
                probe.verify_fixture_owners(corrupted, case)

    def test_incorrect_real_owners_refuse_before_starting_helper(self):
        diagnostic = {}
        snapshot = {".":((0o40755,65534,65534,1,2,1),None)}
        with patch.object(probe, "fixture_snapshot", return_value=snapshot), \
             patch.object(probe.subprocess, "Popen") as child:
            with self.assertRaises(ValueError):
                probe.run_case(Path("helper"), Path("fixtures"), "valid", {}, diagnostic=diagnostic)
        child.assert_not_called()
        self.assertEqual(diagnostic, {"case":"valid", "stage":"fixture-owners"})

    def test_fixtures_preserve_actual_administrator_ownership_and_public_binding(self):
        with self.archive() as archive:
            members = {m.name:m for m in archive}
            slot = "valid/var/lib/vivac/custody-v1/users/65534"
            for name in ("valid", "valid/var", "valid/var/lib", "valid/var/lib/vivac", "valid/var/lib/vivac/custody-v1", "valid/var/lib/vivac/custody-v1/users"):
                self.assertEqual((members[name].uid,members[name].gid,members[name].mode), (0,0,0o755))
            self.assertEqual((members[slot].uid,members[slot].gid,members[slot].mode), (0,0,0o755))
            binding = members[slot + "/binding.json"]
            self.assertEqual((binding.uid,binding.gid,binding.mode), (0,0,0o644))
            self.assertEqual(archive.extractfile(binding).read(), b'{"version":1,"installation_id":"00000000000000000000000001","owner_uid":65534,"owner_gid":65534,"profile_revision":1}')
            for name in ("state","state/packages","state/admissions"):
                member = members[slot + "/" + name]
                self.assertEqual((member.uid,member.gid,member.mode), (65534,65534,0o700))

    def test_malformed_fixtures_are_preseeded_and_links_stay_in_subtree(self):
        from pathlib import PurePosixPath
        with self.archive() as archive:
            members = {m.name:m for m in archive}
            self.assertEqual({m.name.split('/')[0] for m in members.values()}, CASES)
            slot = "/var/lib/vivac/custody-v1/users/65534/"
            self.assertEqual(members["wrong-root-owner"].uid, 65534)
            self.assertEqual(members["writable-ancestor/var"].mode, 0o777)
            self.assertEqual(members["wrong-prefix-mode/var/lib/vivac/custody-v1"].mode, 0o555)
            self.assertEqual(members["wrong-slot-owner" + slot[:-1]].uid, 65534)
            self.assertEqual(members["binding-owner" + slot + "binding.json"].uid, 65534)
            self.assertEqual(members["binding-mode" + slot + "binding.json"].mode, 0o664)
            self.assertTrue(archive.extractfile(members["binding-malformed" + slot + "binding.json"]).read().endswith(b'\n'))
            self.assertEqual(members["binding-oversized" + slot + "binding.json"].size, 513)
            self.assertIn(b'"owner_uid":65533', archive.extractfile(members["binding-identity" + slot + "binding.json"]).read())
            self.assertNotIn("missing-binding" + slot + "binding.json", members)
            self.assertEqual(members["state-mode" + slot + "state"].mode, 0o755)
            self.assertTrue(members["binding-fifo" + slot + "binding.json"].isfifo())
            self.assertTrue(members["binding-hardlink" + slot + "binding.json"].islnk())
            for member in members.values():
                self.assertFalse(PurePosixPath(member.name).is_absolute())
                self.assertNotIn("..", PurePosixPath(member.name).parts)
                if member.issym() or member.islnk():
                    self.assertFalse(PurePosixPath(member.linkname).is_absolute())
                    self.assertNotIn("..", PurePosixPath(member.linkname).parts)

if __name__ == "__main__": unittest.main()
