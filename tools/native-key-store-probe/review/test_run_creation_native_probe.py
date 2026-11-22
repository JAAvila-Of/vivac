"""Guard the native creation experiment's disposable execution boundary."""
import copy
import unittest

import run_creation_native_probe as runner

IMAGE = "sha256:" + "a" * 64
HELPER = "vivac-heap-review-build-" + "1" * 32
WRITER = "vivac-heap-review-build-" + "2" * 32
CASES = tuple(runner.native.CASES)


class NativeCreationControllerTests(unittest.TestCase):
    def configuration(self, case, phase="reader"):
        return runner.fixed.expected_config(IMAGE, HELPER, "real", "decoy", "reports", phase, case)

    def verify(self, value, case="complete", phase="reader"):
        runner.fixed.verify_config(value, HELPER, "real", "decoy", "reports", phase, case, IMAGE)

    def test_all_cases_accept_the_exact_isolated_profile(self):
        for case in CASES:
            with self.subTest(case=case), runner.profile(WRITER, case):
                value = self.configuration(case)
                self.verify(value, case)
                self.assertEqual(value["Config"]["User"], "65534:65534")
                self.assertIn("/probe/creation_native_probe.py", value["Config"]["Cmd"])
                self.assertNotIn("/probe/creation_draft_probe.py", value["Config"]["Cmd"])
                host = value["HostConfig"]
                self.assertEqual((host["Memory"], host["MemorySwap"], host["NanoCpus"], host["PidsLimit"]),
                                 (512 * 1048576, 512 * 1048576, 1000000000, 128))
                self.assertEqual(host["CapAdd"], [])
                self.assertEqual(host["NetworkMode"], "none")
                self.assertTrue(host["ReadonlyRootfs"])
                self.assertEqual({item["Target"] for item in host["Mounts"] if not item["ReadOnly"]},
                                 {"/var/lib/vivac", "/reports"})
                writer = next(item for item in host["Mounts"] if item["Target"] == "/draft-build")
                self.assertEqual(writer["Source"], WRITER)
                self.assertTrue(writer["ReadOnly"])
                setup = self.configuration(case, "setup")
                self.verify(setup, case, "setup")
                self.assertEqual(setup["Config"]["User"], "0:0")
                self.assertIn("fixed-valid", setup["Config"]["Cmd"])

    def test_configuration_rejects_script_privilege_and_resource_changes(self):
        with runner.profile(WRITER, "complete"):
            value = self.configuration("complete")
            self.verify(value)
            replacements = (
                ("HostConfig", "Privileged", True),
                ("HostConfig", "CapAdd", ["CAP_SYS_ADMIN"]),
                ("HostConfig", "Memory", 1024 * 1048576),
                ("HostConfig", "MemorySwap", 1024 * 1048576),
                ("HostConfig", "NanoCpus", 2000000000),
                ("HostConfig", "PidsLimit", 256),
                ("HostConfig", "NetworkMode", "host"),
                ("HostConfig", "ReadonlyRootfs", False),
                ("Config", "User", "0:0"),
            )
            for area, key, replacement in replacements:
                changed = copy.deepcopy(value)
                changed[area][key] = replacement
                with self.subTest(key=key), self.assertRaises(runner.fixed.RunnerError):
                    self.verify(changed)
            changed = copy.deepcopy(value)
            command = changed["Config"]["Cmd"]
            command[command.index("/probe/creation_native_probe.py")] = "/probe/creation_draft_probe.py"
            with self.assertRaises(runner.fixed.RunnerError):
                self.verify(changed)
            changed = copy.deepcopy(value)
            changed["Config"]["Cmd"][2] = "480s"
            with self.assertRaises(runner.fixed.RunnerError):
                self.verify(changed)

    def test_fresh_reader_selection_is_fixed_and_mutations_are_disposable(self):
        for selected in runner.PUBLIC_READER_CHECKS:
            with self.subTest(check=selected), runner.profile(WRITER, "publish-complete", selected):
                value = self.configuration("publish-complete")
                self.verify(value, "publish-complete")
                command = value["Config"]["Cmd"]
                self.assertEqual(command[command.index("--reader") + 1],
                                 "/launcher-build/launcher189/debug/rust-launcher-probe")
                self.assertEqual(command[command.index("--reader-check") + 1], selected)
                changed = copy.deepcopy(value)
                changed_command = changed["Config"]["Cmd"]
                changed_command[changed_command.index("--reader") + 1] = "/personal/reader"
                with self.assertRaises(runner.fixed.RunnerError):
                    self.verify(changed, "publish-complete")
        for case, selected in (("complete", "record-replacement"), ("publish-complete", "repair"),
                               ("publish-complete", "unrelated-publication")):
            with self.subTest(case=case, check=selected), self.assertRaises(runner.fixed.RunnerError):
                with runner.profile(WRITER, case, selected):
                    pass

    def test_configuration_rejects_writable_code_and_foreign_mounts(self):
        with runner.profile(WRITER, "complete"):
            value = self.configuration("complete")
            self.verify(value)
            for target in ("/probe", "/launcher-build", "/draft-build", "/decoy"):
                changed = copy.deepcopy(value)
                next(item for item in changed["HostConfig"]["Mounts"]
                     if item["Target"] == target)["ReadOnly"] = False
                with self.subTest(target=target), self.assertRaises(runner.fixed.RunnerError):
                    self.verify(changed)
            changed = copy.deepcopy(value)
            next(item for item in changed["HostConfig"]["Mounts"]
                 if item["Target"] == "/var/lib/vivac")["Source"] = "personal-store"
            with self.assertRaises(runner.fixed.RunnerError):
                self.verify(changed)

    def test_incomplete_observation_retains_the_isolated_stores(self):
        with runner.profile(WRITER, "complete"):
            with self.assertRaises(runner.fixed.RunnerError):
                runner.fixed.command(["docker", "volume", "rm", "real"])

    def test_profile_restores_shared_controller_after_an_exception(self):
        original_draft = runner.base.draft
        names = ("create_arguments", "mount_specs", "probe", "command", "LIMITS", "compare_phases")
        original = {name: getattr(runner.fixed, name) for name in names}
        with self.assertRaisesRegex(RuntimeError, "fixture interruption"):
            with runner.profile(WRITER, "complete"):
                self.assertIsNot(runner.base.draft, original_draft)
                raise RuntimeError("fixture interruption")
        self.assertIs(runner.base.draft, original_draft)
        for name, expected in original.items():
            self.assertIs(getattr(runner.fixed, name), expected)
        for writer, case in (("personal-volume", "complete"), (WRITER, "valid")):
            with self.subTest(writer=writer, case=case), self.assertRaises(runner.fixed.RunnerError):
                with runner.profile(writer, case):
                    pass


if __name__ == "__main__":
    unittest.main()
