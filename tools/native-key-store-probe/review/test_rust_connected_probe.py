"""Pure control validation for connected sibling transport observations."""
import copy
import json
import unittest
from contextlib import ExitStack
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import rust_connected_probe as probe
import run_lifecycle_probe as runner


def final(case, historical_server_hello=True):
    forwarded = case == "forwarded-sender"
    historical = case == "historical-listener"
    mismatch = case in ("wrong-generation", "wrong-store", "wrong-role")
    value = dict.fromkeys(probe.SCOPE_FIELDS, False)
    value.update(version=1, case=case, status="observed", host_pid=10, creator_tid=10, server_pid=11, server_token=1,
                 client_pid=12, client_token=2, injector_pid=13 if forwarded else None,
                 injector_token=1 if forwarded else None, server_connected_peer_pid=12,
                 client_connected_peer_pid=10 if historical else 11, listener_origin_parent=historical,
                 server_exit_code=0, client_exit_code=0, injector_exit_code=0 if forwarded else None,
                 server_exit_observed=True, owned_exits_observed=True, resources_closed_after_exit=True,
                 pressure_threads=4, pressure_iterations=1)
    for side in ("server", "client"):
        hello = not mismatch and not (historical and (side == "client" or not historical_server_hello))
        bad = not hello or side == "server" and forwarded or side == "client" and case == "peer-exit"
        error = None if hello else "invalid-data" if mismatch else "permission-denied"
        context_exited = side == "client" and case == "peer-exit"
        value.update({side + "_context_captured": True, side + "_context_identities_verified": True,
                      side + "_context_valid_after_exchange": not context_exited,
                      side + "_context_peer_exit_refused": context_exited,
                      side + "_context_sticky_refusal": context_exited})
        value.update({side + "_handshake_completed": hello, side + "_hello_error": error,
                      side + "_accepted_probes": 2 if case == "roundtrip" else 0,
                      side + "_refusals": int(bad), side + "_poisoned": bad, side + "_sticky_refusal": bad})
    return value


def observation():
    report = dict.fromkeys(probe.SCOPE_FIELDS, False)
    report.update(version=1, experiment="rust-connected", status="observed", cases=[], resource_limits={},
                  resources_closed_after_observed_exit=True, helper_sha256="a" * 64, observer_sha256="b" * 64,
                  kernel_release="6.6.87.2-microsoft-standard-WSL2")
    for case in sorted(probe.CASES):
        value = final(case)
        for field in probe.EXTERNAL_FIELDS:
            value[field] = (case == "forwarded-sender" if field.startswith("injector_") else
                            case != "peer-exit" if field == "server_retained_until_observation" else
                            0 if field == "host_exit_code" else True)
        report["cases"].append(value)
    return report


