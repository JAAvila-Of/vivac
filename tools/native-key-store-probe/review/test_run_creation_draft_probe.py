"""Reject changes to the draft experiment's disposable privilege boundary."""
import copy
import unittest

import run_creation_draft_probe as runner

IMAGE = "sha256:" + "a" * 64
HELPER = "vivac-heap-review-build-" + "1" * 32
WRITER = "vivac-heap-review-build-" + "2" * 32


class DraftControllerTests(unittest.TestCase):
    def configuration(self, case, phase):
        return runner.fixed.expected_config(IMAGE, HELPER, "real", "decoy", "reports", phase, case)

    def test_writer_is_unprivileged_and_only_its_new_state_and_report_are_writable(self):
        with runner.profile(WRITER, "valid"):
            value = self.configuration("valid", "reader")
            self.assertEqual(value["Config"]["User"], "65534:65534")
            host = value["HostConfig"]
            self.assertEqual((host["Memory"], host["MemorySwap"], host["NanoCpus"], host["PidsLimit"]),
                             (512 * 1048576, 512 * 1048576, 1000000000, 128))
            self.assertEqual(host["CapAdd"], [])
            self.assertEqual(host["NetworkMode"], "none")
            self.assertTrue(host["ReadonlyRootfs"])
            writable = {item["Target"] for item in host["Mounts"] if not item["ReadOnly"]}
            self.assertEqual(writable, {"/var/lib/vivac", "/reports"})
            self.assertIn("/probe/creation_draft_probe.py", value["Config"]["Cmd"])
            self.assertNotIn("--launcher-helper", value["Config"]["Cmd"])

    def test_same_device_mount_case_adds_only_the_known_subpath(self):
        with runner.profile(WRITER, "mount-packages"):
            value = self.configuration("mount-packages", "reader")
            item = next(item for item in value["HostConfig"]["Mounts"]
                        if item["Target"].endswith("/state/packages"))
            self.assertEqual(item["Source"], "real")
            self.assertEqual(item["VolumeOptions"],
                             {"NoCopy": True, "Subpath": "custody-v1/users/65534/state/packages"})
            self.assertEqual(value["HostConfig"]["CapAdd"], [])

    def test_configuration_guard_refuses_privilege_mount_and_deadline_changes(self):
        with runner.profile(WRITER, "valid"):
            value = self.configuration("valid", "reader")
            runner.fixed.verify_config(value, HELPER, "real", "decoy", "reports", "reader", "valid", IMAGE)
            for area, key, replacement in (("HostConfig", "Privileged", True),
                                           ("HostConfig", "CapAdd", ["CAP_SYS_ADMIN"]),
                                           ("Config", "Cmd", ["python3", "/probe/creation_draft_probe.py"])):
                changed = copy.deepcopy(value)
                changed[area][key] = replacement
                with self.subTest(key=key), self.assertRaises(runner.fixed.RunnerError):
                    runner.fixed.verify_config(changed, HELPER, "real", "decoy", "reports", "reader", "valid", IMAGE)
            changed = copy.deepcopy(value)
            changed["HostConfig"]["Mounts"][0]["ReadOnly"] = False
            with self.assertRaises(runner.fixed.RunnerError):
                runner.fixed.verify_config(changed, HELPER, "real", "decoy", "reports", "reader", "valid", IMAGE)

    def test_profile_restores_the_existing_controller(self):
        original = runner.fixed.probe
        original_mounts = runner.fixed.mount_specs
        with runner.profile(WRITER, "missing-root"):
            value = self.configuration("missing-root", "setup")
            self.assertEqual(value["Config"]["User"], "0:0")
            self.assertIn("missing-fixed-root", value["Config"]["Cmd"])
        self.assertIs(runner.fixed.probe, original)
        self.assertIs(runner.fixed.mount_specs, original_mounts)
        with self.assertRaises(runner.fixed.RunnerError):
            with runner.profile("unrelated-volume", "valid"):
                pass


if __name__ == "__main__":
    unittest.main()
