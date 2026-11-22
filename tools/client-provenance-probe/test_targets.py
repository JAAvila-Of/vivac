"""Check published release archives as data in an isolated offline Linux lab."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


COMMIT = '874d7b0311b5c49e47382713429ab96a145de196'
ARCHIVES = {
    'aarch64-apple-darwin': ('vivac-aarch64-apple-darwin.tar.gz', '1f88eb41e093e2b06faab4e05125b45e31a07bab05c51940b9c6b6db27e363a8'),
    'aarch64-unknown-linux-musl': ('vivac-aarch64-unknown-linux-musl.tar.gz', '727d993c7a7cc99bba0f5dfd0332959187cef27acfa17ac58dc10f9954cc433b'),
    'x86_64-apple-darwin': ('vivac-x86_64-apple-darwin.tar.gz', 'ae7a946793b66729c046619b2695c8a2df0d3b8bbe222f99d9a973f65f24d54b'),
    'x86_64-pc-windows-msvc': ('vivac-x86_64-pc-windows-msvc.zip', '7da4d181fb99628cd76a295cbe203789bd73b655ba33f25352650fdf55548b38'),
    'x86_64-unknown-linux-musl': ('vivac-x86_64-unknown-linux-musl.tar.gz', 'a14d71c207022a06e14057f4ad0795e6ef3585da0aef9fdc142cecc0452b9420'),
}
GH_SHA256 = '7469124f706944133d6a169691dd1c6c3511b12e85878d255e044e2948df4c9b'
ROOTS_SHA256 = '65ca537f6ed8a47fd0e560c421baa1f6c1efb8b25fc200d8c5c02c0e92eb2b9c'
BUNDLE_SHA256 = '5a779ec89b3643ed4ad24c7890bb142e437b674304c3236d57e8c5c073aae7d9'


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(65536), b''):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets-dir', required=True, type=Path)
    parser.add_argument('--output-dir', required=True, type=Path)
    parser.add_argument('--work-dir', required=True, type=Path)
    options = parser.parse_args()
    assets = options.assets_dir.resolve(strict=True)
    output = options.output_dir.resolve(strict=True)
    work = options.work_dir.resolve(strict=True)
    if (work.stat().st_uid != os.getuid() or work.stat().st_mode & 0o777 != 0o700
            or list(work.iterdir())):
        raise RuntimeError('test work directory must be private and empty')
    expected = {name: digest for name, digest in ARCHIVES.values()}
    expected.update({'gh': GH_SHA256, 'trusted-root.jsonl': ROOTS_SHA256,
                     'bundles.jsonl': BUNDLE_SHA256})
    if any(sha256(assets / name) != digest for name, digest in expected.items()):
        raise RuntimeError('retained public fixture identity differs')
    cases = []
    environment = {'PATH': '/usr/bin:/bin', 'HOME': str(work), 'TMPDIR': str(work),
                   'PYTHONDONTWRITEBYTECODE': '1'}
    common = ['/usr/bin/python3', '-B', str(Path(__file__).with_name('verify.py')),
              '--bundle', str(assets / 'bundles.jsonl'), '--gh', str(assets / 'gh'),
              '--gh-sha256', GH_SHA256, '--roots', str(assets / 'trusted-root.jsonl'),
              '--roots-sha256', ROOTS_SHA256, '--source-commit', COMMIT]

    def run(name, archive, target, accepted):
        before_directories = {path.name for path in work.iterdir() if path.is_dir()}
        result = subprocess.run(common + ['--archive', str(archive), '--target', target],
            env=environment, cwd=work, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=40)
        (output / (name + '.stdout.json')).write_bytes(result.stdout)
        (output / (name + '.stderr.log')).write_bytes(result.stderr)
        matched = (result.returncode == 0) == accepted
        if accepted and result.returncode == 0:
            value = json.loads(result.stdout)
            archive_name, digest = ARCHIVES[target]
            matched = (value['archive_name'] == archive_name and value['sha256'] == digest
                       and value['source_commit'] == COMMIT
                       and value['repository_id'] == '1352788684'
                       and value['owner_id'] == '69764058'
                       and value['verifier_sha256'] == GH_SHA256
                       and value['roots_sha256'] == ROOTS_SHA256)
        if not accepted and result.stdout:
            matched = False
        remaining_directories = {path.name for path in work.iterdir() if path.is_dir()}
        residuals = []
        for directory in sorted(remaining_directories - before_directories):
            entries = []
            for path in (work / directory).rglob('*'):
                info = path.lstat()
                entries.append({'path': str(path.relative_to(work / directory)),
                    'mode': oct(info.st_mode), 'uid': info.st_uid, 'size': info.st_size})
            residuals.append({'directory': directory, 'entries': entries})
        cases.append({'case': name, 'actual_archive': archive.name,
                      'expected_target': target, 'expected_accept': accepted,
                      'exit_code': result.returncode, 'matched': matched,
                      'new_temporary_residuals': residuals})

    for actual, (name, _) in ARCHIVES.items():
        for target in ARCHIVES:
            run(actual + '-as-' + target, assets / name, target, actual == target)
        changed = bytearray((assets / name).read_bytes())
        changed[len(changed) // 2] ^= 1
        tampered = work / name
        tampered.write_bytes(changed)
        run(actual + '-tampered', tampered, actual, False)
    unchanged = all(sha256(assets / name) == digest for name, digest in expected.items())
    temporary_leftovers = [path.name for path in work.iterdir() if path.is_dir()]
    report = {'cases': cases, 'input_hashes': expected, 'inputs_unchanged': unchanged,
              'temporary_directories_left': temporary_leftovers,
              'candidate_extracted': False, 'candidate_executed': False}
    (output / 'cases.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'cases': len(cases), 'failed': [case['case'] for case in cases if not case['matched']],
                      'inputs_unchanged': unchanged, 'temporary_leftovers': temporary_leftovers}))
    return 0 if all(case['matched'] for case in cases) and unchanged and not temporary_leftovers else 1


if __name__ == '__main__':
    raise SystemExit(main())
