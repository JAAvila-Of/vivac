"""Test the automated verifier only inside an owned offline container."""
import hashlib
import argparse
import copy
import base64
import json
import os
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--assets-dir', required=True, type=Path)
parser.add_argument('--output-dir', required=True, type=Path)
parser.add_argument('--work-dir', required=True, type=Path)
options = parser.parse_args()
data = options.work_dir.resolve(strict=True)
os.chdir(data)
if data.stat().st_uid != os.getuid() or data.stat().st_mode & 0o777 != 0o700:
    raise RuntimeError('private data directory differs')
assets = options.assets_dir.resolve(strict=True)
out = options.output_dir.resolve(strict=True)
command = ['/usr/bin/python3', '-B', str(Path(__file__).with_name('verify.py')),
    '--archive', str(assets / 'vivac-x86_64-unknown-linux-musl.tar.gz'),
    '--bundle', str(assets / 'bundles.jsonl'), '--gh', str(assets / 'gh'),
    '--gh-sha256', '7469124f706944133d6a169691dd1c6c3511b12e85878d255e044e2948df4c9b',
    '--roots', str(assets / 'trusted-root.jsonl'),
    '--roots-sha256', '65ca537f6ed8a47fd0e560c421baa1f6c1efb8b25fc200d8c5c02c0e92eb2b9c',
    '--source-commit', '874d7b0311b5c49e47382713429ab96a145de196',
    '--target', 'x86_64-unknown-linux-musl']
environment = {'PATH': '/usr/bin:/bin', 'HOME': str(data), 'TMPDIR': str(data),
               'PYTHONDONTWRITEBYTECODE': '1', 'GH_TOKEN': 'disposable-sentinel',
               'HTTPS_PROXY': 'http://127.0.0.1:1', 'GH_TELEMETRY': 'enabled',
               'DO_NOT_TRACK': 'false'}
initial_hashes = {name: hashlib.sha256((assets / name).read_bytes()).hexdigest()
                  for name in ('gh', 'trusted-root.jsonl', 'bundles.jsonl',
                               'vivac-x86_64-unknown-linux-musl.tar.gz')}
cases = []
def replace(flag, value, base=None):
    args = (command if base is None else base).copy()
    args[args.index(flag) + 1] = str(value)
    return args
def run(name, args, accepted=False, category='real-verification'):
    start = time.monotonic()
    result = subprocess.run(args, env=environment, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=45)
    (out / (name + '.stdout.json')).write_bytes(result.stdout)
    (out / (name + '.stderr.log')).write_bytes(result.stderr)
    matched = (result.returncode == 0) == accepted and (accepted or not result.stdout)
    cases.append({'case': name, 'category': category, 'exit_code': result.returncode,
                  'seconds': time.monotonic() - start, 'matched': matched})
    return result

positive = run('real-valid-cli', command, True)
if positive.returncode:
    (out / 'cases.json').write_text(json.dumps({'cases': cases}, indent=2) + '\n')
    raise SystemExit(1)
receipt = json.loads(positive.stdout)
receipt_expected = {
    'type': 'client-release-provenance-probe-v1',
    'archive_name': 'vivac-x86_64-unknown-linux-musl.tar.gz',
    'sha256': initial_hashes['vivac-x86_64-unknown-linux-musl.tar.gz'],
    'source_commit': '874d7b0311b5c49e47382713429ab96a145de196',
    'repository_id': '1352788684', 'owner_id': '69764058',
    'workflow': 'https://github.com/JAAvila-Of/vivac/.github/workflows/release.yml@refs/heads/main',
    'verifier_sha256': initial_hashes['gh'], 'roots_sha256': initial_hashes['trusted-root.jsonl']}
cases.append({'case': 'real-receipt-exact', 'category': 'output-contract',
              'matched': receipt == receipt_expected})
