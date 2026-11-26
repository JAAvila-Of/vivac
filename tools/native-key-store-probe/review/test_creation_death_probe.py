"""Guard process-death evidence and publication visibility boundaries."""
import copy
import hashlib
import unittest
from unittest.mock import patch

import creation_death_probe as death
import creation_draft_probe as draft
import creation_native_probe as native


class DeathEvidenceTests(unittest.TestCase):
    def cut(self, case="death-record-renamed"):
        record = dict(version=2, installation_id="0" * 25 + "1", creation_id="0" * 25 + "2",
                      device_id="0" * 25 + "3", credential_id="0" * 25 + "4",
                      public_key="a" * 64, ciphertext_sha256=hashlib.sha256(b"key").hexdigest(),
                      manifest_sha256=hashlib.sha256(b"manifest").hexdigest(), ciphertext_len=3,
                      manifest_len=8, creation_profile_revision=1, secret_policy_revision=1)
        text = draft.canonical(record).decode("ascii")
        challenge = native.binding.DOMAIN + hashlib.sha256(draft.canonical(record)).digest() + b"n" * 32
        return dict(version=1, stage="publication-cut", case=case, helper_pid=42, agent_pid=43,
            loader_pid=44, creator_tid=42, record_canonical=text, native_binding_verified=True,
            challenge=challenge.hex(), signature="a" * 128, closure_acknowledged=True,
            resources_closed_after_drain=True, agent_exit_code=None, agent_exit_signal=9,
            loader_exit_code=0, loader_exit_signal=None, cut_point=case.removeprefix("death-"),
            durable_reply_sent=False, **dict.fromkeys(native.FLAGS, False), **dict.fromkeys(native.COUNTS, 1))

    def test_cut_cannot_claim_an_ack_or_normal_final(self):
        for field, value in (("durable_reply_sent", True), ("stage", "final"),
                             ("preparation_dropped", True), ("experimental_durable_publication_ack", True)):
            with self.subTest(field=field), self.assertRaises(Exception):
                death.parse_cut(draft.canonical(dict(self.cut(), **{field: value})), "death-record-renamed", 42)

    def test_every_cut_requires_its_original_native_closure(self):
        for case in death.DEATH_CASES:
            cut = self.cut(case)
            with self.subTest(case=case):
                self.assertEqual(death.parse_cut(draft.canonical(cut), case, 42), cut)
        for field, value in (("cut_point", "before-durable-reply"), ("agent_exit_signal", 15),
                             ("loader_exit_code", 1), ("workers_joined", 0), ("creator_tid", 43),
                             ("native_binding_verified", False), ("closure_acknowledged", False)):
            with self.subTest(field=field), self.assertRaises(Exception):
                death.parse_cut(draft.canonical(dict(self.cut(), **{field: value})), "death-record-renamed", 42)
        missing = self.cut()
        del missing["signature"]
        with self.assertRaises(Exception):
            death.parse_cut(draft.canonical(missing), "death-record-renamed", 42)
        original = self.cut()
        with self.assertRaises(Exception):
            death.parse_cut(draft.canonical(dict(original, loader_pid=45)), "death-record-renamed", 42, original)

    def test_visibility_changes_at_rename_without_a_durable_reply(self):
        expected = (
            (False, False, False, "interrupted-unpublished"),
            (True, False, False, "interrupted-package-only"),
            (True, False, False, "interrupted-package-only"),
            (True, False, True, "interrupted-package-only"),
            (True, False, True, "interrupted-package-only"),
            (True, False, True, "interrupted-package-only"),
            (True, True, False, "interrupted-record-visible"),
            (True, True, False, "interrupted-record-visible"),
            (True, True, False, "interrupted-record-visible"),
            (True, True, False, "interrupted-record-visible"),
        )
        self.assertEqual(len(death.DEATH_CASES), len(expected))
        for case, row in zip(death.DEATH_CASES, expected):
            with self.subTest(case=case):
                state = death.expected_state(case)
                self.assertEqual(tuple(state[field] for field in ("package_published", "record_published",
                    "pending_creation_record_observed", "publication_outcome")), row)
                self.assertEqual(state["joint_validation_completed"], case == "death-before-durable-reply")
        self.assertTrue(death.expected_state("death-package-parent-synced")["package_parent_sync_observed"])
        self.assertFalse(death.expected_state("death-package-parent-synced")["package_durable_observed"])
        self.assertTrue(death.expected_state("death-record-parent-synced")["record_parent_sync_observed"])

    def test_only_owned_sigkill_with_exit_readiness_and_no_final_is_a_death_receipt(self):
        death.validate_death_receipt(-9, True, True, False)
        for code in (0, 1, 77, -15, True, None):
            with self.subTest(code=code), self.assertRaises(Exception):
                death.validate_death_receipt(code, True, True, False)
        for evidence in ((False, True, False), (True, False, False), (True, True, True), (True, True, None)):
            with self.subTest(evidence=evidence), self.assertRaises(Exception):
                death.validate_death_receipt(-9, *evidence)

    def observation(self):
        case = "death-record-renamed"
        cut = self.cut(case)
        record = draft.parse_record(cut["record_canonical"])
        result = dict(version=1, experiment="rooted-native-creation", case=case, status="observed",
            helper_pid=42, record_canonical=cut["record_canonical"], publication_cut=cut, cut_point=cut["cut_point"],
            selected_installation_id=record["installation_id"], device_id=record["device_id"],
            credential_id=record["credential_id"], producer_starttime=123,
            producer_identity=dict(credentials=dict(Uid=[65534] * 4, Gid=[65534] * 4, NoNewPrivs=[1], PPid=[41]),
                                   executable_observation="verified"),
            producer_exit_code=None, producer_exit_signal=9, preserved_namespace=dict(packages=[], admissions=[]),
            creation_record_verified=True, final_slot_absent=False, admissions_empty=False,
            **death.expected_state(case), **dict.fromkeys(native.FLAGS, False))
        for field in ("process_exit_observed", "producer_exit_observed", "terminal_restored", "standard_streams_clean",
            "standard_stream_eof_observed",
            "helper_unchanged", "sources_unchanged", "root_binding_unchanged", "process_identity_verified",
            "private_terminal_observed", "secret_boundaries_checked", "original_package_verified", "independent_exits_observed",
            "private_loader_terminal_observed", "native_identity_observed", "provider_revalidated", "retained_connection_refused",
            "new_connection_refused", "native_binding_verified", "independent_signature_verified", "altered_message_refused",
            "closure_acknowledged", "independent_closed_before_publication", "publication_filesystem_verified", "cut_witness_verified",
            "producer_killed_by_pidfd", "producer_kill_identity_verified", "namespace_preserved_verified"):
            result[field] = True
        for field in ("preparation_release_acknowledged", "normal_final_received", "durable_reply_sent",
            "experimental_durable_publication_ack", "physical_power_loss_simulated", "mutation_observed", "invalidated_stage_preserved",
            "abandoned_stage_retained_without_closure", "native_agent_alive_before_abandonment", "abandoned_stage_retained_while_producer_alive"):
            result[field] = False
        for field in ("helper_sha256", "observer_sha256", "real_binding_sha256", "decoy_binding_sha256", "openssl_sha256",
            "death_source_sha256", "controller_source_sha256", "state_at_cut_sha256", "state_after_death_sha256"):
            result[field] = "b" * 64
        metadata = dict(cmdline_secret_absence_verified=True, environment_observation="permission-denied",
                        environment_secret_absence_verified=False)
        identity = dict(credentials=dict(Uid=[65534] * 4, Gid=[65534] * 4, NoNewPrivs=[1], PPid=[42]),
                        executable_observation="verified")
        result.update(native_process_metadata=dict(agent=metadata, loader=metadata), native_environment_absence_verified=False,
                      native_process_identity=dict(agent=identity, loader=identity))
        return result

    def test_observation_requires_a_cut_witness_and_rejects_historical_ack_claims(self):
        original = self.observation()
        self.assertIs(native.validate_observation(original, original["case"], reader_required=False), original)
        for field, value in (("cut_witness_verified", False), ("producer_killed_by_pidfd", False),
            ("experimental_durable_publication_ack", True), ("durable_reply_sent", True),
            ("preparation_release_acknowledged", True), ("preparation_dropped", False), ("final", {}),
            ("producer_exit_signal", 15), ("state_after_death_sha256", "c" * 64), ("publication_outcome", "durable")):
            changed = copy.deepcopy(original)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(Exception):
                native.validate_observation(changed, changed["case"], reader_required=False)
        changed = copy.deepcopy(original)
        del changed["publication_cut"]
        with self.assertRaises(Exception):
            native.validate_observation(changed, changed["case"], reader_required=False)

    def test_cut_parser_does_not_require_a_host_sigkill_constant(self):
        import signal
        with patch.dict(signal.__dict__):
            signal.__dict__.pop("SIGKILL", None)
            self.assertEqual(death.LINUX_SIGKILL, 9)
            self.assertEqual(death.parse_cut(draft.canonical(self.cut()), "death-record-renamed", 42), self.cut())

    def test_observation_requires_observed_standard_stream_eof(self):
        original = self.observation()
        original["standard_stream_eof_observed"] = True
        self.assertIs(native.validate_observation(original, original["case"], reader_required=False), original)
        for state in (False, None):
            changed = copy.deepcopy(original)
            if state is None:
                del changed["standard_stream_eof_observed"]
            else:
                changed["standard_stream_eof_observed"] = state
            with self.subTest(state=state), self.assertRaises(Exception):
                native.validate_observation(changed, changed["case"], reader_required=False)


if __name__ == "__main__":
    unittest.main()
