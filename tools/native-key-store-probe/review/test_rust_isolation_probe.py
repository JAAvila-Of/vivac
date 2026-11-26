"""Pure control validation for process-local admission quarantine."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import rust_isolation_probe as probe


IDENTITY = {"host_pid": 10, "creator_tid": 10, "worker_a_pid": 11, "worker_b_pid": 12, "native_pid": 13}


def final():
    return dict(IDENTITY, version=1, case="process-boundary", status="observed",
                host_admission_uncertain=False, host_empty_owner_admitted=True, host_health_responses=2,
                worker_a_admission_poisoned=True, worker_a_poison_refusals=3, worker_a_drop_elapsed_ns=12345,
                worker_b_reserved=4, worker_b_fifth_errno=16, worker_b_readmitted=True,
                worker_a_exit_code=0, worker_b_exit_code=0, worker_exits_observed=True,
                pressure_threads=4, pressure_iterations=1, resources_closed_after_worker_exit=True,
                custody_accredited=False, product_recovery=False, same_store_recovery=False,
                mcp_integration=False, server_integration=False, close_ack=False)


def observation():
    case = final()
    case.update(worker_a_marker_identity_verified=True, native_alive_while_worker_b_completed=True,
                worker_a_alive_while_worker_b_completed=True, native_exit_signal=9, native_exit_observed=True,
                host_exit_code=0, normal_worker_exit_commanded=True, host_responsive_after_native_exit=True,
                resources_closed_after_observed_exit=True)
    return {"version": 1, "experiment": "rust-isolation", "status": "observed", "cases": [case],
            "custody_accredited": False, "product_recovery": False, "same_store_recovery": False,
            "mcp_integration": False, "server_integration": False, "close_ack": False,
            "resources_closed_after_observed_exit": True, "resource_limits": {},
            "helper_sha256": "a" * 64, "observer_sha256": "b" * 64}


class IsolationValidationTests(unittest.TestCase):
    def proc_fixture(self, root):
        helper = root / "helper"
        helper.write_bytes(b"public executable identity fixture")
        metadata = helper.stat()
        expected = (helper.resolve(strict=True), metadata.st_dev, metadata.st_ino)
        directory = root / "proc" / "7"
        directory.mkdir(parents=True)
        os.link(helper, directory / "exe")
        status = "PPid:\t6\nUid:\t65534 65534 65534 65534\nGid:\t65534 65534 65534 65534\nNoNewPrivs:\t1\nCapEff:\t0000000000000000\n"
        (directory / "status").write_text(status, encoding="ascii")
        return expected, directory, status

    def verify_proc_fixture(self, expected, directory, *, starts=(123, 123), readiness=(False, False), resolved=None):
        original_resolve = Path.resolve

        def resolve(path, strict=False):
            if path == directory / "exe":
                return expected[0] if resolved is None else resolved
            return original_resolve(path, strict=strict)

        with patch.object(probe.abandon, "pidfd_ready", side_effect=readiness), \
                patch.object(probe.life, "process_start", side_effect=starts), \
                patch.object(probe.Path, "resolve", autospec=True, side_effect=resolve):
            return probe.verify_process(7, 6, expected, 50, 123, proc_root=directory.parent, uid=65534, gid=65534)

    def test_live_process_is_bound_to_parent_credentials_executable_and_starttime(self):
        with tempfile.TemporaryDirectory() as temporary:
            expected, directory, _ = self.proc_fixture(Path(temporary))
            self.assertEqual(self.verify_proc_fixture(expected, directory), 123)

    def test_proc_identity_rejects_wrong_parent_privileges_and_executable(self):
        for original, replacement in (("PPid:\t6", "PPid:\t8"), ("Uid:\t65534", "Uid:\t0"),
                                       ("Gid:\t65534", "Gid:\t0"), ("NoNewPrivs:\t1", "NoNewPrivs:\t0"),
                                       ("CapEff:\t0000000000000000", "CapEff:\t0000000000000001")):
            with self.subTest(field=original), tempfile.TemporaryDirectory() as temporary:
                expected, directory, status = self.proc_fixture(Path(temporary))
                (directory / "status").write_text(status.replace(original, replacement), encoding="ascii")
                with self.assertRaises(ValueError):
                    self.verify_proc_fixture(expected, directory)
        with tempfile.TemporaryDirectory() as temporary:
            expected, directory, _ = self.proc_fixture(Path(temporary))
            with self.assertRaises(ValueError):
                self.verify_proc_fixture(expected, directory, resolved=Path(temporary) / "different")
            (directory / "exe").unlink()
            (directory / "exe").write_bytes(b"public executable identity fixture")
            with self.assertRaises(ValueError):
                self.verify_proc_fixture(expected, directory)

    def test_process_exit_or_changed_starttime_cannot_pass_identity_check(self):
        for changes in ({"starts": (124, 124)}, {"starts": (123, 124)}, {"readiness": (True,)},
                        {"readiness": (False, True)}):
            with self.subTest(changes=changes), tempfile.TemporaryDirectory() as temporary:
                expected, directory, _ = self.proc_fixture(Path(temporary))
                with self.assertRaises(ValueError):
                    self.verify_proc_fixture(expected, directory, **changes)

    def test_exact_process_boundary_evidence_and_control_stdout_equivalence(self):
        value = final()
        self.assertEqual(probe.parse_final(json.dumps(value).encode(), IDENTITY), value)
        self.assertEqual(probe.parse_final(json.dumps(value, sort_keys=True).encode(), IDENTITY), value)
        probe.validate_observation(observation())

    def test_parent_health_does_not_replace_worker_quarantine_or_live_child_evidence(self):
        for field, value in (("host_admission_uncertain", True), ("host_empty_owner_admitted", False),
                             ("worker_a_admission_poisoned", False), ("worker_a_poison_refusals", 0),
                             ("worker_b_reserved", 3), ("worker_b_fifth_errno", 0),
                             ("worker_b_readmitted", False), ("worker_a_marker_identity_verified", False),
                             ("native_alive_while_worker_b_completed", False),
                             ("worker_a_alive_while_worker_b_completed", False),
                             ("normal_worker_exit_commanded", False), ("host_exit_code", -9),
                             ("native_exit_signal", 15), ("host_responsive_after_native_exit", False)):
            report = observation()
            report["cases"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_recovery_acknowledgment_extra_content_and_wrong_types_are_rejected(self):
        for field, value in (("custody_accredited", True), ("product_recovery", True),
                             ("same_store_recovery", True), ("mcp_integration", True),
                             ("server_integration", True), ("close_ack", True),
                             ("pressure_threads", True), ("worker_a_poison_refusals", 3.0),
                             ("worker_a_drop_elapsed_ns", float("nan")), ("content", "extra"),
                             ("secret", "public test value"), ("private_key", "public test value")):
            value_dict = dict(final(), **{field: value})
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.parse_final(json.dumps(value_dict).encode(), IDENTITY)
        for field in IDENTITY:
            with self.subTest(identity=field), self.assertRaises(ValueError):
                probe.parse_final(json.dumps(dict(final(), **{field: 99})).encode(), IDENTITY)

    def test_duplicate_packets_missing_matrix_and_fabricated_scope_fail(self):
        payload = json.dumps(final())
        with self.assertRaises(ValueError):
            probe.parse_final((payload[:-1] + ',"close_ack":false}').encode(), IDENTITY)
        for mutation in ("missing", "duplicate", "scope"):
            report = copy.deepcopy(observation())
            if mutation == "missing":
                report["cases"].clear()
            elif mutation == "duplicate":
                report["cases"].append(copy.deepcopy(report["cases"][0]))
            else:
                report["same_store_recovery"] = True
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                probe.validate_observation(report)


if __name__ == "__main__":
    unittest.main()
