"""Metadata-storage evidence must not silently become admission or durability."""
import copy
import unittest
import rust_admission_storage_probe as probe

CASES = ("roundtrip", "existing-destination", "concurrent-publishers", "snapshot-in-place",
         "snapshot-replacement", "symlink-refused", "hardlink-refused", "pre-rename-error",
         "post-rename-sync-error", "crash-before-rename", "crash-after-rename", "acl-refused")
SCOPES = ("custody_accredited", "creation_admission", "producer_origin_checked", "root_bootstrap_accredited",
          "endpoint_authority_accredited", "product_recovery", "same_store_recovery", "mcp_integration",
          "server_integration", "lock_ack")

def fixture(case, *, observation=False):
    concurrent = case == "concurrent-publishers"
    crash = case.startswith("crash-")
    children = [{"pid": 21 + n, "token": 1 + n, "ordinal": 1 + n} for n in range(2 if concurrent else int(crash))]
    exits = [{"pid": c["pid"], "token": c["token"], "code": None if crash else 0, "signal": 9 if crash else None} for c in children]
    value = dict.fromkeys(SCOPES, False)
    value.update(version=1, case=case, status="observed", identity={"host_pid": 20, "creator_tid": 20, "children": children},
        successful_publications=int(case in CASES[:7] or case == "acl-refused"),
        unpublished_refusals=int(case in ("existing-destination", "concurrent-publishers", "pre-rename-error", "post-rename-sync-error", "crash-after-rename")),
        metadata_visible=case not in ("pre-rename-error", "crash-before-rename"),
        snapshot_invalidated=case in ("snapshot-in-place", "snapshot-replacement"),
        alias_checks=2 if case == "symlink-refused" else 1 if case == "hardlink-refused" else 0,
        access_policy_checks=3 if case == "acl-refused" else 0,
        reader_refused=case in ("symlink-refused", "hardlink-refused", "pre-rename-error", "crash-before-rename", "acl-refused"),
        existing_unchanged=case in ("existing-destination", "post-rename-sync-error", "crash-after-rename"),
        durability_indeterminate=case in ("post-rename-sync-error", "crash-after-rename"),
        injected_failure="before-rename" if case == "pre-rename-error" else "before-parent-sync" if case == "post-rename-sync-error" else "none",
        child_exits=exits if not observation or crash else [], owned_exits_observed=not observation or crash,
        resources_closed_after_exit=not observation, pressure_threads=4, pressure_iterations=0 if observation else 10)
    if not observation:
        value.update(process_identities_verified=True, host_retained_until_observation=True,
                     children_observed_alive_before_action=bool(children),
                     children_retained_until_observation=concurrent,
                     children_exits_observed_externally=True, host_exit_code=0, resources_closed_after_observed_exit=True)
    return value

def observation():
    value = dict.fromkeys(SCOPES, False)
    value.update(version=1, experiment="rust-admission-storage", status="observed",
                 cases=[fixture(case) for case in CASES], metadata_store_observed=True,
                 resources_closed_after_observed_exit=True, kernel_release="6.8.0-fixture",
                 helper_sha256="1" * 64, observer_sha256="2" * 64,
                 resource_limits={"memory_bytes": 536870912, "swap_bytes": 0, "pids": 128,
                                  "cpu_quota": 100000, "cpu_period": 100000, "temporary_bytes": 134217728})
    return value


class EvidenceTests(unittest.TestCase):
    def test_acl_observation_requires_all_three_policy_checks(self):
        probe.validate_case(fixture("acl-refused"))
        for count in (0, 2, False, True):
            value = fixture("acl-refused")
            value["access_policy_checks"] = count
            with self.subTest(count=count), self.assertRaises(ValueError): probe.validate_case(value)

    def test_all_expected_cases_are_valid(self):
        for case in CASES:
            probe.validate_case(fixture(case))
        probe.validate_observation(observation())

    def test_global_observation_requires_all_distinct_cases(self):
        value = observation()
        value["cases"][-1] = copy.deepcopy(value["cases"][0])
        with self.assertRaises(ValueError): probe.validate_observation(value)

    def test_cannot_promote_metadata_to_admission(self):
        for field in SCOPES:
            value = fixture("roundtrip")
            value[field] = True
            with self.assertRaises(ValueError):
                probe.validate_case(value)

    def test_post_rename_error_is_not_durable_success(self):
        value = fixture("post-rename-sync-error")
        value["successful_publications"] = 1
        with self.assertRaises(ValueError): probe.validate_case(value)

    def test_crashed_writer_has_exact_signal_receipt(self):
        value = fixture("crash-after-rename")
        value["child_exits"][0]["signal"] = 15
        with self.assertRaises(ValueError): probe.validate_case(value)

    def test_cannot_credit_future_concurrent_exits(self):
        value = fixture("concurrent-publishers", observation=True)
        value["owned_exits_observed"] = True
        with self.assertRaises(ValueError): probe.validate_result(value, value["case"], observation=True)

    def test_concurrent_publishers_need_two_distinct_owned_children(self):
        value = fixture("concurrent-publishers")
        value["identity"]["children"][1]["pid"] = value["identity"]["children"][0]["pid"]
        with self.assertRaises(ValueError): probe.validate_case(value)

    def test_no_unbounded_or_content_fields_are_accepted(self):
        value = fixture("roundtrip")
        value["record_contents"] = "public but outside the schema"
        with self.assertRaises(ValueError): probe.validate_case(value)

    def test_visible_record_after_crash_does_not_imply_success(self):
        value = fixture("crash-after-rename")
        value["durability_indeterminate"] = False
        with self.assertRaises(ValueError): probe.validate_case(value)

    def test_child_receipts_cannot_change_after_observation(self):
        value = fixture("concurrent-publishers", observation=True)
        identity = copy.deepcopy(value["identity"])
        value["identity"]["children"][0]["token"] += 1
        with self.assertRaises(ValueError): probe.validate_result(value, value["case"], identity, observation=True)

if __name__ == "__main__": unittest.main()
