"""Observe fixed namespace selection in disposable, independently owned volumes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import stat
import subprocess
import sys
import time
import uuid

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_launcher_probe as launcher
import rust_provision_probe as provision
import rust_root_reader_probe as reader
import session_probe as session

require = abandon.require
SCOPE_FIELDS = reader.SCOPE_FIELDS
CASES = {'fixed-valid','environment-decoy','cwd-decoy','missing-fixed-root','binding-in-place',
         'binding-replacement','binding-mode','mount-users','mount-state'}
MUTATIONS = {'binding-in-place','binding-replacement','binding-mode'}
NEGATIVES = {'missing-fixed-root','mount-users','mount-state'}
COMMON = SCOPE_FIELDS | {'version','case','status','identity','resources_closed_after_exit','pressure_threads','pressure_iterations'}
SETUP_FIELDS = COMMON | {'real_publications','decoy_publications','real_installation_id','decoy_installation_id'}
READER_FIELDS = COMMON | {'initial_opened','initial_validated','selected_installation_id','initial_error','snapshot_invalidated','validation_error'}
EXTERNAL = {'process_identities_verified','privilege_profile_verified','host_retained_until_observation','host_exit_code',
            'resources_closed_after_observed_exit','real_binding_sha256','decoy_binding_sha256'}
METADATA = {'device','inode','uid','gid','mode','nlink','size','sha256'}
REAL = Path('/var/lib/vivac')
DECOY = Path('/decoy/var/lib/vivac')
SUFFIX = 'custody-v1/users/65534/binding.json'
UNCERTAIN = []
DIAGNOSTIC_STAGES = {'setup','before-packet','identity','proc-verification','action-packet','action-validation',
    'binding-evidence','final-packet','final-exit','report-validation','snapshot-packet','snapshot-validation',
    'mutation-acknowledgement','mount-evidence','mutation'}


def failure_diagnostic(error, context):
    kind=type(error).__name__
    result=dict(kind=kind if kind in {'ValueError','OSError','TimeoutError','FileNotFoundError','PermissionError',
        'BrokenPipeError','ConnectionResetError','ChildProcessError','ReaderRefusal'} else 'Exception',
        case=context.get('case') if context.get('case') in CASES else 'none',
        phase=context.get('phase') if context.get('phase') in {'setup','reader','mutate'} else 'none',
        stage=context.get('stage') if context.get('stage') in DIAGNOSTIC_STAGES else 'unknown')
    if type(error) is reader.ReaderRefusal:result['diagnostic']=error.diagnostic
    return result


def hexadecimal(value, size):
    return type(value) is str and len(value) == size and all(c in '0123456789abcdef' for c in value)


def installation(value):
    return type(value) is str and re.fullmatch('[0-7][0-9a-hjkmnp-tv-z]{25}',value) is not None


def parse_binding(data):
    require(type(data) is bytes and 0 < len(data) <= 512)
    value = abandon.decode(data)
    require(set(value) == {'version','installation_id','owner_uid','owner_gid','profile_revision'})
    for field,expected in {'version':1,'owner_uid':65534,'owner_gid':65534,'profile_revision':1}.items():
        require(type(value[field]) is int and value[field] == expected)
    require(installation(value['installation_id']))
    canonical = ('{"version":1,"installation_id":"'+value['installation_id']+'","owner_uid":65534,"owner_gid":65534,"profile_revision":1}').encode('ascii')
    require(data == canonical, 'noncanonical public binding')
    return value['installation_id']


def validate_request(value, case):
    require(type(value) is dict and set(value) == {'version','case','request_token','reader_host_pid','reader_starttime'})
    require(case in MUTATIONS and value['case'] == case and type(value['version']) is int and value['version'] == 1)
    require(hexadecimal(value['request_token'],32))
    for field in ('reader_host_pid','reader_starttime'): require(type(value[field]) is int and 0 < value[field] <= 0xffffffffffffffff)


def validate_metadata(value, *, mode=0o644):
    require(type(value) is dict and set(value) == METADATA)
    require(all(type(value[key]) is int for key in METADATA - {'sha256'}))
    require(value['device'] >= 0 and value['inode'] > 0 and value['uid'] == value['gid'] == 0
            and value['mode'] == mode and value['nlink'] == 1 and 0 < value['size'] <= 512 and hexadecimal(value['sha256'],64))


def validate_mutation(value, case, token):
    require(type(value) is dict and set(value) == {'version','case','request_token','status','operation','before','after'})
    require(type(value['version']) is int and value['version'] == 1 and value['case'] == case and case in MUTATIONS
            and value['request_token'] == token and hexadecimal(token,32) and value['status'] == 'observed')
    before,after = value['before'],value['after']
    validate_metadata(before); validate_metadata(after,mode=0o600 if case == 'binding-mode' else 0o644)
    operation = {'binding-mode':'chmod','binding-in-place':'write','binding-replacement':'replace'}[case]
    require(value['operation'] == operation)
    changed = {'mode'} if case == 'binding-mode' else {'sha256'} if case == 'binding-in-place' else {'inode'}
    require(all(before[key] == after[key] for key in METADATA - changed))
    require(all(before[key] != after[key] for key in changed))


def binding_parent(root):
    require(root in (REAL,DECOY))
    descriptor = os.open('/',os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        for component in (root / SUFFIX).parts[1:-1]:
            next_fd = os.open(component,os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,dir_fd=descriptor)
            os.close(descriptor); descriptor = next_fd
        return descriptor
    except BaseException:
        os.close(descriptor); raise


def read_binding_at(parent, *, writable=False, mode=0o644):
    fd = os.open('binding.json',(os.O_RDWR if writable else os.O_RDONLY) | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,dir_fd=parent)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 0 and stat.S_IMODE(info.st_mode) == mode
                and info.st_nlink == 1 and 0 < info.st_size <= 512)
        data = os.read(fd,513)
        require(len(data) == info.st_size)
        iid = parse_binding(data)
        linked = os.stat('binding.json',dir_fd=parent,follow_symlinks=False)
        require((linked.st_dev,linked.st_ino) == (info.st_dev,info.st_ino))
        metadata = dict(device=info.st_dev,inode=info.st_ino,uid=info.st_uid,gid=info.st_gid,
                        mode=stat.S_IMODE(info.st_mode),nlink=info.st_nlink,size=info.st_size,sha256=hashlib.sha256(data).hexdigest())
        return fd,data,iid,metadata
    except BaseException:
        os.close(fd); raise


def binding_evidence(root):
    try: parent = binding_parent(root)
    except FileNotFoundError: return None,None
    try:
        try: fd,_,iid,metadata = read_binding_at(parent)
        except FileNotFoundError: return None,None
        os.close(fd)
        return iid,metadata
    finally: os.close(parent)


def mutate(case, token):
    require(case in MUTATIONS and hexadecimal(token,32))
    parent = binding_parent(REAL)
    fd = None
    temporary = None
    try:
        fd,data,iid,before = read_binding_at(parent,writable=case == 'binding-in-place')
        if case == 'binding-in-place':
            new_id = '00000000000000000000000001' if iid != '00000000000000000000000001' else '00000000000000000000000002'
            changed = data.replace(iid.encode('ascii'),new_id.encode('ascii'))
            require(len(changed) == len(data) and parse_binding(changed) == new_id)
            require(os.lseek(fd,0,os.SEEK_SET) == 0 and os.write(fd,changed) == len(changed))
            os.fsync(fd)
        elif case == 'binding-mode':
            os.fchmod(fd,0o600)
            os.fsync(fd)
        else:
            temporary = '.fixed-root-'+token
            replacement = os.open(temporary,os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,0o644,dir_fd=parent)
            try:
                os.fchmod(replacement,0o644)
                require(os.write(replacement,data) == len(data)); os.fsync(replacement)
                info = os.fstat(replacement)
                require(info.st_dev == before['device'] and info.st_ino != before['inode'] and info.st_uid == info.st_gid == 0)
            finally: os.close(replacement)
            os.replace(temporary,'binding.json',src_dir_fd=parent,dst_dir_fd=parent); temporary = None
            os.fsync(parent)
        after_fd,_,_,after = read_binding_at(parent,mode=0o600 if case == 'binding-mode' else 0o644)
        os.close(after_fd)
        result = dict(version=1,case=case,request_token=token,status='observed',operation={
            'binding-in-place':'write','binding-mode':'chmod','binding-replacement':'replace'}[case],before=before,after=after)
        validate_mutation(result,case,token)
        return result
    finally:
        if fd is not None: os.close(fd)
        if temporary is not None: os.unlink(temporary,dir_fd=parent)
        os.close(parent)


def mount_identity(raw, target):
    entries = []
    require(type(raw) is str and len(raw) <= 131072)
    for line in raw.splitlines():
        fields = line.split()
        require(len(fields) >= 10)
        if fields[4] == target: entries.append(fields)
    require(len(entries) == 1)
    fields = entries[0]
    mount,parent = int(fields[0]),int(fields[1])
    require(mount > 0 and parent > 0 and mount != parent and 'ro' in fields[5].split(','))
    parents = [line.split() for line in raw.splitlines() if line.split()[0] == str(parent)]
    require(len(parents) == 1 and parents[0][2] == fields[2])
    return mount,parent


def directory_metadata(path):
    fd = os.open(path,os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        return dict(device=info.st_dev,inode=info.st_ino,uid=info.st_uid,gid=info.st_gid,mode=stat.S_IMODE(info.st_mode))
    finally: os.close(fd)


def validate_result(value, case, phase, identity=None, *, observation=False, held=False):
    fields = SETUP_FIELDS if phase == 'setup' else READER_FIELDS
    require(phase in {'setup','reader'} and case in CASES and type(value) is dict and set(value) == fields)
    require(value['case'] == case and value['status'] == 'observed')
    reader.validate_identity(value['identity'],identity)
    require(all(value[field] is False for field in SCOPE_FIELDS))
    for field,expected in {'version':1,'pressure_threads':4}.items(): require(type(value[field]) is int and value[field] == expected)
    require(type(value['pressure_iterations']) is int and 0 <= value['pressure_iterations'] <= 0xffffffffffffffff)
    require(value['pressure_iterations'] == 0 if observation else value['pressure_iterations'] > 0)
    require(value['resources_closed_after_exit'] is (not observation))
    if phase == 'setup':
        require(type(value['real_publications']) is int and value['real_publications'] == int(case != 'missing-fixed-root'))
        require(type(value['decoy_publications']) is int and value['decoy_publications'] == 1)
        require(installation(value['decoy_installation_id']))
        require(value['real_installation_id'] is None if case == 'missing-fixed-root' else installation(value['real_installation_id'])
                and value['real_installation_id'] != value['decoy_installation_id'])
    else:
        opened = case not in NEGATIVES
        require(value['initial_opened'] is opened and value['initial_validated'] is opened)
        require(value['initial_error'] == ('not-found' if case == 'missing-fixed-root' else 'crosses-devices' if case.startswith('mount-') else 'none'))
        require(installation(value['selected_installation_id']) if opened else value['selected_installation_id'] is None)
        require(value['snapshot_invalidated'] is (case in MUTATIONS and not held))
        require(value['validation_error'] in ({'invalid-input','permission-denied'} if case in MUTATIONS and not held else {'none'}))


def validate_limits(limits,phase):
    require(type(limits) is dict and set(limits) == {'memory_bytes','swap_bytes','pids','cpu_quota','cpu_period','temporary_bytes'})
    require(all(type(value) is int for value in limits.values()))
    memory,pids,cpu,tmp = {'setup':(512,128,1,128),'reader':(320,96,.75,96),'mutate':(128,16,.25,16)}[phase]
    require(0 < limits['memory_bytes'] <= memory*1048576 and limits['swap_bytes'] == 0 and 0 < limits['pids'] <= pids
            and limits['cpu_period'] > 0 and 0 < limits['cpu_quota'] <= limits['cpu_period']*cpu
            and 0 < limits['temporary_bytes'] <= tmp*1048576)


def validate_observation(value,phase=None,case=None):
    require(type(value) is dict)
    phase = value.get('phase') if phase is None else phase
    case = value.get('case') if case is None else case
    require(phase in {'setup','reader','mutate'} and case in CASES)
    expected = SCOPE_FIELDS | {'version','experiment','phase','case','status','helper_sha256','observer_sha256','kernel_release','resource_limits','result'}
    require(set(value) == expected and value['phase'] == phase and value['case'] == case and value['experiment'] == 'rust-fixed-root'
            and type(value['version']) is int and value['version'] == 1 and value['status'] == 'observed')
    require(all(value[field] is False for field in SCOPE_FIELDS))
    for field in ('helper_sha256','observer_sha256'): require(hexadecimal(value[field],64))
    require(type(value['kernel_release']) is str and 0 < len(value['kernel_release']) <= 128 and all(33 <= ord(c) <= 126 for c in value['kernel_release']))
    validate_limits(value['resource_limits'],phase)
    result = value['result']
    if phase == 'mutate': validate_mutation(result,case,result.get('request_token')); return
    additions = {'users_metadata','state_metadata'} if phase == 'setup' else {'mutation_observed','same_installation_expected','mount_evidence'}
    fields = SETUP_FIELDS if phase == 'setup' else READER_FIELDS
    require(type(result) is dict and set(result) == fields | EXTERNAL | additions)
    validate_result({key:result[key] for key in fields},case,phase)
    for field in EXTERNAL - {'host_exit_code','real_binding_sha256','decoy_binding_sha256'}: require(result[field] is True)
    require(type(result['host_exit_code']) is int and result['host_exit_code'] == 0)
    require(hexadecimal(result['decoy_binding_sha256'],64))
    require(result['real_binding_sha256'] is None if case == 'missing-fixed-root' else hexadecimal(result['real_binding_sha256'],64))
    if phase == 'setup':
        for field in additions:
            item = result[field]
            if case == 'missing-fixed-root': require(item is None); continue
            require(type(item) is dict and set(item) == {'device','inode','uid','gid','mode'} and all(type(v) is int for v in item.values()))
            owner = 0 if field == 'users_metadata' else 65534
            require(item['device'] >= 0 and item['inode'] > 0 and item['uid'] == item['gid'] == owner and item['mode'] == (0o755 if owner == 0 else 0o700))
    else:
        require(result['mutation_observed'] is (case in MUTATIONS) and result['same_installation_expected'] is (case not in NEGATIVES))
        evidence = result['mount_evidence']
        if not case.startswith('mount-'): require(evidence is None)
        else:
            require(type(evidence) is dict and set(evidence) == {'target','mount_id','parent_mount_id','same_device','same_inode_as_setup'})
            require(evidence['target'] == case[6:] and evidence['same_device'] is True and evidence['same_inode_as_setup'] is True)
            require(type(evidence['mount_id']) is int and type(evidence['parent_mount_id']) is int
                    and evidence['mount_id'] > 0 and evidence['parent_mount_id'] > 0 and evidence['mount_id'] != evidence['parent_mount_id'])


def validate_privileges(fields,phase):
    provision.validate_privileges(fields,'writer' if phase == 'setup' else 'reader') if phase != 'mutate' else None
    if phase == 'mutate':
        for key,expected in {'Uid':[0]*4,'Gid':[0]*4,'NoNewPrivs':[1],'CapEff':[0],'CapPrm':[0],'CapBnd':[0],'CapInh':[0],'CapAmb':[0]}.items():
            require(type(fields.get(key)) is list and all(type(v) is int for v in fields[key]) and fields[key] == expected)


def wait_ack(directory,request):
    path = directory/'ack.json'
    deadline = time.monotonic()+45
    while not path.exists():
        require(time.monotonic() < deadline,'mutation acknowledgement timed out'); time.sleep(.1)
    fd = os.open(path,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534 and stat.S_IMODE(info.st_mode) == 0o600
                and info.st_nlink == 1 and 0 < info.st_size <= 512)
        value = abandon.decode(os.read(fd,513))
        require(set(value) == {'version','case','request_token','status'} and type(value['version']) is int and value['version'] == 1
                and value['case'] == request['case'] and value['request_token'] == request['request_token'] and value['status'] == 'observed')
    finally: os.close(fd)


def run_helper(helper,case,phase,directory,expected,diagnostic):
    channel,peer = socket.socketpair(socket.AF_UNIX,socket.SOCK_SEQPACKET)
    channel.settimeout(45)
    process = None; host_fd = None; observed = False
    try:
        resolved = helper.resolve(strict=True); info = resolved.stat(); executable=(resolved,info.st_dev,info.st_ino)
        environment = life.private_env(Path('/tmp'))
        if case in {'environment-decoy','missing-fixed-root'} and phase == 'reader':
            environment.update(HOME='/decoy',VIVAC_HOME='/decoy',XDG_STATE_HOME='/decoy')
        process = subprocess.Popen([str(helper),'--fixed-root-setup' if phase == 'setup' else '--fixed-root-case',case,
            '--observer-fd',str(peer.fileno())],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,
            env=environment,cwd='/decoy' if case == 'cwd-decoy' and phase == 'reader' else '/',close_fds=True,pass_fds=(peer.fileno(),))
        peer.close()
        diagnostic['stage']='before-packet'
        before = abandon.packet(channel,{'case','identity'},'before'); require(before['case'] == case)
        diagnostic['stage']='identity'
        identity=before['identity']; reader.validate_identity(identity); require(identity['host_pid'] == process.pid)
        host_fd=os.pidfd_open(process.pid,0)
        def verify(start=None):
            return provision.verify_process(process.pid,os.getpid(),executable,host_fd,'writer' if phase == 'setup' else 'reader',start)
        diagnostic['stage']='proc-verification'
        started=verify()
        channel.sendall(b'G')
        if phase == 'reader':
            diagnostic['stage']='snapshot-packet'
            held = abandon.packet(channel,{'payload'},'snapshot-held')['payload']
            diagnostic['stage']='snapshot-validation'
            validate_result(held,case,phase,identity,observation=True,held=True); verify(started)
            require(held['selected_installation_id'] == (expected['real_installation_id'] if case not in NEGATIVES else None))
            actual_iid,actual_metadata=binding_evidence(REAL)
            decoy_iid,decoy_metadata=binding_evidence(DECOY)
            require(actual_iid == expected['real_installation_id'] and (actual_metadata or {}).get('sha256') == expected['real_binding_sha256'])
            require(decoy_iid == expected['decoy_installation_id'] and decoy_metadata['sha256'] == expected['decoy_binding_sha256'])
            if case in MUTATIONS:
                diagnostic['stage']='mutation-acknowledgement'
                request=dict(version=1,case=case,request_token=uuid.uuid4().hex,reader_host_pid=process.pid,reader_starttime=started)
                validate_request(request,case)
                life.private_write(directory/'request.json',json.dumps(request,separators=(',',':')).encode('ascii'))
                wait_ack(directory,request); verify(started)
            channel.sendall(b'G')
        diagnostic['stage']='action-packet'
        action=reader.parse_action_packet(channel.recv(8193))
        diagnostic['stage']='action-validation'
        validate_result(action,case,phase,identity,observation=True)
        diagnostic['stage']='proc-verification'
        verify(started)
        diagnostic['stage']='binding-evidence'
        if phase == 'reader' and case == 'binding-mode':
            real_iid=expected['real_installation_id']; real_meta=actual_metadata
            info=(REAL/SUFFIX).lstat()
            require(stat.S_ISREG(info.st_mode) and info.st_dev == real_meta['device'] and info.st_ino == real_meta['inode']
                    and info.st_uid == info.st_gid == 0 and stat.S_IMODE(info.st_mode) == 0o600
                    and info.st_nlink == 1 and info.st_size == real_meta['size'])
        else: real_iid,real_meta=binding_evidence(REAL)
        decoy_iid,decoy_meta=binding_evidence(DECOY)
        if phase == 'setup':
            require(real_iid == action['real_installation_id'] and decoy_iid == action['decoy_installation_id'])
        else:
            require(decoy_iid == expected['decoy_installation_id'])
            require(decoy_meta['sha256'] == expected['decoy_binding_sha256'])
            if case not in MUTATIONS: require((real_meta or {}).get('sha256') == expected['real_binding_sha256'])
        channel.sendall(b'G')
        diagnostic['stage']='final-packet'
        result=abandon.decode(channel.recv(8193)); validate_result(result,case,phase,identity)
        fields = SETUP_FIELDS if phase == 'setup' else READER_FIELDS
        require(all(result[field] == action[field] for field in fields-{'resources_closed_after_exit','pressure_iterations'}))
        diagnostic['stage']='final-exit'
        require(abandon.pidfd_ready(host_fd,3))
        output=launcher.bounded_output(process,time.monotonic()+3)
        require(process.wait(timeout=2) == 0 and abandon.decode(output) == result)
        result.update(process_identities_verified=True,privilege_profile_verified=True,host_retained_until_observation=True,
            host_exit_code=0,resources_closed_after_observed_exit=True,real_binding_sha256=None if real_meta is None else real_meta['sha256'],
            decoy_binding_sha256=decoy_meta['sha256'])
        if phase == 'setup':
            result.update(users_metadata=None if real_iid is None else directory_metadata(REAL/'custody-v1/users'),
                state_metadata=None if real_iid is None else directory_metadata(REAL/'custody-v1/users/65534/state'))
        else:
            evidence=None
            if case.startswith('mount-'):
                diagnostic['stage']='mount-evidence'
                target='users' if case == 'mount-users' else 'state'
                path=REAL/('custody-v1/users' if target == 'users' else 'custody-v1/users/65534/state')
                mount,parent=mount_identity(Path('/proc/self/mountinfo').read_text(encoding='ascii'),str(path))
                current=directory_metadata(path); previous=expected[target+'_metadata']
                require(current == previous)
                evidence=dict(target=target,mount_id=mount,parent_mount_id=parent,same_device=True,same_inode_as_setup=True)
            result.update(mutation_observed=case in MUTATIONS,same_installation_expected=case not in NEGATIVES,mount_evidence=evidence)
        observed=True
        return result
    finally:
        if not observed: UNCERTAIN.append((process,channel,peer,host_fd))
        else:
            for value in (channel,peer,process.stdout,process.stderr): value.close()
            if host_fd is not None: os.close(host_fd)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--phase',choices=('setup','reader','mutate'),required=True)
    parser.add_argument('--case',choices=sorted(CASES),required=True)
    parser.add_argument('--launcher-helper',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--expected',type=Path)
    parser.add_argument('--request-token')
    args=parser.parse_args()
    report=dict.fromkeys(SCOPE_FIELDS,False)
    report.update(version=1,experiment='rust-fixed-root',phase=args.phase,case=args.case,status='incomplete')
    diagnostic=dict(case=args.case,phase=args.phase,stage='setup')
    try:
        require(sys.platform == 'linux' and os.environ.get('VIVAC_OPENSSH_DISPOSABLE') == '1')
        life.require_isolation(); validate_privileges(provision.process_credentials(os.getpid()),args.phase)
        report['resource_limits']=life.require_resources(expected_script='rust_fixed_root_probe.py'); validate_limits(report['resource_limits'],args.phase)
        session.require_report_directory(args.report)
        info=args.report.parent.lstat(); owner=65534 if args.phase == 'reader' else 0
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == info.st_gid == owner and stat.S_IMODE(info.st_mode) == 0o700)
        package=session.package_backend()
        require(package.is_mountpoint(REAL) and package.filesystem_magic(REAL) == package.EXT_MAGIC)
        if args.phase != 'mutate': require(package.is_mountpoint(Path('/decoy')) and package.filesystem_magic(DECOY) == package.EXT_MAGIC)
        report.update(kernel_release=os.uname().release,helper_sha256=hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest(),
            observer_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
        expected=None
        if args.phase == 'reader':
            require(args.expected == Path('/reports/run/setup.json'))
            data=args.expected.read_bytes(); require(len(data) <= 16384)
            setup=abandon.decode(data); validate_observation(setup,'setup',args.case); expected=setup['result']
        if args.phase == 'mutate':
            diagnostic['stage']='mutation'
            report['result']=mutate(args.case,args.request_token)
        else:report['result']=run_helper(args.launcher_helper,args.case,args.phase,args.report.parent,expected,diagnostic)
        diagnostic['stage']='report-validation'
        report['status']='observed'; validate_observation(report,args.phase,args.case)
    except Exception as error:
        report.update(status='incomplete',failure=failure_diagnostic(error,diagnostic))
    life.private_write(args.report,json.dumps(report,sort_keys=True).encode('ascii')+b'\n')
    return 0 if report['status'] == 'observed' else 1


if __name__ == '__main__': sys.exit(main())
