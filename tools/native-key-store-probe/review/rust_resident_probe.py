"""Observe a candidate resident snapshot using public disposable fixtures."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import stat
import struct
import subprocess
import sys
import time
import uuid

import rust_fixed_root_probe as fixed

abandon, life, launcher, provision, reader, session = fixed.abandon, fixed.life, fixed.launcher, fixed.provision, fixed.reader, fixed.session
require = fixed.require
SCOPE_FIELDS = fixed.SCOPE_FIELDS
COMMON = fixed.COMMON
READER_FIELDS = fixed.READER_FIELDS | {'selected_credential_id'}
DEVICE = '00000000000000000000000003'
CRED = '00000000000000000000000004'
CREATION = '00000000000000000000000002'
OTHER = '00000000000000000000000005'
STATE = fixed.REAL / 'custody-v1/users/65534/state'
PACKAGE = 'packages/'+CRED
RECORD = 'admissions/'+CRED
PATHS = (PACKAGE,PACKAGE+'/key.openssh',PACKAGE+'/manifest.json',RECORD,RECORD+'/record.json')
MOUNT_PATHS = {'mount-record-slot':RECORD,'mount-record-leaf':RECORD+'/record.json',
    'mount-package-slot':PACKAGE,'mount-key-leaf':PACKAGE+'/key.openssh','mount-manifest-leaf':PACKAGE+'/manifest.json'}
MOUNT_PATHS = {case:'custody-v1/users/65534/state/'+path for case,path in MOUNT_PATHS.items()}
INVALIDATIONS = {'record-in-place','record-replacement','package-replacement','key-in-place','key-replacement','manifest-replacement'}
MUTATIONS = INVALIDATIONS | {'resident-unrelated'}
NEGATIVES = {'missing-record','missing-package','experimental-record','installation-mismatch','credential-mismatch',
    'manifest-mismatch','ciphertext-mismatch','symlink-record','hardlink-key','acl-record'} | set(MOUNT_PATHS)
CASES = MUTATIONS | NEGATIVES | {'resident-valid'}
METADATA = fixed.METADATA | {'kind','acl'}
EXTERNAL = fixed.EXTERNAL | {'before_after','mount_evidence'}
hexadecimal = fixed.hexadecimal
UNCERTAIN = []


def initial_error(case):
    return ('not-found' if case in {'missing-record','missing-package'} else 'crosses-devices' if case in MOUNT_PATHS
            else 'symlink' if case == 'symlink-record' else 'invalid-data' if case in NEGATIVES else 'none')


def record_value(iid, manifest, key, raw):
    return dict(version=2,installation_id=iid,creation_id=CREATION,device_id=DEVICE,credential_id=CRED,
        public_key=manifest['public_key'],ciphertext_sha256=hashlib.sha256(key).hexdigest(),manifest_sha256=hashlib.sha256(raw).hexdigest(),
        ciphertext_len=len(key),manifest_len=len(raw),creation_profile_revision=1,secret_policy_revision=1)


def encoded(value):
    return json.dumps(value,separators=(',',':'),ensure_ascii=True).encode('ascii')


def bounded_file(path, limit):
    fd = os.open(path,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size <= limit)
        data = os.read(fd,limit+1)
        require(len(data) == info.st_size)
        return data
    finally: os.close(fd)


def fixture_bytes():
    package = session.package_backend()
    require(set(os.listdir('/fixture')) == {'key.openssh','manifest.json'})
    key = bounded_file(Path('/fixture/key.openssh'),65536)
    raw = bounded_file(Path('/fixture/manifest.json'),2048)
    manifest,_ = package.read_manifest_bytes(raw)
    parsed = package.native.parse_key_bytes(key)
    require(manifest['device_id'] == DEVICE and manifest['credential_id'] == CRED
        and manifest['key_size'] == len(key) and manifest['key_sha256'] == hashlib.sha256(key).hexdigest()
        and manifest['public_key'] == parsed['public_key'].hex() and parsed['cipher'] == 'aes256-gcm@openssh.com'
        and parsed['kdf'] == 'bcrypt' and parsed['rounds'] == 64)
    return key,raw,manifest


def metadata(path):
    info = path.lstat()
    kind = 'directory' if stat.S_ISDIR(info.st_mode) else 'file' if stat.S_ISREG(info.st_mode) else 'symlink'
    require(kind != 'symlink' or stat.S_ISLNK(info.st_mode))
    digest = None
    if kind == 'file':
        fd = os.open(path,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            current = os.fstat(fd)
            require((current.st_dev,current.st_ino) == (info.st_dev,info.st_ino) and 0 < current.st_size <= 65536)
            data = os.read(fd,65537); require(len(data) == info.st_size)
            digest = hashlib.sha256(data).hexdigest()
        finally: os.close(fd)
    acl = bool(session.package_backend().acl_xattrs(path))
    return dict(device=info.st_dev,inode=info.st_ino,uid=info.st_uid,gid=info.st_gid,mode=stat.S_IMODE(info.st_mode),
        nlink=info.st_nlink,size=info.st_size,sha256=digest,kind=kind,acl=acl)


def evidence():
    result = {}
    for relative in PATHS:
        try: result[relative] = metadata(STATE/relative)
        except FileNotFoundError: result[relative] = None
    return result


def write_file(path, data):
    fd = os.open(path,os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,0o600)
    try:
        os.fchmod(fd,0o600); require(os.write(fd,data) == len(data)); os.fsync(fd)
    finally: os.close(fd)


def populate(case, iid):
    key,raw,manifest = fixture_bytes()
    value = record_value(iid,manifest,key,raw)
    if case == 'experimental-record': value['version'] = 1
    elif case == 'installation-mismatch': value['installation_id'] = OTHER if iid != OTHER else CREATION
    elif case == 'credential-mismatch': value['credential_id'] = OTHER
    elif case == 'manifest-mismatch': value['device_id'] = OTHER
    elif case == 'ciphertext-mismatch': value['ciphertext_sha256'] = '0'*64 if value['ciphertext_sha256'] != '0'*64 else '1'*64
    parents = [STATE/name for name in ('packages','admissions')]
    for parent in parents:
        info = parent.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == info.st_gid == 65534 and stat.S_IMODE(info.st_mode) == 0o700
            and not session.package_backend().acl_xattrs(parent) and os.listdir(parent) == [])
        os.chown(parent,0,0,follow_symlinks=False)
    created = []
    try:
        if case != 'missing-package':
            slot = STATE/PACKAGE; slot.mkdir(mode=0o700); os.chmod(slot,0o700); created.append(slot)
            for name,data in (('key.openssh',key),('manifest.json',raw)):
                path = slot/name; write_file(path,data); created.append(path)
        if case != 'missing-record':
            slot = STATE/RECORD; slot.mkdir(mode=0o700); os.chmod(slot,0o700); created.append(slot)
            path = slot/'record.json'
            if case == 'symlink-record':
                controlled = STATE/'admissions/record-copy.json'; write_file(controlled,encoded(value)); created.append(controlled)
                os.symlink('../record-copy.json',path); created.append(path)
            else: write_file(path,encoded(value)); created.append(path)
        if case == 'hardlink-key': os.link(STATE/PACKAGE/'key.openssh',STATE/'packages/key-alias.openssh')
        if case == 'acl-record':
            payload = struct.pack('<I',2)
            for tag,permissions,identifier in ((1,6,0xffffffff),(2,0,0),(4,0,0xffffffff),(16,0,0xffffffff),(32,0,0xffffffff)):
                payload += struct.pack('<HHI',tag,permissions,identifier)
            os.setxattr(STATE/RECORD/'record.json','system.posix_acl_access',payload,follow_symlinks=False)
        for path in reversed(created): os.chown(path,65534,65534,follow_symlinks=False)
    finally:
        for parent in parents: os.chown(parent,65534,65534,follow_symlinks=False)
    observed = evidence(); validate_evidence(observed,case)
    for relative,names in ((PACKAGE,{'key.openssh','manifest.json'}),(RECORD,{'record.json'})):
        if observed[relative] is not None: require(set(os.listdir(STATE/relative)) == names)
    for parent in parents:
        item = metadata(parent)
        require(item['uid'] == item['gid'] == 65534 and item['mode'] == 0o700 and not item['acl'])
    for relative,data in ((PACKAGE+'/key.openssh',key),(PACKAGE+'/manifest.json',raw),(RECORD+'/record.json',encoded(value))):
        if observed[relative] is not None and observed[relative]['kind'] == 'file':
            require(observed[relative]['sha256'] == hashlib.sha256(data).hexdigest() and observed[relative]['size'] == len(data))
    return observed


def validate_evidence(value, case):
    require(type(value) is dict and set(value) == set(PATHS))
    for relative,item in value.items():
        missing = case == 'missing-package' and relative.startswith(PACKAGE) or case == 'missing-record' and relative.startswith(RECORD)
        if missing: require(item is None); continue
        require(type(item) is dict and set(item) == METADATA)
        require(all(type(item[field]) is int for field in fixed.METADATA-{'sha256'}))
        directory = relative in {PACKAGE,RECORD}
        require(item['device'] >= 0 and item['inode'] > 0 and item['uid'] == item['gid'] == 65534
            and item['mode'] == (0o700 if directory else 0o777 if case == 'symlink-record' and relative.endswith('record.json') else 0o600))
        require(item['kind'] == ('directory' if directory else 'symlink' if case == 'symlink-record' and relative.endswith('record.json') else 'file'))
        require(item['nlink'] == (2 if directory or case == 'hardlink-key' and relative.endswith('key.openssh') else 1))
        require(item['acl'] is (case == 'acl-record' and relative.endswith('record.json')))
        require(item['sha256'] is None if directory or item['kind'] == 'symlink' else hexadecimal(item['sha256'],64))


def validate_result(value, case, identity=None, *, observation=False, held=False):
    require(type(value) is dict and set(value) == READER_FIELDS and case in CASES)
    require(value['case'] == case and value['status'] == 'observed')
    reader.validate_identity(value['identity'],identity)
    require(all(value[field] is False for field in SCOPE_FIELDS))
    for field,expected in {'version':1,'pressure_threads':4}.items(): require(type(value[field]) is int and value[field] == expected)
    require(type(value['pressure_iterations']) is int and 0 <= value['pressure_iterations'] <= 0xffffffffffffffff)
    require(value['pressure_iterations'] == 0 if observation else value['pressure_iterations'] > 0)
    require(value['resources_closed_after_exit'] is (not observation))
    opened = case not in NEGATIVES
    require(value['initial_opened'] is opened and value['initial_validated'] is opened and value['initial_error'] == initial_error(case))
    require(fixed.installation(value['selected_installation_id']) if opened else value['selected_installation_id'] is None)
    require(value['selected_credential_id'] == CRED if opened else value['selected_credential_id'] is None)
    invalid = case in INVALIDATIONS and not held
    require(value['snapshot_invalidated'] is invalid and value['validation_error'] == ('invalid-data' if invalid else 'none'))


def validate_request(value, case):
    require(type(value) is dict and set(value) == {'version','case','request_token','reader_host_pid','reader_starttime'})
    require(case in MUTATIONS and value['case'] == case and type(value['version']) is int and value['version'] == 1)
    require(hexadecimal(value['request_token'],32))
    for field in ('reader_host_pid','reader_starttime'): require(type(value[field]) is int and 0 < value[field] <= 0xffffffffffffffff)


def validate_limits(value, phase):
    fixed.validate_limits(value,phase)
    memory,pids,cpu,tmp = {'setup':(512,128,1,128),'reader':(320,96,.75,96),'mutate':(128,16,.25,16)}[phase]
    require(value == dict(memory_bytes=memory*1048576,swap_bytes=0,pids=pids,cpu_quota=int(cpu*100000),cpu_period=100000,temporary_bytes=tmp*1048576))


def validate_mutation(value, case, token):
    require(type(value) is dict and set(value) == {'version','case','request_token','status','before','after','unrelated_created'})
    require(type(value['version']) is int and value['version'] == 1 and value['case'] == case and case in MUTATIONS
        and hexadecimal(token,32) and value['request_token'] == token and value['status'] == 'observed')
    before,after = value['before'],value['after']
    validate_evidence(before,'resident-valid'); validate_evidence(after,'resident-valid')
    require(value['unrelated_created'] is (case == 'resident-unrelated'))
    if case == 'resident-unrelated': require(before == after); return
    target = RECORD+'/record.json' if case.startswith('record-') else PACKAGE+'/key.openssh' if case.startswith('key-') else PACKAGE+'/manifest.json' if case == 'manifest-replacement' else PACKAGE
    changed_paths = {target} if case != 'package-replacement' else {PACKAGE,PACKAGE+'/key.openssh',PACKAGE+'/manifest.json'}
    for path in PATHS:
        if path not in changed_paths: require(before[path] == after[path]); continue
        changed = {'sha256'} if case.endswith('in-place') else {'inode'}
        require(all(before[path][key] == after[path][key] for key in METADATA-changed))
        require(all(before[path][key] != after[path][key] for key in changed))


def mutate(case, token):
    require(case in MUTATIONS and hexadecimal(token,32))
    before = evidence(); validate_evidence(before,'resident-valid')
    if case == 'resident-unrelated':
        key,raw,manifest = fixture_bytes(); manifest['credential_id'] = OTHER; raw = encoded(manifest)
        iid,_ = fixed.binding_evidence(fixed.REAL)
        value = record_value(iid,manifest,key,raw); value['credential_id'] = OTHER
        for parent,files in (('packages',{'key.openssh':key,'manifest.json':raw}),('admissions',{'record.json':encoded(value)})):
            slot = STATE/parent/OTHER; slot.mkdir(mode=0o700); os.chmod(slot,0o700)
            for name,data in files.items(): write_file(slot/name,data)
            require(set(os.listdir(slot)) == set(files))
            for path in [slot]+[slot/name for name in files]:
                item = metadata(path); require(item['uid'] == item['gid'] == 65534 and item['mode'] == (0o700 if path == slot else 0o600) and not item['acl'])
    else:
        target = STATE/(RECORD+'/record.json' if case.startswith('record-') else PACKAGE+'/key.openssh' if case.startswith('key-') else PACKAGE+'/manifest.json' if case == 'manifest-replacement' else PACKAGE)
        if case == 'package-replacement':
            temporary = target.parent/('.resident-'+token); temporary.mkdir(mode=0o700); os.chmod(temporary,0o700)
            for name in ('key.openssh','manifest.json'): write_file(temporary/name,bounded_file(target/name,65536))
            old = target.parent/('.old-'+token); os.rename(target,old); os.rename(temporary,target)
        else:
            data = bounded_file(target,65536)
            if case.endswith('in-place'):
                if case == 'record-in-place':
                    value = abandon.decode(data); require(value['creation_id'] == CREATION); value['creation_id'] = OTHER; changed = encoded(value)
                else: changed = bytes([data[0]^1])+data[1:]
                require(len(changed) == len(data))
                fd = os.open(target,os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
                try: require(os.write(fd,changed) == len(changed)); os.fsync(fd)
                finally: os.close(fd)
            else:
                temporary = target.parent/('.resident-'+token); write_file(temporary,data); os.replace(temporary,target)
    result = dict(version=1,case=case,request_token=token,status='observed',before=before,after=evidence(),unrelated_created=case == 'resident-unrelated')
    validate_mutation(result,case,token)
    return result


def parse_snapshot_packet(payload):
    require(type(payload) is bytes)
    value = abandon.decode(payload)
    if value.get('stage') == 'reader-refusal':
        reader.parse_action_packet(payload)
    require(set(value) == {'stage','payload'} and value['stage'] == 'snapshot-held' and type(value['payload']) is dict)
    return value['payload']


def run_helper(helper, case, directory, expected, diagnostic):
    channel,peer = socket.socketpair(socket.AF_UNIX,socket.SOCK_SEQPACKET); channel.settimeout(45)
    process = None; host_fd = None; observed = False
    try:
        resolved = helper.resolve(strict=True); info = resolved.stat(); executable = (resolved,info.st_dev,info.st_ino)
        process = subprocess.Popen([str(helper),'--resident-case',case,'--observer-fd',str(peer.fileno())],stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=life.private_env(Path('/tmp')),cwd='/',close_fds=True,pass_fds=(peer.fileno(),))
        peer.close(); diagnostic['stage'] = 'before-packet'
        before = abandon.packet(channel,{'case','identity'},'before'); require(before['case'] == case)
        identity = before['identity']; reader.validate_identity(identity); require(identity['host_pid'] == process.pid)
        host_fd = os.pidfd_open(process.pid,0)
        def verify(start=None): return provision.verify_process(process.pid,os.getpid(),executable,host_fd,'reader',start)
        started = verify(); channel.sendall(b'G')
        diagnostic['stage'] = 'snapshot-packet'
        held = parse_snapshot_packet(channel.recv(8193)); validate_result(held,case,identity,observation=True,held=True); verify(started)
        require(held['selected_installation_id'] == (None if case in NEGATIVES else expected['fixed']['result']['real_installation_id']))
        initial = evidence(); require(initial == expected['fixture_evidence'])
        if case in MUTATIONS:
            request = dict(version=1,case=case,request_token=uuid.uuid4().hex,reader_host_pid=process.pid,reader_starttime=started)
            validate_request(request,case); life.private_write(directory/'request.json',encoded(request))
            fixed.wait_ack(directory,request); verify(started)
        channel.sendall(b'G'); diagnostic['stage'] = 'action-packet'
        action = reader.parse_action_packet(channel.recv(8193)); validate_result(action,case,identity,observation=True); verify(started)
        actual = evidence()
        if case not in MUTATIONS: require(actual == initial)
        real_iid,real_meta = fixed.binding_evidence(fixed.REAL); decoy_iid,decoy_meta = fixed.binding_evidence(fixed.DECOY)
        require(real_iid == expected['fixed']['result']['real_installation_id'] and decoy_iid == expected['fixed']['result']['decoy_installation_id']
            and real_meta['sha256'] == expected['fixed']['result']['real_binding_sha256'] and decoy_meta['sha256'] == expected['fixed']['result']['decoy_binding_sha256'])
        mount_evidence = None
        if case in MOUNT_PATHS:
            path = fixed.REAL/MOUNT_PATHS[case]
            mount,parent = fixed.mount_identity(Path('/proc/self/mountinfo').read_text(encoding='ascii'),str(path))
            relative = MOUNT_PATHS[case].split('/state/',1)[1]
            require(metadata(path) == expected['fixture_evidence'][relative])
            mount_evidence = dict(target=MOUNT_PATHS[case],mount_id=mount,parent_mount_id=parent,same_device=True,same_inode_as_setup=True)
        channel.sendall(b'G'); diagnostic['stage'] = 'final-packet'
        result = abandon.decode(channel.recv(8193)); validate_result(result,case,identity)
        require(all(result[field] == action[field] for field in READER_FIELDS-{'resources_closed_after_exit','pressure_iterations'}))
        require(abandon.pidfd_ready(host_fd,3)); output = launcher.bounded_output(process,time.monotonic()+3)
        require(process.wait(timeout=2) == 0 and abandon.decode(output) == result)
        result.update(process_identities_verified=True,privilege_profile_verified=True,host_retained_until_observation=True,
            host_exit_code=0,resources_closed_after_observed_exit=True,real_binding_sha256=real_meta['sha256'],decoy_binding_sha256=decoy_meta['sha256'],
            before_after=dict(before=initial,after=actual),mount_evidence=mount_evidence)
        observed = True; return result
    finally:
        if not observed: UNCERTAIN.append((process,channel,peer,host_fd))
        else:
            for value in (channel,peer,process.stdout,process.stderr): value.close()
            if host_fd is not None: os.close(host_fd)


def validate_observation(value, phase=None, case=None):
    require(type(value) is dict)
    phase = value.get('phase') if phase is None else phase; case = value.get('case') if case is None else case
    require(phase in {'setup','reader','mutate'} and case in CASES)
    require(set(value) == SCOPE_FIELDS | {'version','experiment','phase','case','status','helper_sha256','observer_sha256','kernel_release','resource_limits','result'})
    require(value['phase'] == phase and value['case'] == case and value['experiment'] == 'rust-resident'
        and type(value['version']) is int and value['version'] == 1 and value['status'] == 'observed')
    require(all(value[field] is False for field in SCOPE_FIELDS))
    for field in ('helper_sha256','observer_sha256'): require(hexadecimal(value[field],64))
    require(type(value['kernel_release']) is str and 0 < len(value['kernel_release']) <= 128 and all(33 <= ord(c) <= 126 for c in value['kernel_release']))
    validate_limits(value['resource_limits'],phase); result = value['result']
    if phase == 'setup':
        require(type(result) is dict and set(result) == {'fixed','fixture_evidence'})
        fixed.validate_observation(result['fixed'],'setup','fixed-valid'); validate_evidence(result['fixture_evidence'],case)
    elif phase == 'mutate': validate_mutation(result,case,result.get('request_token'))
    else:
        require(type(result) is dict and set(result) == READER_FIELDS | EXTERNAL)
        validate_result({key:result[key] for key in READER_FIELDS},case)
        for key in fixed.EXTERNAL-{'host_exit_code','real_binding_sha256','decoy_binding_sha256'}: require(result[key] is True)
        require(type(result['host_exit_code']) is int and result['host_exit_code'] == 0)
        for key in ('real_binding_sha256','decoy_binding_sha256'): require(hexadecimal(result[key],64))
        pair = result['before_after']; require(type(pair) is dict and set(pair) == {'before','after'})
        for item in pair.values(): validate_evidence(item,case)
        if case not in MUTATIONS: require(pair['before'] == pair['after'])
        evidence_item = result['mount_evidence']
        if case not in MOUNT_PATHS: require(evidence_item is None)
        else:
            require(type(evidence_item) is dict and set(evidence_item) == {'target','mount_id','parent_mount_id','same_device','same_inode_as_setup'})
            require(evidence_item['target'] == MOUNT_PATHS[case] and evidence_item['same_device'] is True and evidence_item['same_inode_as_setup'] is True)
            require(all(type(evidence_item[key]) is int and evidence_item[key] > 0 for key in ('mount_id','parent_mount_id'))
                and evidence_item['mount_id'] != evidence_item['parent_mount_id'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--phase',choices=('setup','reader','mutate'),required=True)
    parser.add_argument('--case',choices=sorted(CASES),required=True)
    parser.add_argument('--launcher-helper',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--expected',type=Path)
    parser.add_argument('--request-token')
    args = parser.parse_args(); report = dict.fromkeys(SCOPE_FIELDS,False)
    report.update(version=1,experiment='rust-resident',phase=args.phase,case=args.case,status='incomplete')
    diagnostic = dict(case=args.case,phase=args.phase,stage='setup')
    signal.alarm(120)
    try:
        require(sys.platform == 'linux' and os.environ.get('VIVAC_OPENSSH_DISPOSABLE') == '1')
        life.require_isolation(); provision.validate_privileges(provision.process_credentials(os.getpid()),'writer' if args.phase == 'setup' else 'reader')
        report['resource_limits'] = life.require_resources(expected_script='rust_resident_probe.py'); validate_limits(report['resource_limits'],args.phase)
        session.require_report_directory(args.report)
        info = args.report.parent.lstat(); owner = 0 if args.phase == 'setup' else 65534
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == info.st_gid == owner and stat.S_IMODE(info.st_mode) == 0o700)
        package = session.package_backend()
        require(package.is_mountpoint(fixed.REAL) and package.filesystem_magic(fixed.REAL) == package.EXT_MAGIC)
        if args.phase != 'mutate': require(package.is_mountpoint(Path('/decoy')) and package.filesystem_magic(fixed.DECOY) == package.EXT_MAGIC)
        fixture_bytes()
        report.update(kernel_release=os.uname().release,helper_sha256=hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest(),observer_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
        if args.phase == 'setup':
            fixed_result = fixed.run_helper(args.launcher_helper,'fixed-valid','setup',args.report.parent,None,diagnostic)
            fixed_report = dict(report,experiment='rust-fixed-root',case='fixed-valid',status='observed',result=fixed_result)
            fixed.validate_observation(fixed_report,'setup','fixed-valid')
            fixtures = populate(args.case,fixed_result['real_installation_id'])
            require(fixed.binding_evidence(fixed.REAL)[1]['sha256'] == fixed_result['real_binding_sha256']
                and fixed.binding_evidence(fixed.DECOY)[1]['sha256'] == fixed_result['decoy_binding_sha256'])
            report['result'] = dict(fixed=fixed_report,fixture_evidence=fixtures)
        elif args.phase == 'mutate': report['result'] = mutate(args.case,args.request_token)
        else:
            require(args.expected == Path('/reports/run/setup.json'))
            setup = abandon.decode(bounded_file(args.expected,32768)); validate_observation(setup,'setup',args.case)
            report['result'] = run_helper(args.launcher_helper,args.case,args.report.parent,setup['result'],diagnostic)
        report['status'] = 'observed'; validate_observation(report,args.phase,args.case)
    except Exception as error:
        failure = fixed.failure_diagnostic(error,diagnostic); failure['case'] = args.case
        report.update(status='incomplete',failure=failure)
    life.private_write(args.report,encoded(report)+b'\n')
    return 0 if report['status'] == 'observed' else 1


if __name__ == '__main__': sys.exit(main())
