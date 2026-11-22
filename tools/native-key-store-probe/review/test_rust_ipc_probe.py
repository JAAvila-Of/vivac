"""Pure validation of inherited IPC bootstrap observations."""
import copy
import json
import unittest

import rust_ipc_probe as probe


def final(case):
    root_bad = case in ("wrong-generation", "wrong-store", "peer-exit", "receive-timeout")
    server_bad = case in ("wrong-generation", "wrong-store", "replay", "forwarded-sender",
                          "ancillary-rights", "truncated-frame", "oversized-frame")
    accepted = 2 if case == "roundtrip" else 1 if case == "replay" else 0
    value = dict.fromkeys(probe.SCOPE_FIELDS, False)
    value.update(version=1, case=case, status="observed", host_pid=10, creator_tid=10,
                 server_pid=11, injector_pid=12 if case == "forwarded-sender" else None,
                 handshake_completed=case not in ("wrong-generation", "wrong-store", "receive-timeout"),
                 root_poisoned=root_bad, server_poisoned=server_bad,
                 root_sticky_refusal=root_bad, server_sticky_refusal=server_bad,
                 accepted_probes=accepted, server_accepted_probes=accepted,
                 root_refusals=int(root_bad), server_refusals=int(server_bad),
                 deadline_timed_out=case == "receive-timeout", server_fd_count_before=12,
                 server_fd_count_after=None if case == "peer-exit" else 12,
                 ancillary_fds_closed=case == "ancillary-rights", server_exit_code=0,
                 injector_exit_code=0 if case == "forwarded-sender" else None,
                 owned_exits_observed=True, resources_closed_after_exit=True,
                 pressure_threads=4, pressure_iterations=1)
    return value


def observation():
    rows = []
    for case in probe.CASES:
        value = final(case)
        value.update(root_identity_verified=True, server_identity_verified=True,
                     injector_identity_verified=case == "forwarded-sender", host_exit_code=0,
                     server_exit_observed=True, injector_exit_observed=case == "forwarded-sender",
                     peers_retained_until_observation=case != "peer-exit")
        rows.append(value)
    result = dict.fromkeys(probe.SCOPE_FIELDS, False)
    result.update(version=1, experiment="rust-ipc", status="observed", cases=rows,
                  resources_closed_after_exit=True, resource_limits={},
                  helper_sha256="a" * 64, observer_sha256="b" * 64)
    return result


class IpcValidationTests(unittest.TestCase):
    def test_all_ten_exact_cases_and_phase_equivalence(self):
        self.assertEqual(len(probe.CASES), 10)
        for case in probe.CASES:
            value = final(case)
            identity = {key: value[key] for key in probe.IDENTITY_FIELDS}
            self.assertEqual(probe.parse_result(json.dumps(value).encode(), case, identity), value)
            phase = dict(value, stage="observation", resources_closed_after_exit=False,
                         owned_exits_observed=case == "peer-exit",
                         server_exit_code=0 if case == "peer-exit" else None, injector_exit_code=None,
                         pressure_iterations=0)
            self.assertEqual(probe.parse_result(json.dumps(phase).encode(), case, identity, "observation"), phase)
        probe.validate_observation(observation())

    def test_raw_faults_do_not_fabricate_root_poison_or_probe_acceptance(self):
        for case, changes in (("forwarded-sender", {"server_poisoned": False}),
                              ("ancillary-rights", {"server_fd_count_after": 13}),
                              ("replay", {"server_sticky_refusal": False}),
                              ("truncated-frame", {"root_poisoned": True}),
                              ("oversized-frame", {"accepted_probes": 1}),
                              ("receive-timeout", {"deadline_timed_out": False}),
                              ("peer-exit", {"owned_exits_observed": False})):
            value = dict(final(case), **changes)
            identity = {key: value[key] for key in probe.IDENTITY_FIELDS}
            with self.subTest(case=case), self.assertRaises(ValueError):
                probe.parse_result(json.dumps(value).encode(), case, identity)

    def test_authentication_exit_and_scope_cannot_be_replaced_by_labels(self):
        for field, value in (("root_identity_verified", False), ("server_identity_verified", False),
                             ("injector_identity_verified", False), ("injector_exit_observed", False),
                             ("peers_retained_until_observation", False), ("host_exit_code", -9),
                             ("lock_ack", True), ("mcp_integration", True), ("product_recovery", True),
                             ("accepted_probes", True), ("generation", "public bootstrap remains out of reports"),
                             ("store_hash", "not permitted"), ("secret", "public test value")):
            report = observation()
            row = next(row for row in report["cases"] if row["case"] == "forwarded-sender")
            row[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_missing_duplicate_matrix_and_duplicate_json_fail(self):
        for mutation in ("missing", "duplicate", "scope"):
            report = copy.deepcopy(observation())
            if mutation == "missing":
                report["cases"].pop()
            elif mutation == "duplicate":
                report["cases"][-1] = copy.deepcopy(report["cases"][0])
            else:
                report["server_integration"] = True
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                probe.validate_observation(report)
        value = final("roundtrip")
        payload = json.dumps(value)
        with self.assertRaises(ValueError):
            probe.parse_result((payload[:-1] + ',"lock_ack":false}').encode(), "roundtrip",
                               {key: value[key] for key in probe.IDENTITY_FIELDS})


if __name__ == "__main__":
    unittest.main()
