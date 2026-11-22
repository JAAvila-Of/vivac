"""Pure metadata checks for the disposable Rust loader observer."""
import json
import unittest

import rust_loader_probe as probe

def frame(stage):
    value = {field: False for field in probe.FIELDS}
    value.update(stage=stage, agent_pid=1, loader_pid=2, creator_tid=3, secret_writes=0,
                 agent_exit_code=None, agent_exit_signal=None, loader_exit_code=None, loader_exit_signal=None,
                 prompt_observed=True, echo_disabled=True)
    if stage == "handoff":
        value.update(loader_stopped=True, secret_writes=1)
    if stage == "before-ready":
        value.update(signature="a" * 128, secret_writes=1, identity_bound=True, proof_verified=True, loader_exit_code=0)
    if stage == "ready":
        value.update(ready_published=True, secret_writes=1, identity_bound=True, proof_verified=True, loader_exit_code=0)
    if stage == "closed":
        value.update(close_ack=True, resource_release_after_exit=True, loader_exit_signal=9, agent_exit_signal=9)
    return value

class StageTests(unittest.TestCase):
    def test_expected_stage_metadata_is_bounded_and_contains_no_secret(self):
        for stage in ("prompt", "handoff", "before-ready", "ready", "closed"):
            value = frame(stage)
            self.assertEqual(probe.parse_stage(json.dumps(value).encode(), stage), value)

    def test_pre_ready_stages_cannot_publish_readiness(self):
        for stage in ("prompt", "handoff", "before-ready"):
            with self.subTest(stage=stage), self.assertRaises(probe.ProbeError):
                probe.parse_stage(json.dumps(dict(frame(stage), ready_published=True)).encode())

    def test_handoff_requires_actual_stop_and_one_complete_delivery(self):
        for changes in ({"loader_stopped": False}, {"secret_writes": True}, {"secret_writes": 2}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_stage(json.dumps(dict(frame("handoff"), **changes)).encode())

    def test_loaded_stage_requires_signature_identity_and_successful_exit(self):
        for changes in ({"signature": "A" * 128}, {"proof_verified": False}, {"identity_bound": False},
                        {"loader_exit_code": False}, {"loader_exit_signal": 9}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_stage(json.dumps(dict(frame("before-ready"), **changes)).encode())

    def test_no_extra_payload_or_signature_is_accepted_on_closed_stage(self):
        for changes in ({"signature": "a" * 128}, {"unexpected_payload": "public fixture"}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_stage(json.dumps(dict(frame("closed"), **changes)).encode())

    def test_final_result_cannot_claim_product_admission_or_skip_pressure(self):
        value = dict(frame("closed"), version=1, case="cancel-prompt", status="observed", pressure_iterations=1,
                     custody_accredited=False, launcher_accredited=False, full_session_accredited=False)
        self.assertEqual(probe.parse_final(json.dumps(value).encode(), "cancel-prompt"), value)
        for changes in ({"custody_accredited": True}, {"launcher_accredited": True}, {"pressure_iterations": 0},
                        {"version": True}, {"signature": "a" * 128}):
            with self.subTest(changes=changes), self.assertRaises(probe.ProbeError):
                probe.parse_final(json.dumps(dict(value, **changes)).encode(), "cancel-prompt")

if __name__ == "__main__":
    unittest.main()