class ConnectedValidationTests(unittest.TestCase):
    def main_with_os_error(self, operation, error):
        written = []
        with ExitStack() as stack:
            stack.enter_context(patch.object(probe.sys, "argv", ["rust_connected_probe.py", "--writer", "public-writer", "--verifier", "public-verifier",
                "--launcher-helper", "public-helper", "--store-root", "public-store", "--report", "public-report"]))
            stack.enter_context(patch.object(probe.sys, "platform", "linux"))
            stack.enter_context(patch.dict(probe.os.environ, {"VIVAC_OPENSSH_DISPOSABLE": "1"}))
            stack.enter_context(patch.object(probe.os, "uname", return_value=SimpleNamespace(release="6.8.0-public"), create=True))
            for owner, name in ((probe.life, "require_isolation"), (probe.life, "require_unprivileged"),
                                (probe.session, "require_report_directory"), (probe.session, "require_store")):
                stack.enter_context(patch.object(owner, name))
            stack.enter_context(patch.object(probe.life, "prctl", return_value=1))
            stack.enter_context(patch.object(probe.life, "require_resources", return_value={}))
            stack.enter_context(patch.object(probe.Path, "read_bytes", return_value=b"public fixture"))
            stack.enter_context(patch.object(probe.life, "private_write", side_effect=lambda path, data: written.append(json.loads(data))))
            stack.enter_context(patch.object(probe.sys, "stderr"))
            capability = stack.enter_context(patch.object(probe, "require_peer_pidfd_capability"))
            run_case = stack.enter_context(patch.object(probe, "run_case"))
            (capability if operation == "capability" else run_case).side_effect = error
            self.assertEqual(probe.main(), 1)
        self.assertEqual(len(written), 1)
        result = written[0]
        self.assertEqual(result["status"], "incomplete")
        self.assertEqual(result["cases"], [])
        self.assertFalse(result["resources_closed_after_observed_exit"])
        self.assertTrue(all(result[field] is False for field in probe.SCOPE_FIELDS))
        return result["failure"]

    def test_case_os_error_reports_errno_and_bounded_trace_without_private_data(self):
        namespace = {"PermissionError": PermissionError}
        exec(compile("def nested(depth):\n    if depth:\n        return nested(depth - 1)\n    raise PermissionError(13, 'sensitive payload', '/private/credential')\n", "/private/directory/private_test.py", "exec"), namespace)
        failure = self.main_with_os_error("case", lambda *args: namespace["nested"](20))
        self.assertEqual(failure["kind"], "PermissionError")
        self.assertEqual(failure["errno"], 13)
        self.assertEqual(failure["checkpoint"], "case:forwarded-sender")
        self.assertEqual(len(failure["traceback"]), 8)
        for frame in failure["traceback"]:
            self.assertEqual(set(frame), {"module", "function", "line"})
            self.assertEqual(frame["module"], "private_test.py")
            self.assertEqual(frame["function"], "nested")
            self.assertGreater(frame["line"], 0)
        encoded = json.dumps(failure)
        for omitted in ("sensitive payload", "/private/", "credential"):
            self.assertNotIn(omitted, encoded)

    def test_capability_os_error_keeps_original_errno_and_checkpoint(self):
        failure = self.main_with_os_error("capability", PermissionError(1, "sensitive payload", "/private/credential"))
        self.assertEqual(failure["kind"], "PermissionError")
        self.assertEqual(failure["errno"], 1)
        self.assertEqual(failure["checkpoint"], "capability")
        self.assertTrue(0 < len(failure["traceback"]) <= 8)
        self.assertNotIn("sensitive payload", json.dumps(failure))
        self.assertNotIn("/private/", json.dumps(failure))

    def capacity(self, error=None):
        left, right = MagicMock(), MagicMock()
        for endpoint in (left, right):
            endpoint.__enter__.return_value = endpoint
            endpoint.get_inheritable.return_value = False
        left.getsockopt.return_value = 87
        if error is not None:
            left.getsockopt.side_effect = error
        return left, right

    def capacity_constants(self):
        stack = ExitStack()
        stack.enter_context(patch.multiple(probe.socket, AF_UNIX=1, SOCK_SEQPACKET=5, SOCK_CLOEXEC=0x80000, SO_PASSCRED=16, create=True))
        stack.enter_context(patch.multiple(probe.errno, ENOPROTOOPT=92, EOPNOTSUPP=95))
        return stack

    def test_capability_diagnostic_closes_returned_pidfd_immediately_without_admission(self):
        left, right = self.capacity()
        with self.capacity_constants(), patch.object(probe.socket, "socketpair", return_value=(left, right)) as pair, \
                patch.object(probe.os, "close") as close:
            probe.require_peer_pidfd_capability()
            pair.assert_called_once_with(probe.socket.AF_UNIX, 5 | 0x80000)
            left.getsockopt.assert_called_once_with(probe.socket.SOL_SOCKET, 77)
            for endpoint in (left, right):
                endpoint.setsockopt.assert_called_once_with(probe.socket.SOL_SOCKET, 16, 1)
                endpoint.__exit__.assert_called_once()
            close.assert_called_once_with(87)

    def test_unsupported_option_preserves_errno_and_closes_both_endpoints(self):
        left, right = self.capacity(OSError(92, "public unsupported option fixture"))
        with self.capacity_constants(), patch.object(probe.socket, "socketpair", return_value=(left, right)), \
                patch.object(probe.os, "close") as close:
            with self.assertRaises(probe.KernelCapabilityUnavailable) as raised:
                probe.require_peer_pidfd_capability()
            self.assertEqual(raised.exception.errno, 92)
            close.assert_not_called()
            left.__exit__.assert_called_once()
            right.__exit__.assert_called_once()

    def test_other_option_errors_remain_failures_without_capability_reclassification(self):
        left, right = self.capacity(OSError(9, "public invalid descriptor fixture"))
        with self.capacity_constants(), patch.object(probe.socket, "socketpair", return_value=(left, right)), \
                patch.object(probe.os, "close") as close:
            with self.assertRaises(OSError) as raised:
                probe.require_peer_pidfd_capability()
            self.assertEqual(raised.exception.errno, 9)
            close.assert_not_called()

    def test_unavailable_kernel_reports_incomplete_errno_92_and_never_runs_cases(self):
        written = []
        with ExitStack() as stack:
            stack.enter_context(patch.object(probe.sys, "argv", ["probe", "--writer", "public-writer", "--verifier", "public-verifier",
                "--launcher-helper", "public-helper", "--store-root", "public-store", "--report", "public-report"]))
            stack.enter_context(patch.object(probe.sys, "platform", "linux"))
            stack.enter_context(patch.dict(probe.os.environ, {"VIVAC_OPENSSH_DISPOSABLE": "1"}))
            stack.enter_context(patch.object(probe.os, "uname", return_value=SimpleNamespace(release="5.15.167.4-public"), create=True))
            for owner, name in ((probe.life, "require_isolation"), (probe.life, "require_unprivileged"),
                                (probe.session, "require_report_directory"), (probe.session, "require_store")):
                stack.enter_context(patch.object(owner, name))
            stack.enter_context(patch.object(probe.life, "prctl", return_value=1))
            stack.enter_context(patch.object(probe.life, "require_resources", return_value={}))
            stack.enter_context(patch.object(probe.Path, "read_bytes", return_value=b"public fixture"))
            stack.enter_context(patch.object(probe.life, "private_write", side_effect=lambda path, data: written.append(json.loads(data))))
            stack.enter_context(patch.object(probe.sys, "stderr"))
            stack.enter_context(patch.object(probe, "require_peer_pidfd_capability", side_effect=probe.KernelCapabilityUnavailable(92)))
            run_case = stack.enter_context(patch.object(probe, "run_case"))
            self.assertEqual(probe.main(), 1)
            run_case.assert_not_called()
        self.assertEqual(len(written), 1)
        self.assertEqual(written[0]["failure"], {"kind": "KernelCapabilityUnavailable", "option": "SO_PEERPIDFD", "errno": 92})
        self.assertEqual(written[0]["status"], "incomplete")
        self.assertEqual(written[0]["cases"], [])
        self.assertFalse(written[0]["resources_closed_after_observed_exit"])
        self.assertTrue(all(written[0][field] is False for field in probe.SCOPE_FIELDS))

    def test_seven_exact_cases_and_control_stdout_parser(self):
        report = observation()
        probe.validate_observation(report)
        runner.validate_rust_connected_observation(report)
        self.assertEqual(runner.EXPERIMENTS["rust-connected"], ("rust_connected_probe.py", probe.CASES))
        for case in probe.CASES:
            expected = final(case)
            self.assertEqual(probe.parse_result(json.dumps(expected).encode(), case), expected)

    def test_historical_listener_records_actual_server_outcome_without_false_poison(self):
        for server_hello in (True, False):
            value = final("historical-listener", server_hello)
            probe.validate_result(value, "historical-listener")
        value = final("historical-listener")
        value["server_poisoned"] = value["server_sticky_refusal"] = True
        with self.assertRaises(ValueError):
            probe.validate_result(value, "historical-listener")

    def test_historical_client_must_reject_sender_identity_while_root_remains_bound(self):
        for field, replacement in (("client_connected_peer_pid", 11), ("client_handshake_completed", True),
                                   ("client_hello_error", "invalid-data"), ("client_hello_error", "timed-out"),
                                   ("client_sticky_refusal", False), ("listener_origin_parent", False)):
            value = final("historical-listener")
            value[field] = replacement
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_result(value, "historical-listener")

    def test_mismatch_failures_are_frame_errors_and_cannot_be_timeout_success(self):
        for case in ("wrong-generation", "wrong-store", "wrong-role"):
            for error in (None, "permission-denied", "timed-out"):
                value = final(case)
                value["client_hello_error"] = error
                with self.subTest(case=case, error=error), self.assertRaises(ValueError):
                    probe.validate_result(value, case)

    def test_forwarded_sender_requires_only_the_side_that_observed_refusal_to_poison(self):
        value = final("forwarded-sender")
        probe.validate_result(value, "forwarded-sender")
        self.assertFalse(value["client_poisoned"])
        self.assertTrue(value["server_sticky_refusal"])
        for field, replacement in (("server_sticky_refusal", False), ("server_refusals", 0),
                                   ("injector_pid", 12), ("injector_token", None)):
            changed = dict(value, **{field: replacement})
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_result(changed, "forwarded-sender")

    def test_observation_cannot_claim_future_exit_or_resource_closure(self):
        for case in probe.CASES:
            value = final(case)
            value.update(stage="observation", server_exit_code=0 if case == "peer-exit" else None,
                         client_exit_code=None, injector_exit_code=None, server_exit_observed=case == "peer-exit",
                         owned_exits_observed=False, resources_closed_after_exit=False, pressure_iterations=0)
            probe.validate_result(value, case, observation=True)
            value["client_exit_code"] = 0
            with self.subTest(case=case), self.assertRaises(ValueError):
                probe.validate_result(value, case, observation=True)

    def test_endpoint_authority_and_all_other_acknowledgments_stay_false(self):
        for field in probe.SCOPE_FIELDS:
            for nested in (True, False):
                report = observation()
                (report["cases"][0] if nested else report)[field] = True
                with self.subTest(field=field, nested=nested), self.assertRaises(ValueError):
                    probe.validate_observation(report)

    def test_schema_rejects_unselected_payloads_and_boolean_counters(self):
        for field, value in (("version", True), ("client_token", True), ("client_accepted_probes", True),
                             ("client_exit_code", False), ("server_connected_peer_pid", True),
                             ("generation", "unexpected"), ("store_words", [1, 2, 3]), ("contents", "unexpected")):
            report = final("roundtrip")
            report[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_result(report, "roundtrip")

    def test_context_observations_are_exact_and_do_not_replace_sender_validation(self):
        for case in probe.CASES:
            value = final(case)
            probe.validate_result(value, case)
            for side in ("server", "client"):
                for field in ("context_captured", "context_identities_verified", "context_valid_after_exchange",
                              "context_peer_exit_refused", "context_sticky_refusal"):
                    name = side + "_" + field
                    for replacement in (not value[name], int(value[name]), None):
                        changed = dict(value, **{name: replacement})
                        with self.subTest(case=case, field=name, replacement=replacement), self.assertRaises(ValueError):
                            probe.validate_result(changed, case)
                    changed = dict(value)
                    del changed[name]
                    with self.subTest(case=case, missing=name), self.assertRaises(ValueError):
                        probe.validate_result(changed, case)
        for case, side in (("historical-listener", "client"), ("forwarded-sender", "server")):
            value = final(case)
            self.assertTrue(value[side + "_context_valid_after_exchange"])
            self.assertFalse(value[side + "_handshake_completed"] if case == "historical-listener"
                             else not value[side + "_poisoned"])

    def test_missing_duplicate_or_extra_case_is_incomplete(self):
        for mutation in (lambda rows: rows.pop(), lambda rows: rows.append(copy.deepcopy(rows[0])),
                         lambda rows: rows.__setitem__(0, copy.deepcopy(rows[1]))):
            report = observation()
            mutation(report["cases"])
            with self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_external_live_identity_and_normal_exit_are_required(self):
        for field, replacement in (("root_identity_verified", False), ("client_retained_until_observation", False),
                                   ("injector_exit_observed_externally", False), ("host_exit_code", -9),
                                   ("resources_closed_after_observed_exit", False)):
            report = observation()
            next(value for value in report["cases"] if value["case"] == "forwarded-sender")[field] = replacement
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.validate_observation(report)

    def test_kernel_metadata_is_printable_bounded_and_never_an_admission_fallback(self):
        for value in (None, "", "6.6\n", "6.6\x1b", "6.6 bad", "a" * 129, "versión"):
            report = observation()
            report["kernel_release"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                probe.validate_observation(report)
        report = observation()
        report["kernel_release"] = "0.0-public-fixture"
        probe.validate_observation(report)


if __name__ == "__main__":
    unittest.main()
