#!/usr/bin/env python3
"""Compare Cargo and nextest only after proving their default test coverage."""

import argparse
import copy
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


def normalize_path(value):
    return os.path.normcase(os.path.realpath(value))


def parse_json(value):
    def unique(pairs):
        result = {}
        for key, item in pairs:
            if key in result:
                raise ValueError('Duplicate JSON key')
            result[key] = item
        return result
    return json.loads(value, object_pairs_hook=unique)


def parse_native(value):
    names = set()
    for line in value.splitlines():
        if not line.strip():
            continue
        name, separator, kind = line.rpartition(': ')
        if not separator or kind not in ('test', 'benchmark') or not name:
            raise ValueError('Unrecognized native test inventory line')
        if name in names:
            raise ValueError('Duplicate native test name')
        names.add(name)
    return names


def compare_inventory(native, listed):
    # Schema: nextest-metadata/src/test_list.rs at cargo-nextest-0.9.114.
    suites = {}
    count = 0
    for suite in listed['rust-suites'].values():
        binary = normalize_path(suite['binary-path'])
        if binary in suites:
            raise ValueError('Duplicate nextest binary')
        if suite['status'] != 'listed':
            raise ValueError('Nextest skipped a suite')
        tests = {}
        for name, case in suite['testcases'].items():
            ignored = case['ignored']
            if not isinstance(ignored, bool):
                raise ValueError('Invalid ignored status')
            expected = ({'status': 'mismatch', 'reason': 'ignored'} if ignored
                        else {'status': 'matches'})
            if case['filter-match'] != expected:
                raise ValueError('Nextest filter changes default test coverage')
            tests[name] = ignored
        suites[binary] = tests
        count += len(tests)
    if count != listed['test-count']:
        raise ValueError('Nextest test count differs from its inventory')
    if set(suites) - set(native):
        raise ValueError('Nextest lists a binary missing from Cargo artifacts')
    for binary, tests in native.items():
        if binary not in suites:
            if tests:
                raise ValueError('Nextest omitted a nonempty Cargo test binary')
        elif tests != suites[binary]:
            raise ValueError('Cargo and nextest test names or ignored flags differ')
    return [{'binary': os.path.relpath(binary).replace(os.sep, '/'),
             'tests': [{'name': name, 'ignored': ignored}
                       for name, ignored in sorted(tests.items())]}
            for binary, tests in sorted(native.items())]


def prepare_output(output):
    if output.exists():
        if not output.is_dir() or any(output.iterdir()):
            raise ValueError('Output must be a nonexistent or empty directory')
    else:
        output.mkdir(parents=True)


class Recorder:
    def __init__(self, output, threads):
        self.output = output
        self.data = {'schema_version': 1, 'threads': threads,
                     'cpu_count': os.cpu_count(), 'os': platform.system(),
                     'os_release': platform.release(), 'arch': platform.machine(),
                     'environment': {'NO_COLOR': None, 'TERM': 'xterm-256color'},
                     'phases': [], 'inventory_complete': False,
                     'valid_comparison': False, 'errors': []}
        self.env = os.environ.copy()
        self.env.pop('NO_COLOR', None)
        self.env['TERM'] = 'xterm-256color'
        self.save()

    def save(self):
        temporary = self.output / 'results.json.tmp'
        temporary.write_text(json.dumps(self.data, indent=2) + '\n', encoding='utf-8')
        temporary.replace(self.output / 'results.json')

    def run(self, name, command, runner=None):
        phase = {'name': name, 'command': command, 'runner': runner,
                 'stdout': name + '.stdout.log', 'stderr': name + '.stderr.log',
                 'exit_code': None, 'elapsed_seconds': None, 'status': 'running'}
        self.data['phases'].append(phase)
        self.save()
        print('Starting ' + name, flush=True)
        start = time.perf_counter()
        with (self.output / phase['stdout']).open('wb') as stdout, \
                (self.output / phase['stderr']).open('wb') as stderr:
            try:
                completed = subprocess.run(command, env=self.env, stdout=stdout,
                                           stderr=stderr, check=False)
                phase['exit_code'] = completed.returncode
            except OSError:
                stderr.write(b'Unable to start command.\n')
                phase['exit_code'] = 127
            finally:
                phase['elapsed_seconds'] = time.perf_counter() - start
                phase['status'] = 'finished'
                self.save()
        print('Finished {}: exit {}, {:.3f}s'.format(
            name, phase['exit_code'], phase['elapsed_seconds']), flush=True)
        return phase

    def read(self, phase):
        return (self.output / phase['stdout']).read_text(encoding='utf-8')

    def require(self, name, command):
        phase = self.run(name, command)
        if phase['exit_code'] != 0:
            raise ValueError('Preparation phase failed: ' + name)
        return self.read(phase)


