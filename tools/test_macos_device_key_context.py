"""Authorization boundary tests; run in an isolated offline container."""

import argparse
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import stat
import subprocess
import sys
import types
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "device_keys_runner", Path(__file__).with_name("test-macos-device-keys.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)

GUID = "11111111-2222-4333-8444-555555555555"
MARKER = "vivac-disposable-macos-account-v1:" + "a" * 32
HOME = "/Users/vivac-test"


class ContextTests(unittest.TestCase):
    def setUp(self):
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(patch.dict(os.environ, {"HOME": HOME}, clear=True))
        self.stack.enter_context(patch.object(runner.sys, "platform", "darwin"))
        self.stack.enter_context(patch.object(runner.os, "getuid", return_value=600))
        self.stack.enter_context(patch.object(runner.os, "geteuid", return_value=600))
        self.account = types.SimpleNamespace(pw_name="vivac-test", pw_uid=600, pw_dir=HOME)
        self.pwd_uid = self.stack.enter_context(patch("pwd.getpwuid", return_value=self.account))
        self.pwd_name = self.stack.enter_context(patch("pwd.getpwnam", return_value=self.account))
        self.stack.enter_context(patch.object(Path, "resolve", lambda p, **kw: p))
        self.stack.enter_context(patch.object(Path, "is_symlink", return_value=False))
        self.metadata = {
            HOME: types.SimpleNamespace(st_mode=stat.S_IFDIR | 0o700, st_uid=600),
            "/Users": types.SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_uid=0),
            "/": types.SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_uid=0),
        }
        self.stack.enter_context(patch.object(Path, "lstat", lambda p: self.metadata[str(p)]))
        self.record = {"dsAttrTypeStandard:UniqueID": ["600"],
                       "dsAttrTypeStandard:GeneratedUID": [GUID],
                       "dsAttrTypeStandard:NFSHomeDirectory": [HOME],
                       "dsAttrTypeStandard:Comment": [MARKER]}
        self.acl = b"drwx------+ 12 vivac-test staff 384 Nov 15 18:20 /Users/vivac-test\n 0: group:everyone deny delete\n"
        self.member = b"user is not a member of the group\n"
        self.calls = []
        self.parents = {
            "/": b"drwxr-xr-x 23 root wheel 736 Sep 24 02:10 /\n",
            "/Users": b"drwxr-xr-x 7 root admin 224 Nov 15 18:20 /Users\n",
        }
        self.stack.enter_context(patch.object(runner, "metadata_command", side_effect=self.command))

    def command(self, argv):
        self.calls.append(argv)
        if argv[0] == "/bin/ls":
            return self.acl if argv[-1] == HOME else self.parents[argv[-1]]
        if argv[0] == "/usr/bin/dscl":
            return plistlib.dumps(self.record)
        if argv[0] == "/usr/bin/dsmemberutil":
            return self.member
        raise AssertionError("unexpected metadata executable")

    def args(self, **changes):
        values = dict(allow_hosted_keychain_test=False,
                      allow_disposable_user_keychain_test=True, test_uid=600,
                      test_guid=GUID, account_marker=MARKER, operator_uid=501)
        values.update(changes)
        return argparse.Namespace(**values)

    def refused(self, **changes):
        with self.assertRaises(runner.Refused):
            runner.authorize_context(self.args(**changes))

    def test_valid_local_account(self):
        self.assertEqual(runner.authorize_context(self.args()), "disposable-user")
        self.assertEqual(len(self.calls), 5)

    def test_no_explicit_authorization(self):
        self.refused(allow_disposable_user_keychain_test=False)
        self.assertFalse(self.calls)

    def test_conflicting_modes(self):
        self.refused(allow_hosted_keychain_test=True)
        self.assertFalse(self.calls)

    def test_wrong_platform(self):
        with patch.object(runner.sys, "platform", "linux"):
            self.refused()
        self.assertFalse(self.calls)

    def test_real_and_effective_identity(self):
        for fn in ("getuid", "geteuid"):
            for uid in (0, 501, 601):
                with self.subTest(fn=fn, uid=uid), patch.object(runner.os, fn, return_value=uid):
                    self.refused()

    def test_independent_pins_required(self):
        for field in ("test_uid", "test_guid", "account_marker", "operator_uid"):
            with self.subTest(field=field):
                self.refused(**{field: None})

    def test_invalid_pin_formats(self):
        for changes in ({"test_uid": 599}, {"test_uid": 1000},
                        {"operator_uid": 600}, {"operator_uid": 0},
                        {"test_guid": "invalid"}, {"account_marker": "invalid"}):
            with self.subTest(changes=changes):
                self.refused(**changes)

    def test_ci_labels_refused_locally(self):
        for name in ("GITHUB_ACTIONS", "RUNNER_ENVIRONMENT"):
            for value in ("true", "", "github-hosted"):
                with self.subTest(name=name, value=value), patch.dict(os.environ, {name: value}):
                    self.refused()

    def test_wrong_home_environment(self):
        with patch.dict(os.environ, {"HOME": "/Users/operator"}):
            self.refused()

    def test_password_database_disagreement(self):
        for field, value in (("pw_name", "operator"), ("pw_uid", 501), ("pw_dir", "/Users/operator")):
            before = getattr(self.account, field)
            setattr(self.account, field, value)
            with self.subTest(field=field):
                self.refused()
            setattr(self.account, field, before)

    def test_home_and_parent_permissions(self):
        for path, field, value in ((HOME, "st_mode", stat.S_IFDIR | 0o755),
                                   (HOME, "st_uid", 501), ("/Users", "st_uid", 600),
                                   ("/", "st_mode", stat.S_IFDIR | 0o777)):
            before = getattr(self.metadata[path], field)
            setattr(self.metadata[path], field, value)
            with self.subTest(path=path, field=field):
                self.refused()
            setattr(self.metadata[path], field, before)

    def test_home_symlink(self):
        with patch.object(Path, "is_symlink", return_value=True):
            self.refused()

    def test_parent_acl_refused(self):
        for path in self.parents:
            before = self.parents[path]
            self.parents[path] = before + b" 0: group:everyone allow add_subdirectory\n"
            with self.subTest(path=path):
                self.refused()
            self.parents[path] = before

    def test_parent_acl_indicator_without_entry_refused(self):
        self.parents["/Users"] = self.parents["/Users"].replace(b"drwxr-xr-x ", b"drwxr-xr-x+ ")
        self.refused()

    def test_acl_grants_or_extra_entries_refused(self):
        for acl in (b" 0: group:everyone allow read\n", b" 1: user:operator allow read\n"):
            self.acl += acl
            self.refused()

    def test_absent_acl_accepted(self):
        self.acl = b"drwx------ 12 vivac-test staff 384 Nov 15 18:20 /Users/vivac-test\n"
        self.assertEqual(runner.authorize_context(self.args()), "disposable-user")

    def test_directory_service_pins(self):
        for field in self.record:
            before = self.record[field]
            for value in ([], ["wrong"], before + before):
                self.record[field] = value
                with self.subTest(field=field, value=value):
                    self.refused()
            self.record[field] = before

    def test_administrator_and_unknown_membership(self):
        for output in (b"user is a member of the group\n", b"unknown\n", b""):
            self.member = output
            self.refused()

    def test_hosted_gate_preserved(self):
        args = self.args(allow_hosted_keychain_test=True,
                         allow_disposable_user_keychain_test=False,
                         test_uid=None, test_guid=None, account_marker=None, operator_uid=None)
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted"}):
            self.assertEqual(runner.authorize_context(args), "github-hosted")
            args.test_uid = 600
            with self.assertRaises(runner.Refused):
                runner.authorize_context(args)
        self.assertFalse(self.calls)

    def test_check_context_precedes_all_native_work(self):
        argv = ["--allow-disposable-user-keychain-test", "--test-uid", "600",
                "--test-guid", GUID, "--account-marker", MARKER,
                "--operator-uid", "501", "--check-context"]
        out = io.StringIO()
        with patch.object(sys, "argv", ["runner"] + argv), contextlib.redirect_stdout(out), \
                patch.object(runner, "inventory", side_effect=AssertionError("Keychain touched")), \
                patch.object(runner.tempfile, "mkdtemp", side_effect=AssertionError("fixture created")):
            self.assertEqual(runner.main(), 0)
        self.assertEqual(json.loads(out.getvalue()), {"status": "authorized", "profile": "disposable-user"})


