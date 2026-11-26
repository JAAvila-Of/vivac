"""Reject fabricated creation-binding evidence without executing native helpers."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import rust_creation_binding_probe as probe
import run_lifecycle_probe as runner


PUBLIC_KEY = bytes.fromhex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
IDENTITY = {"agent_pid": 111, "loader_pid": 222, "creator_tid": 333}
COMMENT = "vivac.device-key.v1:" + "0" * 25 + "1:" + "0" * 25 + "2"


def fixture():
    return {"manifest": {"device_id": "0" * 25 + "1", "credential_id": "0" * 25 + "2",
                         "public_key": PUBLIC_KEY.hex()},
            "key_bytes": b"synthetic encrypted fixture", "manifest_bytes": b"synthetic manifest fixture"}


def record(case="complete"):
    return probe.make_record(fixture(), {"public_key": PUBLIC_KEY}, COMMENT, case)


def result(case="complete", final=False):
    digest = hashlib.sha256(probe.canonical(record(case))).digest()
    value = {"stage": "creation-result", "case": case, **IDENTITY, "record_sha256": digest.hex(),
             "identity_mismatch": case != "complete", "native_binding_verified": case == "complete",
             "challenge": (probe.DOMAIN + digest + b"n" * 32).hex() if case == "complete" else None,
             "signature": (b"s" * 64).hex() if case == "complete" else None,
             "ready_published": False, **dict.fromkeys(probe.FLAGS, False)}
    if final:
        value.update(stage="creation-final", version=1, status="observed", state="CLOSED", active=False,
                     lock_callers=1, lock_acknowledgments=1, close_acknowledged=True,
                     resources_closed_after_drain=True, busy_refused=0, closed_refused=0,
                     agent_exit_code=None, agent_exit_signal=9, loader_exit_code=0, loader_exit_signal=None,
                     pressure_iterations=4)
        for key in ("native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published", "workers_joined"):
            value[key] = int(case == "complete")
    return value


def startup(stage="prompt"):
    value = dict.fromkeys(probe.loader.FIELDS, False)
    value.update(stage=stage, **IDENTITY, secret_writes=int(stage == "handoff"),
                 echo_disabled=True, prompt_observed=True, loader_stopped=stage == "handoff",
                 loader_exit_code=None, loader_exit_signal=None, agent_exit_code=None, agent_exit_signal=None,
                 **dict.fromkeys(probe.FLAGS, False))
    return value


class CreationEvidenceTests(unittest.TestCase):
    def test_controller_accepts_complete_native_observations(self):
        runner.validate_rust_creation_binding_observation(self.controller_report())

    def controller_report(self):
        cases = []
        for case in probe.CASES:
            value = result(case, final=True)
            value.update(independent_signature_verified=case == "complete", altered_message_refused=case == "complete")
            value.update(dict.fromkeys(("close_acknowledged", "resources_closed_after_drain", "independent_exits_observed",
                         "original_identity_observed", "retained_connection_refused", "new_connection_refused",
                         "pty_master_retained_through_exit", "pty_slave_private", "sealed_snapshot", "lease_revalidated"), True))
            cases.append(value)
        return dict(version=1, experiment="rust-creation-binding", status="observed", cases=cases,
                    sentinel_survived=True, resources_closed_after_exit=True, **dict.fromkeys(probe.FLAGS, False))

    def test_controller_rejects_claimed_admission_and_missing_independent_evidence(self):
        for field in (*probe.FLAGS, "ready_published", "independent_exits_observed", "independent_signature_verified",
                      "original_identity_observed", "resources_closed_after_drain", "lease_revalidated"):
            value = self.controller_report()
            value["cases"][0][field] = not value["cases"][0][field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.validate_rust_creation_binding_observation(value)

    def test_controller_rejects_duplicate_cases_and_boolean_counters(self):
        value = self.controller_report()
        value["cases"][2] = copy.deepcopy(value["cases"][1])
        with self.assertRaises(ValueError):
            runner.validate_rust_creation_binding_observation(value)
        value = self.controller_report()
        value["cases"][0]["proofs_verified"] = True
        with self.assertRaises(ValueError):
            runner.validate_rust_creation_binding_observation(value)

    def parse(self, value, case="complete", final=False):
        return probe.parse_result(probe.canonical(value), case, IDENTITY, record(case), final=final)

    def test_exact_case_matrix_accepts_result_and_closed_receipts(self):
        for case in probe.CASES:
            for final in (False, True):
                with self.subTest(case=case, final=final):
                    value = result(case, final)
                    self.assertEqual(self.parse(value, case, final), value)

    def test_unknown_missing_extra_and_duplicate_fields_are_refused(self):
        value = result()
        variants = [{**value, "unknown": False}, {key: item for key, item in value.items() if key != "signature"}]
        for altered in variants:
            with self.assertRaises(probe.loader.ProbeError):
                self.parse(altered)
        duplicate = probe.canonical(value)[:-1] + b',"case":"wrong-key"}'
        with self.assertRaises(Exception):
            probe.parse_result(duplicate, "complete", IDENTITY, record())
        with self.assertRaises(probe.loader.ProbeError):
            probe.parse_result(probe.canonical(value), "unknown", IDENTITY, record())

    def test_case_identity_digest_and_native_identity_cannot_be_relabelled(self):
        for key, replacement in (("case", "wrong-key"), ("stage", "creation-final"),
                                 ("record_sha256", "00" * 32), ("record_sha256", "A0" * 32),
                                 ("agent_pid", 112), ("loader_pid", True), ("creator_tid", 0)):
            with self.subTest(key=key), self.assertRaises(probe.loader.ProbeError):
                self.parse({**result(), key: replacement})

    def test_flags_never_claim_producer_admission_or_custody(self):
        for key in probe.FLAGS | {"ready_published"}:
            for replacement in (True, 0, None):
                with self.subTest(key=key, replacement=replacement), self.assertRaises(probe.loader.ProbeError):
                    self.parse({**result(), key: replacement})

    def test_creation_challenge_refuses_other_purposes_records_and_encodings(self):
        value = result()
        message = bytes.fromhex(value["challenge"])
        alterations = [b"vivac.device.enrollment-proof.v1\0" + message[len(probe.DOMAIN):],
                       bytes([message[0] ^ 1]) + message[1:],
                       probe.DOMAIN + b"x" * 32 + message[-32:], message[:-1], message + b"x"]
        for altered in alterations:
            with self.subTest(message=altered), self.assertRaises(probe.loader.ProbeError):
                self.parse({**value, "challenge": altered.hex()})
        for key, replacement in (("challenge", value["challenge"].upper()), ("signature", "aa" * 63),
                                 ("signature", "gg" * 64), ("signature", None)):
            with self.assertRaises(probe.loader.ProbeError):
                self.parse({**value, key: replacement})

    def test_negative_cases_refuse_any_proof_or_positive_identity_claim(self):
        for case in ("wrong-key", "wrong-comment"):
            for key, replacement in (("challenge", result()["challenge"]), ("signature", "aa" * 64),
                                     ("identity_mismatch", False), ("identity_mismatch", 1),
                                     ("native_binding_verified", True), ("native_binding_verified", 0)):
                with self.subTest(case=case, key=key), self.assertRaises(probe.loader.ProbeError):
                    self.parse({**result(case), key: replacement}, case)

    def test_closed_receipts_require_exact_counts_and_separate_exit_causes(self):
        counters = ("native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published",
                    "workers_joined", "lock_callers", "lock_acknowledgments", "busy_refused", "closed_refused",
                    "agent_exit_signal", "loader_exit_code", "pressure_iterations", "version")
        for case in probe.CASES:
            value = result(case, True)
            for key in counters:
                for replacement in (True, -1, value[key] + 1 if key != "pressure_iterations" else 0):
                    with self.subTest(case=case, key=key), self.assertRaises(probe.loader.ProbeError):
                        self.parse({**value, key: replacement}, case, True)
            for key, replacement in (("active", True), ("close_acknowledged", False),
                                     ("resources_closed_after_drain", 1), ("state", "CLOSING"),
                                     ("agent_exit_code", 0), ("loader_exit_signal", 9), ("status", "incomplete")):
                with self.subTest(case=case, key=key), self.assertRaises(probe.loader.ProbeError):
                    self.parse({**value, key: replacement}, case, True)

    def test_startup_cannot_publish_readiness_or_change_retained_processes(self):
        for stage in ("prompt", "handoff"):
            value = startup(stage)
            self.assertEqual(probe.parse_startup(probe.canonical(value), stage, IDENTITY)["stage"], stage)
            for key, replacement in (("creation_admission", True), ("ready_published", True),
                                     ("identity_bound", True), ("loader_exit_code", 0), ("agent_pid", 112),
                                     ("agent_pid", True), ("creator_tid", 222)):
                with self.subTest(stage=stage, key=key), self.assertRaises(probe.loader.ProbeError):
                    probe.parse_startup(probe.canonical({**value, key: replacement}), stage, IDENTITY)

    def test_record_changes_preserve_package_bytes_hashes_lengths_and_field_order(self):
        original_fixture = fixture()
        before = copy.deepcopy(original_fixture)
        original = record()
        self.assertEqual(list(original), ["version", "installation_id", "creation_id", "device_id", "credential_id",
            "public_key", "ciphertext_sha256", "manifest_sha256", "ciphertext_len", "manifest_len",
            "creation_profile_revision", "secret_policy_revision"])
        self.assertEqual(original["ciphertext_sha256"], hashlib.sha256(before["key_bytes"]).hexdigest())
        self.assertEqual(original["manifest_sha256"], hashlib.sha256(before["manifest_bytes"]).hexdigest())
        for case in probe.CASES:
            value = probe.make_record(original_fixture, {"public_key": PUBLIC_KEY}, COMMENT, case)
            probe.check_record_change(value, original, case)
            self.assertEqual(original_fixture, before)
            self.assertEqual(value["ciphertext_len"], len(before["key_bytes"]))
            self.assertEqual(value["manifest_len"], len(before["manifest_bytes"]))
            for key in ("ciphertext_sha256", "manifest_sha256", "credential_id"):
                with self.assertRaises(probe.loader.ProbeError):
                    probe.check_record_change({**value, key: "changed"}, original, case)
        with self.assertRaises(probe.loader.ProbeError):
            probe.make_record(original_fixture, {"public_key": PUBLIC_KEY}, COMMENT + "x", "complete")

    def test_complete_coverage_refuses_missing_duplicate_unknown_or_incomplete_cases(self):
        values = [result(case, True) for case in probe.CASES]
        probe.require_complete_cases(values)
        for altered in (values[:-1], values + values[:1], [values[0]] * 3,
                        [values[0], values[1], {**values[2], "case": "other"}],
                        [values[0], values[1], {**values[2], "status": "incomplete"}]):
            with self.assertRaises(probe.loader.ProbeError):
                probe.require_complete_cases(altered)

    def test_openssl_verification_writes_only_public_inputs_and_uses_private_environment(self):
        verifier = mock.Mock(original=Path("/usr/bin/openssl"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with mock.patch.object(probe.subprocess, "Popen", side_effect=RuntimeError("execution prevented")) as launch:
                with self.assertRaisesRegex(RuntimeError, "execution prevented"):
                    probe.openssl_verify(verifier, root, {"HOME": directory}, PUBLIC_KEY, b"creation message", b"s" * 64, "proof")
            self.assertEqual((root / "proof.der").read_bytes(), probe.DER_PREFIX + PUBLIC_KEY)
            self.assertEqual((root / "proof.message").read_bytes(), b"creation message")
            self.assertEqual((root / "proof.signature").read_bytes(), b"s" * 64)
            self.assertNotEqual((root / "proof.altered").read_bytes(), b"creation message")
            arguments, options = launch.call_args
            self.assertEqual(arguments[0][0:3], ["/usr/bin/openssl", "pkeyutl", "-verify"])
            self.assertEqual(options["env"], {"HOME": directory})
            self.assertNotIn("shell", options)
            verifier.revalidate.assert_called_once()

    def test_openssl_always_accepting_or_always_refusing_verifiers_cannot_supply_evidence(self):
        for outcomes, accepted in (((0, 1), True), ((0, 0), False), ((1,), False), ((0, 2), False)):
            with self.subTest(outcomes=outcomes), tempfile.TemporaryDirectory() as directory:
                verifier = mock.Mock(original=Path("/usr/bin/openssl"))
                processes = []
                for outcome in outcomes:
                    process = mock.Mock()
                    process.stdout.fileno.return_value = 10
                    process.stderr.fileno.return_value = 11
                    process.poll.return_value = outcome
                    process.wait.return_value = outcome
                    processes.append(process)
                selector = mock.MagicMock()
                selector.__enter__.return_value.get_map.return_value = {}
                with mock.patch.object(probe.subprocess, "Popen", side_effect=processes) as launch, \
                     mock.patch.object(probe.selectors, "DefaultSelector", return_value=selector), \
                     mock.patch.object(probe.os, "set_blocking"):
                    if accepted:
                        probe.openssl_verify(verifier, Path(directory), {}, PUBLIC_KEY, b"message", b"s" * 64, "verify")
                        self.assertEqual(launch.call_count, 2)
                        self.assertNotEqual(launch.call_args_list[0].args[0], launch.call_args_list[1].args[0])
                    else:
                        with self.assertRaises(probe.loader.ProbeError):
                            probe.openssl_verify(verifier, Path(directory), {}, PUBLIC_KEY, b"message", b"s" * 64, "verify")


if __name__ == "__main__":
    unittest.main()
