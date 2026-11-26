"""Pure regressions for provider selection and bounded local metadata."""

from pathlib import Path
import stat
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import provider_probe as probe


class ProviderTests(unittest.TestCase):
    def test_duplicate_package_file_claim_is_refused(self):
        value = (b"a" * 32 + b"  usr/bin/ssh-add\n" + b"b" * 32 + b"  usr/bin/ssh-add\n")
        with self.assertRaises(probe.ProbeError):
            probe.parse_md5(value)

    def test_unsafe_file_is_refused_before_any_binary_read(self):
        metadata = SimpleNamespace(st_mode=stat.S_IFREG | 0o755, st_uid=65534, st_size=32)
        with self.assertRaises(probe.ProbeError):
            probe.validate_metadata(metadata, executable=True)

    def test_root_file_writable_by_another_account_is_refused(self):
        for mode in (0o775, 0o757, 0o4775):
            with self.subTest(mode=mode), self.assertRaises(probe.ProbeError):
                probe.validate_metadata(SimpleNamespace(st_mode=stat.S_IFREG | mode, st_uid=0, st_size=32), executable=True)

    def test_setgid_is_only_explicitly_allowed_while_setuid_never_is(self):
        metadata = SimpleNamespace(st_mode=stat.S_IFREG | 0o2755, st_uid=0, st_size=32)
        with self.assertRaises(probe.ProbeError):
            probe.validate_metadata(metadata, executable=True)
        probe.validate_metadata(metadata, executable=True, allow_setgid=True)
        metadata.st_mode = stat.S_IFREG | 0o4755
        with self.assertRaises(probe.ProbeError):
            probe.validate_metadata(metadata, executable=True, allow_setgid=True)

    def test_oversize_executable_and_metadata_are_refused(self):
        for executable, size in ((True, probe.BINARY_LIMIT + 1), (False, probe.METADATA_LIMIT + 1)):
            with self.subTest(executable=executable), self.assertRaises(probe.ProbeError):
                probe.validate_metadata(SimpleNamespace(st_mode=stat.S_IFREG | 0o755, st_uid=0, st_size=size), executable=executable)

    def test_metadata_cannot_supply_shell_expressions_or_other_os(self):
        for value in (b'ID=ubuntu\nVERSION_ID="$(id)"\n', b'ID=ubuntu\nID=ubuntu\nVERSION_ID=24.04\n',
                      b'ID=debian\nVERSION_ID=13\n', b'ID=ubuntu\nVERSION_ID=26.04\n'):
            with self.subTest(value=value), self.assertRaises(probe.ProbeError):
                probe.parse_os_release(value)
        self.assertEqual(probe.parse_os_release(b'NAME="Ubuntu"\nID=ubuntu\nVERSION_ID="24.04"\n'),
                         {"ID": "ubuntu", "VERSION_ID": "24.04"})

    def test_package_receipt_refuses_absent_extra_or_injected_fields(self):
        valid = b'openssh-client\t1:9.6p1-3ubuntu13.99\tamd64\tinstalled\n'
        self.assertEqual(probe.parse_package(valid), ('1:9.6p1-3ubuntu13.99', 'amd64'))
        for data in (valid + b'extra\n', valid.replace(b'installed', b'not-installed'),
                     valid.replace(b'openssh-client', b'other-package'), valid.replace(b'amd64', b'amd64\tunsafe')):
            with self.subTest(data=data), self.assertRaises(probe.ProbeError):
                probe.parse_package(data)

    def test_checksum_parser_refuses_traversal_bad_digest_and_truncation(self):
        for data in (b'a' * 32 + b'  ../bin/ssh\n', b'x' * 32 + b'  usr/bin/ssh\n',
                     b'a' * 31 + b'  usr/bin/ssh\n', b'a' * 32 + b'  /usr/bin/ssh\n'):
            with self.subTest(data=data), self.assertRaises(probe.ProbeError):
                probe.parse_md5(data)

    def test_package_owner_receipt_refuses_missing_duplicate_and_other_package(self):
        lines = ['openssh-client: /usr/bin/' + name for name in probe.NAMES]
        probe.package_owners(('\n'.join(lines) + '\n').encode())
        for altered in (lines[:-1], lines[:-1] + [lines[0]], ['other: /usr/bin/ssh'] + lines[1:]):
            with self.subTest(altered=altered), self.assertRaises(probe.ProbeError):
                probe.package_owners(('\n'.join(altered) + '\n').encode())

    def test_digest_refuses_truncated_retained_object(self):
        retained = object.__new__(probe.RetainedFile)
        retained.fd, retained.identity = 7, (1, 2, 3, 4, 5, 6, 20)
        with patch.object(probe.os, 'pread', return_value=b'', create=True):
            with self.assertRaises(probe.ProbeError):
                retained.digest('sha256')

    def test_identity_change_is_refused_before_hash_or_execution(self):
        retained = object.__new__(probe.RetainedFile)
        retained.original, retained.path, retained.fd = Mock(), Path('/usr/bin/ssh-add'), 7
        retained.original.lstat.return_value = SimpleNamespace(st_uid=0)
        retained.original.resolve.return_value = retained.path
        retained.identity = (1, 2)
        with patch.object(probe, 'check_directory_chain'), patch.object(probe, 'signature', return_value=(1, 3)), \
                patch.object(probe.Path, 'lstat'), patch.object(probe.os, 'fstat'), \
                patch.object(probe.RetainedFile, 'digest') as digest:
            with self.assertRaises(probe.ProbeError):
                retained.revalidate()
        digest.assert_not_called()

    def test_execution_reference_is_checked_against_retained_object(self):
        retained = object.__new__(probe.RetainedFile)
        retained.executable, retained.fd, retained.identity = True, 7, (1, 2)
        with patch.object(probe.RetainedFile, 'revalidate'), patch.object(probe.Path, 'stat'), \
                patch.object(probe, 'signature', return_value=(1, 3)):
            with self.assertRaises(probe.ProbeError):
                retained.execution_path()

    def test_selection_does_not_search_path(self):
        provider = probe.Provider()
        with patch.object(probe, 'RetainedFile') as retained, patch.object(probe, 'parse_os_release'):
            provider.open()
        admitted = [call.args[0] for call in retained.call_args_list]
        self.assertEqual(admitted[:5], [Path('/usr/bin') / name for name in (*probe.NAMES, 'dpkg-query')])

    def test_native_exit_uncertainty_retains_all_provider_objects(self):
        first, second, provider = Mock(), Mock(), Mock()
        first.stop.side_effect = probe.ProbeError('unknown exit')
        bundle = {'agents': [first, second], 'provider': provider, 'root': Path('/tmp/private'), 'identity': ()}
        report = {}
        with patch.object(probe, 'CURRENT_REPORT', report), patch.object(probe, 'UNCERTAIN_BUNDLES', []), \
                patch.object(probe.session, 'package_backend') as package:
            with self.assertRaises(probe.ProbeError):
                probe.finish(bundle, None)
            self.assertEqual(len(probe.UNCERTAIN_BUNDLES), 1)
        second.stop.assert_called_once()
        first.close.assert_not_called()
        second.close.assert_not_called()
        provider.close.assert_not_called()
        package.assert_not_called()

    def test_primary_and_cleanup_errors_remain_separate_without_raw_text(self):
        agent = Mock()
        agent.stop.side_effect = OSError(13, 'private terminal transcript')
        report = {}
        with patch.object(probe, 'CURRENT_REPORT', report), patch.object(probe, 'UNCERTAIN_BUNDLES', []):
            probe.finish({'agents': [agent]}, probe.ProbeError('secret primary text'))
        self.assertEqual(report['primary_failure']['error_type'], 'ProbeError')
        self.assertEqual(report['cleanup_failure']['errno'], 13)
        self.assertNotIn('secret', str(report))
        self.assertNotIn('transcript', str(report))

    def test_public_key_output_must_match_exact_expected_blob(self):
        self.assertTrue(probe.keygen_matches(b'Enter passphrase:\r\nssh-ed25519 YWJj\r\n', b'abc'))
        self.assertFalse(probe.keygen_matches(b'ssh-ed25519 YWJk\r\n', b'abc'))
        self.assertFalse(probe.keygen_matches(b'ssh-ed25519 !!!\r\n', b'abc'))

    def test_executor_exec_error_is_reported_after_go_without_raw_os_message(self):
        control = Mock()
        args = SimpleNamespace(expected_parent=17, control_fd=7, executable='/proc/17/fd/9',
                               agent_socket=Path('/tmp/test.sock'), root=Path('/tmp/test'))
        with patch.object(probe.socket, 'socket', return_value=control), \
                patch.object(probe.os, 'getppid', return_value=17), \
                patch.object(probe.os, 'getpid', return_value=19), \
                patch.object(probe.native, 'child_setup'), \
                patch.object(probe.life, 'configure_parent_death', return_value='armed'), \
                patch.object(probe.life, 'receive_command', return_value=b'G'), \
                patch.object(probe.life, 'send_json') as send, \
                patch.object(probe.os, 'set_inheritable'), \
                patch.object(probe.os, 'execve', side_effect=OSError(13, 'private error text')):
            with self.assertRaises(OSError):
                probe.agent_exec(args)
        value = send.call_args.args[1]
        self.assertEqual(value['kind'], 'executor-error')
        self.assertEqual(value['diagnostic']['phase'], 'executor-exec')
        self.assertEqual(value['diagnostic']['errno'], 13)
        self.assertNotIn('private', str(value))

    def test_post_go_executor_failure_wins_over_generic_process_exit(self):
        control = Mock()
        control.recv.return_value = b'x'
        diagnostic = {'phase': 'executor-exec', 'error_type': 'PermissionError', 'errno': 13, 'code': 'os-error'}
        agent = object.__new__(probe.OwnedAgent)
        agent.process = Mock()
        agent.process.poll.return_value = 1
        with patch.object(probe.select, 'select', return_value=([control], [], [])), \
                patch.object(probe.life, 'receive_json', return_value={'kind': 'executor-error', 'diagnostic': diagnostic}):
            with self.assertRaises(probe.ExecutorFailed) as caught:
                agent.wait_ready(control)
        self.assertEqual(caught.exception.diagnostic, diagnostic)
        agent.process.poll.assert_not_called()

    def test_successful_exec_control_eof_does_not_refuse_ready_agent(self):
        control = Mock()
        control.recv.return_value = b''
        agent = object.__new__(probe.OwnedAgent)
        agent.process, agent.path = Mock(), Mock()
        agent.process.poll.return_value = None
        agent.path.exists.return_value = True
        with patch.object(probe.select, 'select', return_value=([control], [], [])), \
                patch.object(probe.native, 'identities', return_value=[]):
            agent.wait_ready(control)

    def test_unvalidated_executor_diagnostic_cannot_enter_report(self):
        diagnostic = {'phase': 'executor-exec', 'error_type': 'PermissionError', 'errno': 13, 'code': 'private transcript'}
        with patch.object(probe.life, 'receive_json', return_value={'kind': 'executor-error', 'diagnostic': diagnostic}):
            with self.assertRaises(probe.ProbeError):
                probe.receive_executor(Mock())

    def test_runtime_dispatch_uses_retained_reference_and_revalidates_on_failure(self):
        provider = probe.Provider()
        provider.files['ssh-add'] = Mock()
        provider.files['ssh-add'].execution_path.return_value = '/proc/91/fd/7'
        with patch.object(provider, 'revalidate') as validate, \
                patch.object(probe.native, 'bounded_run', side_effect=probe.ProbeError('child failed')) as run:
            with self.assertRaises(probe.ProbeError):
                provider.execute('ssh-add', ['-k', '/tmp/fixture'], {'PATH': '/tmp/poison'})
        self.assertEqual(run.call_args.args[0], ['/proc/91/fd/7', '-k', '/tmp/fixture'])
        self.assertEqual(validate.call_count, 2)


if __name__ == "__main__":
    unittest.main()