class MetadataProcessTests(unittest.TestCase):
    def exercise(self, source, succeeds=False):
        original = subprocess.Popen
        children = []

        def launch(argv, **kwargs):
            self.assertEqual(kwargs["env"], {"LC_ALL": "C", "PATH": "/usr/bin:/bin", "HOME": HOME})
            self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
            child = original([sys.executable, "-I", "-S", "-B", "-c", source], **kwargs)
            children.append(child)
            return child

        with patch.object(runner.subprocess, "Popen", side_effect=launch):
            if succeeds:
                self.assertEqual(runner.metadata_command(["/bin/ls"]), b"ok\n")
            else:
                with self.assertRaises(runner.Refused):
                    runner.metadata_command(["/bin/ls"])
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll())
        self.assertTrue(children[0].stdout.closed and children[0].stderr.closed)

    def test_small_metadata_success(self):
        self.exercise("print('ok')", succeeds=True)

    def test_stdout_overflow(self):
        self.exercise("import os; os.write(1, b'x' * 65536)")

    def test_stderr_overflow(self):
        self.exercise("import os; os.write(2, b'x' * 65536)")

    def test_stderr_even_on_success_refused(self):
        self.exercise("import os; os.write(2, b'warning')")

    def test_nonzero_exit_refused(self):
        self.exercise("raise SystemExit(3)")

    def test_deadline_kills_and_reaps_child(self):
        self.exercise("import time; time.sleep(30)")

    def test_other_executable_refused_before_launch(self):
        with patch.object(runner.subprocess, "Popen") as launch:
            with self.assertRaises(runner.Refused):
                runner.metadata_command(["/usr/bin/security"])
        launch.assert_not_called()


