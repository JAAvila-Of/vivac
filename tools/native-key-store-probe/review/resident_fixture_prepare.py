"""Prepare public encrypted test data and compare writer-check behavior."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--mode',choices=['baseline','regression'],required=True)
parser.add_argument('--writer',type=Path,required=True)
parser.add_argument('--root',type=Path,required=True)
args = parser.parse_args()
assert os.name == 'posix' and Path('/heap-build').is_mount()
assert args.root == Path('/heap-build/resident-fixture')
assert args.writer == Path('/heap-build/writer189/debug/ssh-writer-graph-audit')
environment = dict(os.environ,VIVAC_OPENSSH_DISPOSABLE='1')
cases = ['valid','unknown-field','duplicate-field','wrong-hash','wrong-public','malformed-key','empty-key']

def invoke(arguments, data=None):
    value = subprocess.run([str(args.writer),*arguments],input=data,stdin=subprocess.DEVNULL if data is None else None,
        capture_output=True,env=environment,timeout=120)
    assert len(value.stdout) <= 4096 and len(value.stderr) <= 4096
    return dict(exit_code=value.returncode,stdout_hex=value.stdout.hex(),stderr_hex=value.stderr.hex())

def write(path,data):
    descriptor = os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_CLOEXEC,0o600)
    with os.fdopen(descriptor,'wb') as output: output.write(data)

if args.mode == 'baseline':
    args.root.mkdir(mode=0o700)
    comment = 'vivac.device-key.v1:00000000000000000000000003:00000000000000000000000004'
    password = b'public-resident-reader-fixture-secret'
    created = invoke(['--package-create',str(args.root/'valid'),comment],password+b'\n'+password+b'\n')
    assert created == dict(exit_code=0,stdout_hex='',stderr_hex=''),created
    key = (args.root/'valid/key.openssh').read_bytes()
    manifest = (args.root/'valid/manifest.json').read_bytes()
    for name in cases[1:]:
        directory = args.root/name;directory.mkdir(mode=0o700)
        changed_key = key; changed_manifest = manifest
        value = json.loads(manifest)
        if name == 'unknown-field': value['approved']=True
        elif name == 'duplicate-field': changed_manifest=b'{"version":1,'+manifest[1:]
        elif name == 'wrong-hash': value['key_sha256']='00'*32
        elif name == 'wrong-public': value['public_key']='00'*32
        elif name == 'malformed-key': changed_key=b'public malformed test input'
        elif name == 'empty-key': changed_key=b''
        if name in {'unknown-field','wrong-hash','wrong-public'}:
            changed_manifest=json.dumps(value,separators=(',',':')).encode('ascii')
        write(directory/'key.openssh',changed_key);write(directory/'manifest.json',changed_manifest)
    baseline = {name:invoke(['--package-check',str(args.root/name)]) for name in cases}
    assert baseline['valid']['exit_code'] == 0
    assert all(baseline[name]['exit_code'] == 1 for name in cases[1:])
    write(args.root/'baseline.json',json.dumps(baseline,sort_keys=True).encode('ascii'))
else:
    baseline=json.loads((args.root/'baseline.json').read_bytes())
    actual={name:invoke(['--package-check',str(args.root/name)]) for name in cases}
    assert actual == baseline,'Writer check behavior changed'
    write(args.root/'regression.json',json.dumps(actual,sort_keys=True).encode('ascii'))
print(json.dumps(dict(status='observed',mode=args.mode,cases=len(cases),key_sha256=hashlib.sha256((args.root/'valid/key.openssh').read_bytes()).hexdigest())))
