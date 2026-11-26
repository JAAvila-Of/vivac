"""Reject substituted mount selections and receipts before trusting native outcomes."""
import copy
import unittest

import creation_mount_common as common


class MountEvidenceTests(unittest.TestCase):
    def request(self):
        return dict(version=1, operation="mount", check="reader-key",
            target="/var/lib/vivac/custody-v1/users/65534/state/packages/" + "0" * 25 + "1/key.openssh",
            kind="file", token="a" * 64, original=dict(kind="file", mount_id=10,
            device=20, inode=30, mode=0o600, size=42, sha256="b" * 64))

    def ack(self, request):
        after = copy.deepcopy(request["original"])
        after["mount_id"] += 1
        process = dict(Uid="0 0 0 0", Gid="0 0 0 0", NoNewPrivs="1", Seccomp="2",
            Seccomp_filters="1", CapEff="2c0004", CapPrm="2c0004", CapBnd="2c0004", CapInh="0", CapAmb="0")
        target = dict(process, Uid="65534 65534 65534 65534", Gid="65534 65534 65534 65534",
                      CapEff="0", CapPrm="0", CapBnd="0")
        return dict(version=1, operation=request["operation"], check=request["check"], token=request["token"],
            status="observed", before=copy.deepcopy(request["original"]), after=after,
            namespaces=dict(target_euid=65534, target_mount_namespace="mnt:[100]", helper_mount_namespace="mnt:[101]",
                target_root_mount_id=1, helper_root_mount_id=2, target_process=target, helper_process=process,
                mount_propagation=dict(helper=[], target=[])), setns_verified=True, private_mount_verified=True)

    def test_ack_requires_observed_private_mount_for_both_operations(self):
        for operation in ("mount", "unmount"):
            request = self.request()
            acknowledgment = self.ack(request)
            if operation == "unmount":
                request["operation"] = "unmount"
                acknowledgment.update(operation="unmount", before=acknowledgment["after"], after=request["original"])
            for state in (False, None):
                changed = copy.deepcopy(acknowledgment)
                if state is None:
                    del changed["private_mount_verified"]
                else:
                    changed["private_mount_verified"] = state
                with self.subTest(operation=operation, state=state), self.assertRaises(Exception):
                    common.validate_ack(changed, request)

    def test_request_rejects_escape_and_wrong_object_selection(self):
        value = self.request()
        for path in ("/etc/hosts", value["target"].replace("packages/", "packages/../packages/"),
                     value["target"].replace("key.openssh", "manifest.json"), value["target"] + "/"):
            changed = dict(value, target=path)
            with self.subTest(path=path), self.assertRaises(Exception):
                common.validate_request(changed)

    def test_exact_request_and_mount_then_restore_receipts_are_accepted(self):
        request = self.request()
        self.assertEqual(common.validate_request(request), request)
        mounted = self.ack(request)
        self.assertEqual(common.validate_ack(mounted, request), mounted)
        unmount = dict(request, operation="unmount")
        restored = dict(mounted, operation="unmount", before=mounted["after"], after=request["original"])
        self.assertEqual(common.validate_ack(restored, unmount), restored)

    def test_duplicate_json_members_and_extra_authority_fields_are_rejected(self):
        with self.assertRaises(Exception):
            common.decode_json(b'{"version":1,"version":2}')
        changed = dict(self.request(), creation_admission=True)
        with self.assertRaises(Exception):
            common.validate_request(changed)

    def test_ack_rejects_other_request_and_unchanged_mount(self):
        request = self.request()
        for change in (dict(token="c" * 64), dict(check="reader-manifest"), dict(after=request["original"])):
            changed = self.ack(request)
            changed.update(change)
            with self.subTest(change=change), self.assertRaises(Exception):
                common.validate_ack(changed, request)

    def test_ack_rejects_content_substitution_and_unrestored_object(self):
        request = self.request()
        changed = self.ack(request)
        changed["after"]["sha256"] = "d" * 64
        with self.assertRaises(Exception):
            common.validate_ack(changed, request)
        request["operation"] = "unmount"
        changed = self.ack(request)
        with self.assertRaises(Exception):
            common.validate_ack(changed, request)

    def test_ack_rejects_privileged_target_or_wrong_helper_namespace(self):
        request = self.request()
        for selection in ("target-capability", "same-namespace", "unfiltered-helper"):
            changed = self.ack(request)
            ns = changed["namespaces"]
            if selection == "target-capability":
                ns["target_process"]["CapEff"] = "1"
            elif selection == "same-namespace":
                ns["helper_mount_namespace"] = ns["target_mount_namespace"]
            else:
                ns["helper_process"]["Seccomp"] = "0"
            with self.subTest(selection=selection), self.assertRaises(Exception):
                common.validate_ack(changed, request)

    def volume_propagation(self):
        return [dict(type="ext4", mountpoint=path, readonly=readonly, optional=["master:66"])
                for path, readonly in (("/draft-build", True), ("/launcher-build", True), ("/decoy", True),
                                       ("/reports", False), ("/var/lib/vivac", False))]

    def runtime_mounts(self, propagation):
        root = dict(id=1, mountpoint="/", options=["ro"], optional=[], filesystem="overlay")
        return [root, *[dict(id=index + 2, mountpoint=item["mountpoint"],
            options=["ro" if item["readonly"] else "rw"], optional=item["optional"], filesystem=item["type"])
            for index, item in enumerate(propagation)]]

    def test_observed_disposable_volume_profile_is_accepted_by_runtime_guard(self):
        propagation = self.volume_propagation()
        _, observed = common.validate_private_readonly_root(self.runtime_mounts(propagation), "target")
        self.assertEqual(observed, propagation)

    def test_observed_disposable_volume_profile_is_accepted_in_namespace_receipt(self):
        request = self.request()
        acknowledgment = self.ack(request)
        acknowledgment["namespaces"]["mount_propagation"]["target"] = self.volume_propagation()
        self.assertEqual(common.validate_ack(acknowledgment, request), acknowledgment)

    def test_volume_propagation_rejects_access_changes_other_filesystems_paths_and_tags(self):
        changes = (("/reports", dict(readonly=True)), ("/decoy", dict(readonly=False)),
                   ("/draft-build", dict(type="xfs")), ("/reports", dict(mountpoint="/other")),
                   ("/reports", dict(optional=["master:66", "shared:77"])),
                   ("/reports", dict(optional=["propagate_from:66"])),
                   ("/reports", dict(optional=["unknown:66"])))
        for selection, replacement in changes:
            propagation = self.volume_propagation()
            selected = next(item for item in propagation if item["mountpoint"] == selection)
            selected.update(replacement)
            with self.subTest(selection=selection, replacement=replacement):
                with self.assertRaises(Exception):
                    common.validate_private_readonly_root(self.runtime_mounts(propagation), "target")
                request = self.request()
                acknowledgment = self.ack(request)
                acknowledgment["namespaces"]["mount_propagation"]["target"] = propagation
                with self.assertRaises(Exception):
                    common.validate_ack(acknowledgment, request)


if __name__ == "__main__":
    unittest.main()