class InventoryTests(unittest.TestCase):
    MISSING = b'security: SecKeychainCopyDomainDefault user: A default keychain could not be found.\n'

    def inventory(self, local=True, search=b'', code=1, output=b'', error=None, list_code=0):
        values = [types.SimpleNamespace(returncode=list_code, stdout=search, stderr=b''),
                  types.SimpleNamespace(returncode=code, stdout=output,
                                        stderr=self.MISSING if error is None else error)]
        with patch.object(runner.subprocess, 'run', side_effect=values) as query:
            result = runner.inventory(allow_missing_default=local)
        self.assertEqual(query.call_args.kwargs['env']['LC_ALL'], 'C')
        return result

    def test_explicit_local_absence_is_stable(self):
        self.assertEqual(self.inventory(), self.inventory())
        self.assertEqual(self.inventory()[1][0], 1)

    def test_hosted_absence_refused(self):
        with self.assertRaises(runner.Refused):
            self.inventory(local=False)

    def test_absence_requires_empty_search_list(self):
        with self.assertRaises(runner.Refused):
            self.inventory(search=b'"/fixture/keychain"\n')

    def test_query_errors_are_not_absence(self):
        for changes in ({'code': 2}, {'error': b'permission denied\n'}, {'output': b'unexpected'},
                        {'list_code': 1}, {'error': b''}):
            with self.subTest(changes=changes), self.assertRaises(runner.Refused):
                self.inventory(**changes)

    def test_created_default_changes_inventory(self):
        before = self.inventory()
        after = self.inventory(code=0, output=b'"/fixture/new-keychain"\n', error=b'')
        self.assertNotEqual(before, after)

    def test_existing_hosted_inventory_remains_valid(self):
        result = self.inventory(local=False, search=b'fixture\n', code=0, output=b'fixture\n', error=b'')
        self.assertEqual(result[1][0], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