def run_rounds(recorder, threads):
    commands = {
        'cargo': ['cargo', 'test', '--locked', '--all-targets', '--no-fail-fast',
                  '--', '--test-threads=' + str(threads)],
        'nextest': ['cargo', 'nextest', 'run', '--locked', '--all-targets',
                    '--no-fail-fast', '--retries', '0', '--test-threads', str(threads)],
    }
    for number, order in enumerate((('cargo', 'nextest'), ('nextest', 'cargo')), 1):
        for runner in order:
            recorder.run('round-{}-{}'.format(number, runner), commands[runner], runner)
    recorder.data['valid_comparison'] = (
        recorder.data['inventory_complete'] and
        all(phase['exit_code'] == 0 for phase in recorder.data['phases']))
    recorder.save()


def measure(output, threads):
    if (os.environ.get('GITHUB_ACTIONS') != 'true' or
            os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'):
        raise ValueError('Measurements require a GitHub-hosted Actions runner')
    prepare_output(output)
    recorder = Recorder(output, threads)
    try:
        recorder.data['commit_sha'] = recorder.require(
            'commit', ['git', 'rev-parse', 'HEAD']).strip()
        versions = {}
        for name, command in (
                ('rustc', ['rustc', '--version']), ('cargo', ['cargo', '--version']),
                ('nextest', ['cargo', 'nextest', '--version'])):
            versions[name] = recorder.require('version-' + name, command).strip()
        recorder.data['versions'] = versions
        recorder.save()
        if versions['nextest'].split()[:2] != ['cargo-nextest', '0.9.114']:
            raise ValueError('Expected cargo-nextest 0.9.114')
        build = recorder.require('build', ['cargo', 'test', '--locked', '--all-targets',
                                 '--no-run', '--message-format=json'])
        binaries = set()
        for line in build.splitlines():
            artifact = parse_json(line)
            if (artifact.get('reason') == 'compiler-artifact' and
                    artifact.get('profile', {}).get('test') and artifact.get('executable')):
                binary = normalize_path(artifact['executable'])
                if binary in binaries:
                    raise ValueError('Duplicate Cargo test artifact')
                binaries.add(binary)
        if not binaries:
            raise ValueError('No Cargo test artifacts found')
        listed = parse_json(recorder.require('nextest-list', [
            'cargo', 'nextest', 'list', '--locked', '--all-targets', '--message-format=json']))
        native = {}
        for index, binary in enumerate(sorted(binaries), 1):
            command = os.path.relpath(binary)
            if os.sep not in command:
                command = '.' + os.sep + command
            names = parse_native(recorder.require('native-list-' + str(index),
                                 [command, '--list', '--format', 'terse']))
            ignored = parse_native(recorder.require('native-ignored-' + str(index),
                                   [command, '--ignored', '--list', '--format', 'terse']))
            if not ignored <= names:
                raise ValueError('Ignored native tests missing from full native inventory')
            native[binary] = {name: name in ignored for name in names}
        inventory = compare_inventory(native, listed)
        (output / 'inventory.json').write_text(
            json.dumps(inventory, indent=2) + '\n', encoding='utf-8')
        recorder.data['inventory_complete'] = True
        recorder.save()
        run_rounds(recorder, threads)
    except (ValueError, KeyError, TypeError, OSError) as error:
        recorder.data['errors'].append(str(error))
        recorder.save()
        print('Comparison stopped; see results.json.', file=sys.stderr)
        return 1
    return 0 if recorder.data['valid_comparison'] else 1


def self_test():
    class ComparisonTests(unittest.TestCase):
        def fixture(self):
            binary = normalize_path('target/debug/deps/example')
            native = {binary: {'works': False, 'later': True}}
            listed = {'test-count': 2, 'rust-suites': {'example': {
                'binary-path': binary, 'status': 'listed', 'testcases': {
                    'works': {'ignored': False, 'filter-match': {'status': 'matches'}},
                    'later': {'ignored': True, 'filter-match': {
                        'status': 'mismatch', 'reason': 'ignored'}},
                }}}}
            return native, listed

        def test_equal_and_empty_binary(self):
            native, listed = self.fixture()
            native[normalize_path('target/debug/deps/empty')] = {}
            self.assertEqual(len(compare_inventory(native, listed)), 2)

        def test_mismatches(self):
            for change in ('binary', 'test', 'ignored', 'filter', 'suite',
                           'missing-suite', 'count'):
                with self.subTest(change=change):
                    native, listed = self.fixture()
                    suite = listed['rust-suites']['example']
                    if change == 'binary':
                        suite['binary-path'] += '-other'
                    elif change == 'test':
                        suite['testcases']['other'] = suite['testcases'].pop('works')
                    elif change == 'ignored':
                        suite['testcases']['works']['ignored'] = True
                    elif change == 'filter':
                        suite['testcases']['works']['filter-match'] = {
                            'status': 'mismatch', 'reason': 'default-filter'}
                    elif change == 'suite':
                        suite['status'] = 'skipped-default-filter'
                    elif change == 'missing-suite':
                        listed['rust-suites'].clear()
                        listed['test-count'] = 0
                    else:
                        listed['test-count'] = 3
                    with self.assertRaises(ValueError):
                        compare_inventory(native, listed)

        def test_duplicates(self):
            with self.assertRaises(ValueError):
                parse_json('{"test":1,"test":2}')
            with self.assertRaises(ValueError):
                parse_native('same: test\nsame: test\n')
            native, listed = self.fixture()
            listed['rust-suites']['duplicate'] = copy.deepcopy(
                listed['rust-suites']['example'])
            with self.assertRaises(ValueError):
                compare_inventory(native, listed)

        def test_failure_does_not_repeat_or_stop_rounds(self):
            with tempfile.TemporaryDirectory() as temporary:
                recorder = Recorder(Path(temporary), 2)
                exits = iter([9, 0, 0, 0])

                def fake_run(command, **kwargs):
                    kwargs['stdout'].write(b'fixture output\n')
                    return subprocess.CompletedProcess(command, next(exits))

                with patch('subprocess.run', side_effect=fake_run) as run:
                    run_rounds(recorder, 2)
                self.assertEqual(run.call_count, 4)
                self.assertEqual([phase['exit_code'] for phase in recorder.data['phases']],
                                 [9, 0, 0, 0])
                self.assertEqual([phase['runner'] for phase in recorder.data['phases']],
                                 ['cargo', 'nextest', 'nextest', 'cargo'])
                self.assertFalse(recorder.data['valid_comparison'])
                self.assertEqual((Path(temporary) / 'round-1-cargo.stdout.log').read_text(),
                                 'fixture output\n')

        def test_output_never_overwrites(self):
            with tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / 'output'
                prepare_output(output)
                retained = output / 'retained'
                retained.write_text('original')
                with self.assertRaises(ValueError):
                    prepare_output(output)
                self.assertEqual(retained.read_text(), 'original')

        def test_hosted_guard_starts_no_commands(self):
            for environment in ({}, {'GITHUB_ACTIONS': 'true'},
                                {'RUNNER_ENVIRONMENT': 'github-hosted'},
                                {'GITHUB_ACTIONS': 'true',
                                 'RUNNER_ENVIRONMENT': 'self-hosted'}):
                with self.subTest(environment=environment):
                    with patch.dict(os.environ, environment, clear=True), \
                            patch('subprocess.run') as run:
                        with self.assertRaises(ValueError):
                            measure(Path('unused-output'), 1)
                        run.assert_not_called()

        def test_complete_measurement_without_rust(self):
            for failed_run in (False, True):
                with self.subTest(failed_run=failed_run), \
                        tempfile.TemporaryDirectory() as temporary:
                    native, listed = self.fixture()
                    binary = next(iter(native))
                    artifact = {'reason': 'compiler-artifact',
                                'profile': {'test': True}, 'executable': binary}
                    calls = []

                    def fake_run(command, **kwargs):
                        self.assertNotIn('NO_COLOR', kwargs['env'])
                        self.assertEqual(kwargs['env']['TERM'], 'xterm-256color')
                        calls.append(command)
                        code = 0
                        if command[:2] == ['git', 'rev-parse']:
                            output = '0123456789abcdef\n'
                        elif command == ['rustc', '--version']:
                            output = 'rustc fixture\n'
                        elif command == ['cargo', '--version']:
                            output = 'cargo fixture\n'
                        elif command == ['cargo', 'nextest', '--version']:
                            output = 'cargo-nextest 0.9.114\n'
                        elif '--no-run' in command:
                            output = json.dumps(artifact) + '\n'
                        elif command[:3] == ['cargo', 'nextest', 'list']:
                            output = json.dumps(listed)
                        elif '--list' in command:
                            output = ('later: test\n' if '--ignored' in command else
                                      'works: test\nlater: test\n')
                        else:
                            output = 'fixture execution\n'
                            if failed_run and command[:2] == ['cargo', 'test']:
                                code = 7
                        kwargs['stdout'].write(output.encode('utf-8'))
                        return subprocess.CompletedProcess(command, code)

                    output = Path(temporary) / 'output'
                    with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true',
                                    'RUNNER_ENVIRONMENT': 'github-hosted',
                                    'NO_COLOR': '1'}), \
                            patch('subprocess.run', side_effect=fake_run):
                        self.assertEqual(measure(output, 2), int(failed_run))
                    result = json.loads((output / 'results.json').read_text())
                    self.assertTrue(result['inventory_complete'])
                    self.assertEqual(result['valid_comparison'], not failed_run)
                    measured = [phase for phase in result['phases'] if phase['runner']]
                    self.assertEqual(len(measured), 4)
                    self.assertTrue((output / 'inventory.json').exists())

    result = unittest.TextTestRunner(verbosity=2).run(
        unittest.defaultTestLoader.loadTestsFromTestCase(ComparisonTests))
    return 0 if result.wasSuccessful() else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--threads', type=int, default=os.cpu_count() or 1)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.threads <= 0:
        parser.error('--threads must be positive')
    if args.self_test:
        return self_test()
    if args.output is None:
        parser.error('--output is required for measurements')
    try:
        return measure(args.output, args.threads)
    except (ValueError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