run('real-wrong-commit', replace('--source-commit', '0' * 40))
run('real-wrong-target', replace('--target', 'x86_64-pc-windows-msvc'))
tampered = data / 'tampered.tar.gz'
content = bytearray((assets / 'vivac-x86_64-unknown-linux-musl.tar.gz').read_bytes())
content[len(content) // 2] ^= 1
tampered.write_bytes(content)
run('real-tampered-archive', replace('--archive', tampered))
bundle = json.loads((assets / 'bundles.jsonl').read_text().strip())
signature = bytearray(base64.b64decode(bundle['dsseEnvelope']['signatures'][0]['sig']))
signature[-1] ^= 1
bundle['dsseEnvelope']['signatures'][0]['sig'] = base64.b64encode(signature).decode()
(data / 'tampered-bundle').write_text(json.dumps(bundle) + '\n')
run('real-tampered-signature', replace('--bundle', data / 'tampered-bundle'))
run('wrong-verifier-hash', replace('--gh-sha256', '0' * 64), category='input-guard')
run('wrong-roots-hash', replace('--roots-sha256', '0' * 64), category='input-guard')
for flag, name in (('--archive', 'vivac-x86_64-unknown-linux-musl.tar.gz'),
                   ('--bundle', 'bundles.jsonl'), ('--gh', 'gh'), ('--roots', 'trusted-root.jsonl')):
    symlink = data / ('symlink-' + flag[2:])
    symlink.symlink_to(assets / name)
    run('refuse-' + flag[2:] + '-symlink', replace(flag, symlink), category='input-guard')
local = data / 'local-archive'
local.write_bytes((assets / 'vivac-x86_64-unknown-linux-musl.tar.gz').read_bytes())
os.link(local, data / 'hardlink-archive')
run('refuse-archive-hardlink', replace('--archive', local), category='input-guard')
os.mkfifo(data / 'archive-fifo', 0o600)
run('refuse-archive-fifo', replace('--archive', data / 'archive-fifo'), category='input-guard')
with (data / 'oversize-archive').open('wb') as stream:
    stream.truncate(128 * 1024 * 1024 + 1)
run('refuse-oversize-archive', replace('--archive', data / 'oversize-archive'), category='input-guard')
run('refuse-malformed-commit', replace('--source-commit', 'not-a-commit'), category='input-guard')

# These are deliberately trusted fake verifiers for local branch tests, not signatures.
workflow = 'https://github.com/JAAvila-Of/vivac/.github/workflows/release.yml@refs/heads/main'
certificate = {'sourceRepositoryIdentifier': '1352788684', 'sourceRepositoryOwnerIdentifier': '69764058',
    'sourceRepositoryURI': 'https://github.com/JAAvila-Of/vivac',
    'sourceRepositoryDigest': '874d7b0311b5c49e47382713429ab96a145de196',
    'sourceRepositoryRef': 'refs/heads/main', 'subjectAlternativeName': workflow,
    'issuer': 'https://token.actions.githubusercontent.com', 'runnerEnvironment': 'github-hosted'}
value = [{'verificationResult': {'signature': {'certificate': certificate},
    'statement': {'predicateType': 'https://slsa.dev/provenance/v1', 'subject': [
        {'name': 'vivac-x86_64-unknown-linux-musl.tar.gz', 'digest': {'sha256': initial_hashes['vivac-x86_64-unknown-linux-musl.tar.gz']}}]},
    'verifiedTimestamps': [{'type': 'Tlog', 'timestamp': '2026-11-04T03:14:29Z'}]}}]
def fake(name, text, version='gh version 2.102.0 (2026-09-30)', behavior=''):
    path = data / ('fake-' + name)
    script = ('#!/usr/bin/python3\nimport os, sys, time\n'
        'if os.environ.get("GH_TELEMETRY") != "false" or os.environ.get("DO_NOT_TRACK") != "true": raise SystemExit(72)\n'
        'if sys.argv[1:] == ["--version"]:\n'
        + ('    time.sleep(15)\n' if behavior == 'version-timeout' else '')
        + ('    import subprocess\n    child = subprocess.Popen(["/usr/bin/python3", "-c", "import time;time.sleep(60)"])\n    open(' + repr(str(data / 'descendant.pid')) + ',"w").write(str(child.pid))\n' if behavior == 'version-descendant' else '')
        + '    print(' + repr(version) + '); raise SystemExit(0)\n'
        + 'if "GH_TOKEN" in os.environ or "HTTPS_PROXY" in os.environ: raise SystemExit(70)\n'
        + 'if not os.environ["HOME"].startswith(' + repr(str(data) + '/') + '): raise SystemExit(71)\n'
        + ('os.write(1,b"x" * (3 * 1024 * 1024)); raise SystemExit(0)\n' if behavior == 'stdout-flood' else '')
        + ('os.write(2,b"x" * (128 * 1024)); raise SystemExit(0)\n' if behavior == 'stderr-flood' else '')
        + ('os.chmod(sys.argv[3],0o600); open(sys.argv[3],"wb").write(b"changed")\n' if behavior == 'mutate-archive' else '')
        + ('time.sleep(40)\n' if behavior == 'verify-timeout' else '')
        + 'print(' + repr(text) + ')\n'
        + ('raise SystemExit(9)\n' if behavior == 'failed-process' else ''))
    path.write_text(script)
    path.chmod(0o700)
    return replace('--gh-sha256', hashlib.sha256(path.read_bytes()).hexdigest(), replace('--gh', path))

run('mock-trusted-verifier-control', fake('control', json.dumps(value)), True, 'mock-process-policy')
run('mock-failed-process-valid-json', fake('failed', json.dumps(value), behavior='failed-process'), category='mock-process-policy')
run('mock-old-version', fake('old', json.dumps(value), version='gh version 2.101.0 (2026-09-29)'), category='mock-process-policy')
run('mock-prerelease-version', fake('prerelease', json.dumps(value), version='gh version 2.102.0-rc1'), category='mock-process-policy')
run('mock-bad-json', fake('bad-json', '{'), category='mock-process-policy')
run('mock-empty-array', fake('empty', '[]'), category='mock-process-policy')
run('mock-duplicate-json', fake('duplicate', '[{"verificationResult":{},"verificationResult":{}}]'), category='mock-process-policy')
run('mock-nan-json', fake('nan', '[NaN]'), category='mock-process-policy')
run('mock-unknown-major', fake('major3', json.dumps(value), version='gh version 3.0.0'), category='mock-process-policy')
run('mock-too-many-results', fake('many', json.dumps(value * 17)), category='mock-process-policy')
for stamp_label, stamp in (('empty', []), ('null', [None]), ('empty-object', [{}]),
                           ('unstructured', [{'unexpected': 'value'}]),
                           ('bad-date', [{'type': 'Tlog', 'timestamp': 'not-a-date'}]),
                           ('empty-type', [{'type': '', 'timestamp': '2026-11-04T03:14:29Z'}]),
                           ('missing-zone', [{'type': 'Tlog', 'timestamp': '2026-11-04T03:14:29'}]),
                           ('invalid-day', [{'type': 'Tlog', 'timestamp': '2026-02-30T03:14:29Z'}])):
    changed = copy.deepcopy(value)
    changed[0]['verificationResult']['verifiedTimestamps'] = stamp
    run('mock-timestamps-' + stamp_label, fake('timestamps-' + stamp_label, json.dumps(changed)), category='mock-process-policy')
changed = copy.deepcopy(value)
changed[0]['verificationResult']['verifiedTimestamps'] = [{'type': 'Tlog', 'timestamp': '2026-11-04T03:14:29.123+00:00'}]
run('mock-timestamp-fraction-offset', fake('fraction-offset', json.dumps(changed)), True, 'mock-process-policy')
changed = [copy.deepcopy(value[0]), copy.deepcopy(value[0])]
changed[0]['verificationResult']['statement']['subject'][0]['name'] = 'wrong-target'
changed[1]['verificationResult']['signature']['certificate']['sourceRepositoryIdentifier'] = 'wrong'
run('mock-split-evidence', fake('split', json.dumps(changed)), category='mock-process-policy')
for key in ('sourceRepositoryIdentifier', 'sourceRepositoryOwnerIdentifier', 'subjectAlternativeName'):
    changed = copy.deepcopy(value)
    changed[0]['verificationResult']['signature']['certificate'][key] = 'wrong'
    run('mock-wrong-' + key, fake(key, json.dumps(changed)), category='mock-process-policy')
for behavior in ('stdout-flood', 'stderr-flood', 'version-timeout', 'mutate-archive', 'version-descendant', 'verify-timeout'):
    run('mock-' + behavior, fake(behavior, json.dumps(value), behavior=behavior), category='mock-process-policy')
child_pid = int((data / 'descendant.pid').read_text())
child_status = Path('/proc') / str(child_pid) / 'stat'
child_state = child_status.read_text().split(') ', 1)[1].split()[0] if child_status.exists() else None
cases.append({'case': 'mock-descendant-terminated', 'category': 'process-cleanup',
              'matched': child_state in (None, 'Z'), 'state': child_state})
unchanged = all(hashlib.sha256((assets / name).read_bytes()).hexdigest() == digest
                for name, digest in initial_hashes.items())
temporary_leftovers = [str(path.name) for path in data.iterdir() if path.is_dir()]
report = {'cases': cases, 'input_hashes': initial_hashes, 'inputs_unchanged': unchanged,
    'temporary_directories_left': temporary_leftovers, 'receipt': receipt}
(out / 'cases.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'cases': len(cases), 'failed': [case['case'] for case in cases if not case['matched']],
                  'inputs_unchanged': unchanged, 'temporary_leftovers': temporary_leftovers}))
raise SystemExit(0 if all(case['matched'] for case in cases) and unchanged and not temporary_leftovers else 1)
