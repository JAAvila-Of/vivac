"""Guard the fresh reader protocol and evidence classification."""
import copy
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import creation_draft_probe as draft
import creation_reader_probe as probe


class FreshReaderTests(unittest.TestCase):
    def test_launch_checks_are_exact_and_keep_existing_checks(self):
        expected = ("launch-validate", *("launch-" + name for name in probe.MUTATIONS),
                    *("launch-" + name for name in probe.RESTORE_CHECKS))
        self.assertEqual(probe.LAUNCH_CHECKS, expected)
        self.assertTrue(set(expected).issubset(probe.CHECKS))
        self.assertIn("stable", probe.CHECKS)

    def test_launch_packets_require_mode_bound_evidence(self):
        for check in probe.LAUNCH_CHECKS:
            invalid = check != "launch-validate"
            for stage in ("opened", "validated", "final"):
                value = self.packet(stage, True, invalid)
                value.update(dict.fromkeys(probe.MATERIAL_FLAGS, False))
                value["reserved_snapshot_revalidated"] = False
                value["reserved_failure_sticky"] = False
                value["reserved_validation_error"] = "not-required"
                value.update(dict.fromkeys(probe.LAUNCH_FLAGS, False))
                value.update(launch_prepared=True,
                    launch_snapshot_revalidated=stage != "opened" and not invalid,
                    launch_failure_sticky=stage == "final" and invalid,
                    launch_validation_error="not-run" if stage == "opened" else "invalid-data" if invalid else "none",
                    launch_reservation_contention_rejected=stage == "final" and not invalid,
                    launch_reservation_reacquired=stage == "final" and not invalid,
                    **dict.fromkeys(probe.LAUNCH_ROLE_FLAGS, True))
                self.assertEqual(probe.parse_packet(draft.canonical(value), 42, "0" * 25 + "4", stage, check=check), value)
                for field in probe.LAUNCH_FLAGS | probe.LAUNCH_ROLE_FLAGS | probe.LAUNCH_ERRORS:
                    changed = dict(value); del changed[field]
                    with self.subTest(check=check, stage=stage, field=field), self.assertRaises(Exception):
                        probe.parse_packet(draft.canonical(changed), 42, "0" * 25 + "4", stage, check=check)
                with self.assertRaises(Exception):
                    self.parse(value)

    def test_launch_reports_reject_forged_missing_and_cross_mode_evidence(self):
        for check in probe.LAUNCH_CHECKS:
            report = self.report(check=check)
            self.assertEqual(probe.validate_observation(report), report["fresh_reader"])
            for field in probe.LAUNCH_FLAGS | probe.LAUNCH_ROLE_FLAGS | probe.LAUNCH_ERRORS:
                missing = copy.deepcopy(report); del missing["fresh_reader"][field]
                with self.subTest(check=check, missing=field), self.assertRaises(Exception):
                    probe.validate_observation(missing)
                for replacement in (None, 1, "true", not report["fresh_reader"][field]):
                    changed = copy.deepcopy(report)
                    changed["fresh_reader"][field] = replacement
                    with self.subTest(check=check, field=field, replacement=replacement), self.assertRaises(Exception):
                        probe.validate_observation(changed)
            changed = copy.deepcopy(report)
            changed["reader_check"] = changed["fresh_reader"]["check"] = probe.semantic_check(check)
            with self.assertRaises(Exception):
                probe.validate_observation(changed)

    def record(self):
        return draft.canonical(dict(version=2, installation_id="0" * 25 + "1", creation_id="0" * 25 + "2",
            device_id="0" * 25 + "3", credential_id="0" * 25 + "4", public_key="a" * 64,
            ciphertext_sha256="b" * 64, manifest_sha256="c" * 64, ciphertext_len=3, manifest_len=8,
            creation_profile_revision=1, secret_policy_revision=1)).decode("ascii")

    def packet(self, stage="opened", opened=True, invalid=False):
        value = dict(version=1, stage=stage, pid=42, credential_id="0" * 25 + "4", initial_opened=opened,
            initial_error="none" if opened else "not-found", record_canonical=self.record() if opened else None,
            snapshot_revalidated=opened and stage != "opened" and not invalid,
            validation_error="not-run" if stage == "opened" else "unavailable" if not opened else "invalid-data" if invalid else "none",
            repeat_snapshot_revalidated=opened and stage == "final" and not invalid,
            repeat_validation_error="not-run" if stage != "final" else "unavailable" if not opened else "invalid-data" if invalid else "none",
            read_only=True,
            material_snapshot_revalidated=opened and stage != "opened" and not invalid,
            reserved_snapshot_revalidated=opened and stage != "opened" and not invalid,
            material_failure_sticky=opened and stage == "final" and invalid,
            reserved_failure_sticky=opened and stage == "final" and invalid,
            current_namespace_revalidated=opened and stage == "final",
            material_validation_error="not-run" if stage == "opened" else "unavailable" if not opened else "invalid-data" if invalid else "none",
            reserved_validation_error="not-run" if stage == "opened" else "unavailable" if not opened else "invalid-data" if invalid else "none",
            **dict.fromkeys(probe.MATERIAL_FLAGS, opened and stage == "final" and not invalid),
            **dict.fromkeys(probe.LAUNCH_FLAGS, False),
            launch_validation_error="not-required",
            **dict.fromkeys(probe.FLAGS, False))
        if stage == "final":
            value["snapshot_released"] = True
        return value

    def parse(self, value, stage=None, identity=None):
        return probe.parse_packet(draft.canonical(value), 42, "0" * 25 + "4", stage or value["stage"], identity)

    def report(self, *, opened=True, check="stable"):
        selected_check = check
        launch = check in probe.LAUNCH_CHECKS
        check = probe.semantic_check(check)
        invalid = opened and check in (*probe.MUTATIONS, *probe.MOUNT_CHECKS, *probe.RESTORE_CHECKS)
        value = dict(check=check, reader_pid=42, reader_starttime=123,
            reader_identity=dict(executable_observation="verified", credentials=dict(Uid=[65534] * 4,
                Gid=[65534] * 4, NoNewPrivs=[1], PPid=[40])), accepted=opened,
            initial_error="none" if opened else "not-found", record_canonical=self.record() if opened else None,
            revalidated=opened and not invalid, validation_error="invalid-data" if invalid else "none" if opened else "unavailable",
            repeat_snapshot_revalidated=opened and not invalid,
            repeat_validation_error="invalid-data" if invalid else "none" if opened else "unavailable",
            mutation_observed=invalid, reader_state_unchanged=True, filesystem_observation="persistent-state-comparison",
            process_exit_observed=True, helper_unchanged=True,
            source_unchanged=True, snapshot_released=True, reader_sha256="a" * 64, source_sha256="b" * 64,
            state_before_sha256="c" * 64, state_after_sha256="c" * 64, retained_original_verified=opened,
            unrelated_publication_observed=check == "unrelated-publication",
            material_snapshot_revalidated=opened and not invalid,
            reserved_snapshot_revalidated=opened and not invalid,
            material_failure_sticky=invalid, reserved_failure_sticky=invalid,
            current_namespace_revalidated=opened,
            material_validation_error="crosses-devices" if check in probe.MOUNT_CHECKS else "invalid-data" if invalid else "none" if opened else "unavailable",
            reserved_validation_error="crosses-devices" if check in probe.MOUNT_CHECKS else "invalid-data" if invalid else "none" if opened else "unavailable",
            restoration_observed=check in probe.RESTORE_CHECKS,
            restoration_evidence=self.restoration_evidence(check) if check in probe.RESTORE_CHECKS else None,
            **dict.fromkeys(probe.MATERIAL_FLAGS, opened and not invalid),
            **dict.fromkeys(probe.LAUNCH_FLAGS, False),
            launch_validation_error="not-required",
            **dict.fromkeys(probe.FLAGS, False))
        value["check"] = selected_check
        if launch:
            value.update(dict.fromkeys(probe.MATERIAL_FLAGS, False))
            value.update(reserved_snapshot_revalidated=False, reserved_failure_sticky=False,
                         reserved_validation_error="not-required", launch_prepared=opened,
                         launch_snapshot_revalidated=opened and not invalid,
                         launch_failure_sticky=invalid,
                         launch_validation_error="invalid-data" if invalid else "none",
                         launch_reservation_contention_rejected=opened and not invalid,
                         launch_reservation_reacquired=opened and not invalid,
                         **dict.fromkeys(probe.LAUNCH_ROLE_FLAGS, opened))
            if value["restoration_evidence"] is not None:
                value["restoration_evidence"]["check"] = selected_check
        return dict(case="publish-complete" if opened else "publish-package-failure", reader_check=selected_check, helper_pid=41,
                    record_published=opened, record_canonical=self.record(), fresh_reader=value)

    def restoration_evidence(self, check):
        return dict(check=check, original_identity=[10, 20], replacement_identity=[10, 21],
                    restored_identity=[10, 20], retained_identity=[10, 20], original_sha256="a" * 64,
                    replacement_sha256="a" * 64, restored_sha256="a" * 64, retained_sha256="a" * 64)

    def test_exact_packet_sequences_accept_available_unavailable_and_invalidated(self):
        for opened in (False, True):
            first = self.packet(opened=opened)
            self.assertEqual(self.parse(first), first)
            for invalid in ((False, True) if opened else (False,)):
                middle = self.packet("validated", opened, invalid)
                final = self.packet("final", opened, invalid)
                self.assertEqual(self.parse(middle, identity=first), middle)
                self.assertEqual(self.parse(final, identity=first), final)

    def test_admitted_material_evidence_requires_the_final_stable_snapshot(self):
        fields = ("admitted_material_prepared", "admitted_material_validated",
                  "admitted_reservation_validated", "admitted_reservation_contention_rejected",
                  "admitted_reservation_reacquired")
        for opened, invalid in ((True, False), (True, True), (False, False)):
            for stage in ("opened", "validated", "final"):
                expected = opened and not invalid and stage == "final"
                packet = dict(self.packet(stage, opened, invalid), **dict.fromkeys(fields, expected))
                self.assertEqual(self.parse(packet), packet)
                for field in fields:
                    for value in (not expected, 1, 0, None, "true"):
                        with self.subTest(stage=stage, opened=opened, invalid=invalid, field=field, value=value), \
                                self.assertRaises(Exception):
                            self.parse(dict(packet, **{field: value}))
                    missing = dict(packet); del missing[field]
                    with self.subTest(missing=field), self.assertRaises(Exception):
                        self.parse(missing)

    def test_admitted_material_report_rejects_missing_mistyped_or_forged_evidence(self):
        fields = ("admitted_material_prepared", "admitted_material_validated",
                  "admitted_reservation_validated", "admitted_reservation_contention_rejected",
                  "admitted_reservation_reacquired")
        for opened, check in ((True, "stable"), (True, "key-in-place"),
                              (True, "unrelated-publication"), (False, "stable")):
            report = self.report(opened=opened, check=check)
            if check == "unrelated-publication":
                second = self.report()
                record = draft.parse_record(second["record_canonical"])
                record.update(creation_id="0" * 25 + "5", device_id="0" * 25 + "6",
                              credential_id="0" * 25 + "7", public_key="d" * 64)
                second["record_canonical"] = draft.canonical(record).decode("ascii")
                second["fresh_reader"]["record_canonical"] = second["record_canonical"]
                second["helper_pid"] = 51
                second["fresh_reader"]["reader_pid"] = 52
                for field in ("process_exit_observed", "native_binding_verified", "independent_signature_verified",
                              "closure_acknowledged", "package_published", "record_published",
                              "experimental_durable_publication_ack", "namespace_preserved_verified"):
                    second[field] = True
                second.update(publication_outcome="durable", creation_admission=False, custody_accredited=False,
                              producer_origin_checked=False)
                report["unrelated_producer"] = second
            expected = opened and check not in (*probe.MUTATIONS, *probe.MOUNT_CHECKS, *probe.RESTORE_CHECKS)
            report["fresh_reader"].update(dict.fromkeys(fields, expected))
            self.assertEqual(probe.validate_observation(report), report["fresh_reader"])
            for field in fields:
                for value in (not expected, 1, 0, None, "true"):
                    changed = copy.deepcopy(report)
                    changed["fresh_reader"][field] = value
                    with self.subTest(opened=opened, check=check, field=field, value=value), \
                            self.assertRaises(Exception):
                        probe.validate_observation(changed)
                changed = copy.deepcopy(report); del changed["fresh_reader"][field]
                with self.subTest(missing=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)

    def test_rejects_forged_authority_identity_types_and_packet_shape(self):
        for field, value in [("pid", True), ("pid", 43), ("version", True), ("read_only", False),
                             ("initial_opened", 1), ("initial_error", "other"), ("snapshot_revalidated", True),
                             ("validation_error", "none"), ("extra", False), *[(name, True) for name in probe.FLAGS]]:
            changed = dict(self.packet(), **{field: value})
            with self.subTest(field=field), self.assertRaises(Exception):
                self.parse(changed)
        for field in self.packet():
            changed = self.packet(); del changed[field]
            with self.subTest(missing=field), self.assertRaises(Exception):
                self.parse(changed)
        with self.assertRaises(Exception):
            probe.parse_packet(b"x" * 8193, 42, "0" * 25 + "4", "opened")

    def test_material_lifecycle_packet_keeps_independent_failures_sticky(self):
        fields = ("material_snapshot_revalidated", "reserved_snapshot_revalidated",
                  "material_failure_sticky", "reserved_failure_sticky", "current_namespace_revalidated")
        for opened, invalid in ((True, False), (True, True), (False, False)):
            for stage in ("opened", "validated", "final"):
                checked = stage != "opened"
                success = checked and opened and not invalid
                sticky = stage == "final" and opened and invalid
                error = "not-run" if not checked else "unavailable" if not opened else "invalid-data" if invalid else "none"
                value = dict(self.packet(stage, opened, invalid),
                    material_snapshot_revalidated=success, reserved_snapshot_revalidated=success,
                    material_failure_sticky=sticky, reserved_failure_sticky=sticky,
                    current_namespace_revalidated=stage == "final" and opened,
                    material_validation_error=error, reserved_validation_error=error)
                self.assertEqual(self.parse(value), value)
                for field in fields:
                    changed = dict(value, **{field: 1})
                    with self.subTest(stage=stage, field=field), self.assertRaises(Exception):
                        self.parse(changed)
                for field in ("material_validation_error", "reserved_validation_error"):
                    with self.subTest(stage=stage, field=field), self.assertRaises(Exception):
                        self.parse(dict(value, **{field: "none" if error != "none" else "invalid-data"}))

    def test_restored_namespace_does_not_refresh_failed_material(self):
        for check in ("restore-record-replacement", "restore-key-replacement", "restore-package-replacement"):
            report = self.report()
            report["reader_check"] = check
            value = report["fresh_reader"]
            value.update(check=check, revalidated=False, validation_error="invalid-data", mutation_observed=True,
                         repeat_snapshot_revalidated=False, repeat_validation_error="invalid-data",
                         material_snapshot_revalidated=False, reserved_snapshot_revalidated=False,
                         material_failure_sticky=True, reserved_failure_sticky=True,
                         current_namespace_revalidated=True, material_validation_error="invalid-data",
                         reserved_validation_error="invalid-data", restoration_observed=True,
                         restoration_evidence=self.restoration_evidence(check))
            value.update(dict.fromkeys(probe.MATERIAL_FLAGS, False))
            self.assertEqual(probe.validate_observation(report), value)
            for field in ("material_failure_sticky", "reserved_failure_sticky",
                          "current_namespace_revalidated", "restoration_observed"):
                changed = copy.deepcopy(report)
                changed["fresh_reader"][field] = False
                with self.subTest(check=check, field=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)
            for field, replacement in (("original_identity", [10, 21]), ("restored_identity", [10, 22]),
                                       ("retained_identity", [10, True]), ("replacement_identity", [10, 20]),
                                       ("restored_sha256", "b" * 64), ("original_sha256", "invalid"),
                                       ("check", "restore-key-replacement" if check != "restore-key-replacement" else "stable")):
                changed = copy.deepcopy(report)
                changed["fresh_reader"]["restoration_evidence"][field] = replacement
                with self.subTest(check=check, evidence=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)

    def test_restorable_replacements_retain_and_restore_the_original_inode_and_bytes(self):
        credential = "0" * 25 + "4"
        for check in probe.RESTORE_CHECKS:
            with self.subTest(check=check), tempfile.TemporaryDirectory() as root:
                base = Path(root)
                package = base / "packages" / credential
                admission = base / "admissions" / credential
                package.mkdir(parents=True, mode=0o700)
                admission.mkdir(parents=True, mode=0o700)
                for path, raw in ((package / "key.openssh", b"public synthetic ciphertext"),
                                  (package / "manifest.json", b"public synthetic manifest"),
                                  (admission / "record.json", b"public synthetic record")):
                    probe.write_new(path, raw)
                with patch.object(draft, "PACKAGES", package.parent), patch.object(draft, "ADMISSIONS", admission.parent):
                    retained, originals, restore = probe.replace_restorable(check, credential)
                    try:
                        evidence = restore()
                        value = dict(restoration_observed=True, restoration_evidence=evidence,
                                     current_namespace_revalidated=True)
                        probe.validate_restoration(value, check)
                        self.assertTrue(all(os.pread(descriptor, 65537, 0) == raw for descriptor, raw in originals))
                        self.assertEqual(sorted(path.name for path in package.iterdir()), ["key.openssh", "manifest.json"])
                        self.assertEqual(sorted(path.name for path in admission.iterdir()), ["record.json"])
                        self.assertEqual(sorted(path.name for path in package.parent.iterdir()), [credential])
                    finally:
                        for descriptor in retained:
                            os.close(descriptor)

    def test_rejects_refresh_and_false_release(self):
        first = self.packet()
        later = self.packet("validated")
        later["record_canonical"] = later["record_canonical"].replace('"creation_id":"0', '"creation_id":"1', 1)
        with self.assertRaises(Exception):
            self.parse(later, identity=first)
        final = self.packet("final"); final["snapshot_released"] = False
        with self.assertRaises(Exception):
            self.parse(final, identity=first)

    def test_repeat_validation_preserves_the_first_native_error_and_cannot_recover(self):
        first = self.packet()
        middle = self.packet("validated", invalid=True)
        final = self.packet("final", invalid=True)
        for packet in (middle, final):
            for field in ("validation_error", "material_validation_error", "reserved_validation_error"):
                packet[field] = "crosses-devices"
        self.assertEqual(self.parse(middle, identity=first), middle)
        self.assertEqual(self.parse(final, identity=middle), final)
        for field, value in (("validation_error", "invalid-data"), ("repeat_snapshot_revalidated", True),
                             ("material_validation_error", "invalid-data"),
                             ("reserved_validation_error", "invalid-data"),
                             ("repeat_snapshot_revalidated", 0), ("repeat_validation_error", "none"),
                             ("repeat_validation_error", "crosses-devices"), ("repeat_validation_error", "not-run")):
            changed = dict(final, **{field: value})
            with self.subTest(field=field, value=value), self.assertRaises(Exception):
                self.parse(changed, identity=middle)
        changed = dict(middle, repeat_validation_error="invalid-data")
        with self.assertRaises(Exception):
            self.parse(changed, identity=first)

    def test_unopened_reader_cannot_claim_a_repeated_validation_or_record(self):
        first = self.packet(opened=False)
        final = self.packet("final", opened=False)
        self.assertEqual(self.parse(final, identity=first), final)
        for field, value in (("repeat_snapshot_revalidated", True), ("repeat_validation_error", "none"),
                             ("repeat_validation_error", "invalid-data"), ("record_canonical", self.record())):
            with self.subTest(field=field), self.assertRaises(Exception):
                self.parse(dict(final, **{field: value}), identity=first)

    def test_repeat_report_rejects_recovery_after_mutation_and_false_unavailable_success(self):
        for opened, check in ((True, "stable"), (True, "key-in-place"), (False, "stable")):
            report = self.report(opened=opened, check=check)
            self.assertEqual(probe.validate_observation(report), report["fresh_reader"])
            for field, value in (("repeat_snapshot_revalidated", not report["fresh_reader"]["repeat_snapshot_revalidated"]),
                                 ("repeat_snapshot_revalidated", 1), ("repeat_validation_error", "not-run"),
                                 ("repeat_validation_error", "crosses-devices")):
                changed = copy.deepcopy(report)
                changed["fresh_reader"][field] = value
                with self.subTest(opened=opened, check=check, field=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)
            changed = copy.deepcopy(report)
            del changed["fresh_reader"]["repeat_validation_error"]
            with self.subTest(opened=opened, check=check), self.assertRaises(Exception):
                probe.validate_observation(changed)

    def test_only_complete_visible_records_are_available_without_inferred_ack(self):
        for opened in (True, False):
            report = self.report(opened=opened)
            self.assertEqual(probe.validate_observation(report), report["fresh_reader"])
            report["fresh_reader"]["retained_original_verified"] = not opened
            with self.assertRaises(Exception):
                probe.validate_observation(report)
        report = self.report()
        report["case"] = "publish-record-after-rename-failure"
        self.assertTrue(probe.validate_observation(report)["accepted"])
        self.assertFalse(probe.validate_observation(report)["durable_ack_inferred"])

    def test_retained_publication_mutations_require_fresh_data_rejection_without_ack(self):
        for case in ("publish-invalidate-retained-record", "publish-invalidate-retained-package"):
            report = self.report(opened=False)
            report.update(case=case, record_published=True, mutation_observed=True,
                          retained_publication_mutation_preserved=True,
                          publication_outcome="record-durability-unknown", experimental_durable_publication_ack=False)
            report["fresh_reader"]["initial_error"] = "invalid-data"
            with self.subTest(case=case):
                self.assertEqual(probe.validate_observation(report), report["fresh_reader"])
            for field, value in (("initial_error", "not-found"), ("accepted", True),
                                 ("durable_ack_inferred", True), ("record_canonical", self.record()),
                                 ("revalidated", True), ("mutation_observed", True)):
                changed = copy.deepcopy(report)
                changed["fresh_reader"][field] = value
                with self.subTest(case=case, field=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)
            for field, value in (("case", "publish-record-after-rename-failure"), ("record_published", False),
                                 ("mutation_observed", False), ("retained_publication_mutation_preserved", False),
                                 ("experimental_durable_publication_ack", True), ("publication_outcome", "durable")):
                changed = copy.deepcopy(report)
                changed[field] = value
                with self.subTest(case=case, producer_field=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)

    def test_mutations_require_exact_invalidation_without_implicit_refresh(self):
        for check in probe.MUTATIONS:
            report = self.report(check=check)
            self.assertTrue(probe.validate_observation(report)["mutation_observed"])
            for field, replacement in (("validation_error", "other"), ("validation_error", "not-found"),
                                       ("revalidated", True), ("mutation_observed", False), ("retained_original_verified", False)):
                changed = copy.deepcopy(report); changed["fresh_reader"][field] = replacement
                with self.subTest(check=check, field=field), self.assertRaises(Exception):
                    probe.validate_observation(changed)

    def test_live_mount_requires_cross_device_rejection_and_separate_mount_evidence(self):
        for check in ("mount-package", "mount-key", "mount-manifest", "mount-record"):
            report = self.report(check=check)
            report["fresh_reader"].update(revalidated=False, validation_error="crosses-devices", mutation_observed=True,
                                          repeat_snapshot_revalidated=False, repeat_validation_error="invalid-data")
            report["live_mount"] = {"check": check}
            def covered(value):
                self.assertEqual(value["live_mount"]["check"], check)
            with self.subTest(check=check), patch.dict(sys.modules, {
                    "creation_mount_probe": SimpleNamespace(validate_covered=covered)}):
                self.assertEqual(probe.validate_observation(report), report["fresh_reader"])
                for field, replacement in (("validation_error", "invalid-data"), ("validation_error", "none"),
                                           ("revalidated", True), ("mutation_observed", False)):
                    changed = copy.deepcopy(report); changed["fresh_reader"][field] = replacement
                    with self.subTest(field=field), self.assertRaises(Exception):
                        probe.validate_observation(changed)

    def test_mount_observation_rejects_missing_or_unexpected_callbacks_before_launch(self):
        for check, callback in [(name, None) for name in probe.MOUNT_CHECKS] + [("stable", lambda _: None)]:
            with self.subTest(check=check), self.assertRaisesRegex(Exception, "fresh reader mount callback differs"):
                probe.observe(SimpleNamespace(reader_check=check), {"case": "publish-complete"}, None,
                              mount_cover=callback)

    def test_mount_evidence_refusal_cannot_become_a_reader_success(self):
        report = self.report(check="mount-key")
        report["fresh_reader"].update(revalidated=False, validation_error="crosses-devices", mutation_observed=True,
                                          repeat_snapshot_revalidated=False, repeat_validation_error="invalid-data")
        def refuse(_):
            raise ValueError("mount witness missing")
        with patch.dict(sys.modules, {"creation_mount_probe": SimpleNamespace(validate_covered=refuse)}), \
                self.assertRaisesRegex(ValueError, "mount witness missing"):
            probe.validate_observation(report)

    def test_unrelated_publication_preserves_the_original_reader_and_requires_a_real_second_result(self):
        report = self.report(check="unrelated-publication")
        second = self.report()
        record = draft.parse_record(second["record_canonical"])
        record.update(creation_id="0" * 25 + "5", device_id="0" * 25 + "6",
                      credential_id="0" * 25 + "7", public_key="d" * 64)
        second["record_canonical"] = draft.canonical(record).decode("ascii")
        second["fresh_reader"]["record_canonical"] = second["record_canonical"]
        second["helper_pid"] = 51
        second["fresh_reader"]["reader_pid"] = 52
        for field in ("process_exit_observed", "native_binding_verified", "independent_signature_verified",
                      "closure_acknowledged", "package_published", "record_published",
                      "experimental_durable_publication_ack", "namespace_preserved_verified"):
            second[field] = True
        second.update(publication_outcome="durable", creation_admission=False, custody_accredited=False,
                      producer_origin_checked=False)
        report["unrelated_producer"] = second
        self.assertTrue(probe.validate_observation(report)["revalidated"])
        self.assertFalse(report["fresh_reader"]["mutation_observed"])
        for field, value in (("revalidated", False), ("mutation_observed", True),
                             ("unrelated_publication_observed", False)):
            changed = copy.deepcopy(report)
            changed["fresh_reader"][field] = value
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.validate_observation(changed)
        for field in ("experimental_durable_publication_ack", "namespace_preserved_verified"):
            changed = copy.deepcopy(report)
            changed["unrelated_producer"][field] = False
            with self.subTest(second=field), self.assertRaises(Exception):
                probe.validate_observation(changed)

    def test_rejects_missing_evidence_state_writes_false_identity_and_authority(self):
        replacements = [(field, False) for field in ("reader_state_unchanged", "process_exit_observed", "helper_unchanged",
            "source_unchanged", "snapshot_released", "retained_original_verified")]
        replacements += [(field, True) for field in probe.FLAGS]
        replacements += [("state_after_sha256", "d" * 64), ("record_canonical", None), ("reader_pid", True),
                         ("reader_starttime", 0), ("source_sha256", "invalid"), ("filesystem_observation", "syscall-trace")]
        for field, replacement in replacements:
            report = self.report(); report["fresh_reader"][field] = replacement
            with self.subTest(field=field), self.assertRaises(Exception):
                probe.validate_observation(report)
        report = self.report(); report["fresh_reader"]["reader_identity"]["executable_observation"] = "exe_link_denied_cmdline_matches"
        with self.assertRaises(Exception):
            probe.validate_observation(report)
        report = self.report(); report["fresh_reader"]["reader_identity"]["credentials"]["NoNewPrivs"] = [True]
        with self.assertRaises(Exception):
            probe.validate_observation(report)
        for producer in (42, True, 0, -1, None):
            report = self.report(); report["helper_pid"] = producer
            with self.subTest(producer=producer), self.assertRaises(Exception):
                probe.validate_observation(report)


if __name__ == "__main__":
    unittest.main()
