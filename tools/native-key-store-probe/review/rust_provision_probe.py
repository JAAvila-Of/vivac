"""Observe disposable administrative publication and a separate unprivileged reader."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import sys
import time

import lifecycle_probe as life
import rust_abandon_probe as abandon
import rust_launcher_probe as launcher
import rust_root_reader_probe as reader
import session_probe as session

require = abandon.require
CASES = {'new-slot','existing-destination','existing-malformed','existing-symlink','concurrent-writers',
 'lost-capabilities','wrong-scaffold-owner','wrong-scaffold-mode','scaffold-symlink','scaffold-acl','missing-capabilities','invalid-selectors',
 'error-scaffold-2','error-scaffold-3','error-scaffold-4','error-scaffold-5','error-stage-created','error-before-handoff',
 'error-after-handoff','error-binding-sync','error-packages-sync','error-admissions-sync','error-openings-sync','error-endpoints-sync','error-sessions-sync','error-state-sync',
 'error-stage-sync','error-before-rename','error-after-rename','error-parent-sync','error-after-parent-sync',
 'crash-before-rename','crash-after-rename','lost-reply'}
SCOPE_FIELDS = reader.SCOPE_FIELDS
POST_ERRORS = {'error-after-rename','error-parent-sync','error-after-parent-sync'}
OPEN_CASES = {'new-slot','existing-destination','concurrent-writers','crash-after-rename','lost-reply'} | POST_ERRORS
EXISTING = {'existing-destination','existing-malformed','existing-symlink'}
SCAFFOLD_CASES = {'wrong-scaffold-owner','wrong-scaffold-mode','scaffold-symlink','scaffold-acl'}
DEATH_CASES = {'crash-before-rename','crash-after-rename','lost-reply'}
COMMON_FIELDS = SCOPE_FIELDS | {'version','case','status','identity','resources_closed_after_exit','pressure_threads','pressure_iterations'}
WRITER_FIELDS = COMMON_FIELDS | {'publication_results','unpublished_refusals','durability_indeterminate','reply_lost',
 'final_name_occupied','existing_unchanged','access_policy_checks','injected_failure','child_exits','owned_exits_observed'}
READER_FIELDS = COMMON_FIELDS | {'reader_opened'}
EXTERNAL_FIELDS = {'process_identities_verified','privilege_profile_verified','host_retained_until_observation',
 'children_observed_alive_before_action','children_retained_until_observation','children_exits_observed_externally',
 'host_exit_code','resources_closed_after_observed_exit','final_binding_sha256'}
REPORT_FIELDS = SCOPE_FIELDS | {'version','experiment','phase','status','cases','resource_limits','kernel_release',
 'resources_closed_after_observed_exit','candidate_provision_writer_observed','candidate_provision_reader_observed',
 'helper_sha256','observer_sha256'}
UNCERTAIN = []
ReaderRefusal = reader.ReaderRefusal
parse_action_packet = reader.parse_action_packet


def validate_privileges(fields, phase, *, missing=False):
    require(phase in {'writer','reader'})
    owner = 0 if phase == 'writer' else 65534
    caps = 5 if phase == 'writer' else 0
    for key, expected in {'Uid':[owner]*4,'Gid':[owner]*4,'NoNewPrivs':[1], 'CapEff':[0 if missing else caps],
                         'CapPrm':[0 if missing else caps],'CapBnd':[caps],'CapInh':[0],'CapAmb':[0]}.items():
        require(type(fields.get(key)) is list and all(type(item) is int for item in fields[key]) and fields[key] == expected, 'process privilege profile differs: ' + key)


def process_credentials(pid, proc_root=Path('/proc')):
    raw = (proc_root / str(pid) / 'status').read_bytes()
    require(len(raw) <= 16384)
    result = {}
    for line in raw.decode('ascii').splitlines():
        key, separator, value = line.partition(':')
        if separator and key in {'PPid','Uid','Gid','NoNewPrivs','CapEff','CapPrm','CapBnd','CapInh','CapAmb'}:
            require(key not in result)
            result[key] = [int(item, 16 if key.startswith('Cap') else 10) for item in value.split()]
    return result



def verify_threads(pid, phase, *, missing=False, single_thread=False):
    root = Path('/proc') / str(pid) / 'task'
    tasks = list(root.iterdir())
    require(bool(tasks) and (not single_thread or len(tasks) == 1), 'unexpected task count at privilege transition')
    for task in tasks:
        require(task.name.isdecimal())
        validate_privileges(process_credentials(int(task.name),proc_root=root),phase,missing=missing)


def verify_process(pid, parent, executable, pidfd, phase, expected_start=None, *, missing=False, single_thread=False):
    require(not abandon.pidfd_ready(pidfd))
    started = life.process_start(pid)
    require(type(started) is int and started > 0 and (expected_start is None or started == expected_start))
    fields = process_credentials(pid)
    validate_privileges(fields, phase, missing=missing)
    require(fields.get('PPid') == [parent])
    verify_threads(pid,phase,missing=missing,single_thread=single_thread)
    path = Path('/proc') / str(pid) / 'exe'
    info = path.stat()
    require(path.resolve(strict=True) == executable[0] and (info.st_dev,info.st_ino) == executable[1:])
    require(life.process_start(pid) == started and not abandon.pidfd_ready(pidfd))
    return started


def validate_identity(value, case, phase, expected=None):
    require(type(value) is dict and set(value) == {'host_pid','creator_tid','children'})
    require(type(value['host_pid']) is int and 0 < value['host_pid'] <= 0xffffffffffffffff)
    require(type(value['creator_tid']) is int and value['creator_tid'] == value['host_pid'])
    count = (2 if case == 'concurrent-writers' else int(case in DEATH_CASES)) if phase == 'writer' else 0
    require(type(value['children']) is list and len(value['children']) == count)
    pids, tokens = {value['host_pid']}, set()
    for ordinal, child in enumerate(value['children'], 1):
        require(type(child) is dict and set(child) == {'pid','token','ordinal'})
        for field in ('pid','token'): require(type(child[field]) is int and 0 < child[field] <= 0xffffffffffffffff)
        require(type(child['ordinal']) is int and child['ordinal'] == ordinal and child['pid'] not in pids and child['token'] not in tokens)
        pids.add(child['pid']); tokens.add(child['token'])
    require(expected is None or value == expected)


def validate_result(value, case, phase, identity=None, *, observation=False):
    require(phase in {'writer','reader'} and case in CASES)
    require(type(value) is dict and set(value) == (WRITER_FIELDS if phase == 'writer' else READER_FIELDS))
    require(value['case'] == case and value['status'] == 'observed')
    validate_identity(value['identity'], case, phase, identity)
    require(all(value[field] is False for field in SCOPE_FIELDS))
    for field, expected in {'version':1,'pressure_threads':4}.items(): require(type(value[field]) is int and value[field] == expected)
    require(type(value['pressure_iterations']) is int and 0 <= value['pressure_iterations'] <= 0xffffffffffffffff)
    require(value['pressure_iterations'] == 0 if observation else value['pressure_iterations'] > 0)
    require(value['resources_closed_after_exit'] is (not observation))
    if phase == 'reader':
        require(value['reader_opened'] is (case in OPEN_CASES))
        return
    publication = int(case in {'new-slot','existing-destination','concurrent-writers'})
    refused = 4 if case == 'invalid-selectors' else int(case not in {'new-slot','crash-before-rename','crash-after-rename','lost-reply'} | POST_ERRORS)
    for field, expected in {'publication_results':publication,'unpublished_refusals':refused,'access_policy_checks':int(case == 'scaffold-acl')}.items():
        require(type(value[field]) is int and value[field] == expected, 'publication counter differs: ' + field)
    for field, expected in {'durability_indeterminate':case in POST_ERRORS | {'crash-after-rename'}, 'reply_lost':case == 'lost-reply',
        'final_name_occupied':case in OPEN_CASES | EXISTING,'existing_unchanged':case in EXISTING,
        'owned_exits_observed':not observation or case in DEATH_CASES}.items(): require(value[field] is expected, 'publication evidence differs: ' + field)
    require(value['injected_failure'] == (case if case.startswith('error-') else 'none'))
    children = [] if observation and case not in DEATH_CASES else value['identity']['children']
    require(type(value['child_exits']) is list and len(value['child_exits']) == len(children))
    for child, exited in zip(children, value['child_exits']):
        require(type(exited) is dict and set(exited) == {'pid','token','code','signal'})
        for field in ('pid','token'): require(type(exited[field]) is int and exited[field] == child[field])
        if case in DEATH_CASES: require(exited['code'] is None and type(exited['signal']) is int and exited['signal'] == 9)
        else: require(type(exited['code']) is int and exited['code'] == 0 and exited['signal'] is None)


def validate_case(value, phase):
    fields = WRITER_FIELDS if phase == 'writer' else READER_FIELDS
    require(type(value) is dict and set(value) == fields | EXTERNAL_FIELDS | ({'scaffold_unchanged'} if phase == 'writer' else set()))
    validate_result({key:value[key] for key in fields}, value['case'], phase)
    for field in EXTERNAL_FIELDS - {'host_exit_code','final_binding_sha256','children_observed_alive_before_action','children_retained_until_observation'}:
        require(value[field] is True)
    require(value['children_observed_alive_before_action'] is bool(value['identity']['children']))
    require(value['children_retained_until_observation'] is (phase == 'writer' and value['case'] == 'concurrent-writers'))
    require(type(value['host_exit_code']) is int and value['host_exit_code'] == 0)
    if phase == 'writer': require(value['scaffold_unchanged'] is (True if value['case'] in SCAFFOLD_CASES else None))
    digest = value['final_binding_sha256']
    require(digest is None or type(digest) is str and len(digest) == 64 and all(c in '0123456789abcdef' for c in digest))
    require((digest is not None) is (value['case'] in OPEN_CASES | {'existing-malformed'}))


def validate_observation(value, phase=None):
    require(type(value) is dict and set(value) == REPORT_FIELDS)
    phase = value['phase'] if phase is None else phase
    require(phase in {'writer','reader'} and value['phase'] == phase and value['experiment'] == 'rust-provision')
    require(type(value['version']) is int and value['version'] == 1 and value['status'] == 'observed')
    require(all(value[field] is False for field in SCOPE_FIELDS))
    require(value['resources_closed_after_observed_exit'] is True)
    for candidate in ('writer','reader'): require(value['candidate_provision_' + candidate + '_observed'] is (candidate == phase))
    require(type(value['cases']) is list and len(value['cases']) == len(CASES) and {case.get('case') for case in value['cases']} == CASES)
    for case in value['cases']: validate_case(case, phase)
    for field in ('helper_sha256','observer_sha256'):
        require(type(value[field]) is str and len(value[field]) == 64 and all(c in '0123456789abcdef' for c in value[field]))
    require(type(value['kernel_release']) is str and 0 < len(value['kernel_release']) <= 128 and all(33 <= ord(c) <= 126 for c in value['kernel_release']))
    limits = value['resource_limits']
    require(type(limits) is dict and set(limits) == {'memory_bytes','swap_bytes','pids','cpu_quota','cpu_period','temporary_bytes'})
    require(all(type(item) is int for item in limits.values()))
    require(0 < limits['memory_bytes'] <= 536870912 and limits['swap_bytes'] == 0 and 0 < limits['pids'] <= 128
        and 0 < limits['cpu_quota'] <= limits['cpu_period'] and 0 < limits['temporary_bytes'] <= 134217728)


def binding_digest(root):
    # Walk through retained no-follow directory descriptors; never read an alias or FIFO.
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        for component in ('var','lib','vivac','custody-v1','users','65534'):
            try: next_fd = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=descriptor)
            except (FileNotFoundError, NotADirectoryError, PermissionError): return None
            os.close(descriptor); descriptor = next_fd
        try: fd = os.open('binding.json', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=descriptor)
        except FileNotFoundError: return None
        try:
            info = os.fstat(fd)
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size <= 512)
            data = os.read(fd,513)
            require(len(data) == info.st_size and len(data) <= 512)
            return hashlib.sha256(data).hexdigest()
        finally: os.close(fd)
    finally: os.close(descriptor)


def verify_fixture_owners(root, case):
    snapshot = reader.fixture_snapshot(root)
    custody = './var/lib/vivac/custody-v1'
    for relative, (metadata, _) in snapshot.items():
        owner = 65534 if case == 'wrong-scaffold-owner' and relative == custody else 0
        require(metadata[1] == metadata[2] == owner, 'fixture ownership differs')
    require(snapshot['.'][0][0] & 0o777 == 0o755)
    require(snapshot['./var'][0][0] & 0o777 == 0o755 and snapshot['./var/lib'][0][0] & 0o777 == 0o755)


def scaffold_snapshot(root):
    entries = reader.fixture_snapshot(root)
    attributes = {}
    root_fd = os.open(root,os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        for relative,(metadata,_) in entries.items():
            if not stat.S_ISDIR(metadata[0]): continue
            descriptor = os.dup(root_fd)
            try:
                for component in relative.split('/')[1:]:
                    next_fd = os.open(component,os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,dir_fd=descriptor)
                    os.close(descriptor);descriptor=next_fd
                info = os.fstat(descriptor)
                require((info.st_mode,info.st_uid,info.st_gid,info.st_dev,info.st_ino,info.st_nlink) == metadata)
                names = os.listxattr(descriptor)
                require(len(names) <= 64)
                selected = {}
                for name in ('system.posix_acl_access','system.posix_acl_default'):
                    if name in names:
                        data = os.getxattr(descriptor,name)
                        require(len(data) <= 4096)
                        selected[name] = data
                attributes[relative] = selected
            finally: os.close(descriptor)
    finally: os.close(root_fd)
    require(reader.fixture_snapshot(root) == entries, 'scaffold changed during snapshot')
    return entries,attributes


def require_scaffold_unchanged(before,after):
    require(before == after, 'existing scaffold changed during refused provisioning')


def prepare_acl(root):
    path = root / 'var/lib/vivac/custody-v1'
    payload = struct.pack('<I',2)
    for tag, permissions, identifier in ((1,7,0xffffffff),(2,0,65534),(4,5,0xffffffff),(16,5,0xffffffff),(32,5,0xffffffff)):
        payload += struct.pack('<HHI',tag,permissions,identifier)
    os.setxattr(path,'system.posix_acl_access',payload,follow_symlinks=False)
    info = path.stat()
    require(info.st_uid == info.st_gid == 0 and stat.S_IMODE(info.st_mode) == 0o755 and len(os.getxattr(path,'system.posix_acl_access',follow_symlinks=False)) > 0)


def run_case(helper, store, case, phase):
    root = store / case
    if phase == 'writer':
        verify_fixture_owners(root,case)
        if case == 'scaffold-acl': prepare_acl(root)
    before_tree = reader.fixture_snapshot(root) if phase == 'writer' and case in {'lost-capabilities','missing-capabilities','invalid-selectors'} else None
    scaffold_before = scaffold_snapshot(root) if phase == 'writer' and case in SCAFFOLD_CASES else None
    descriptor = os.open(root,os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    channel, peer = socket.socketpair(socket.AF_UNIX,socket.SOCK_SEQPACKET)
    channel.settimeout(8)
    process, pidfds, observed = None, [], False
    try:
        resolved = helper.resolve(strict=True); info = resolved.stat()
        executable = (resolved,info.st_dev,info.st_ino)
        process = subprocess.Popen([str(helper),'--provision-case' if phase == 'writer' else '--provision-reader',case,
            '--observer-fd',str(peer.fileno()),'--root-fd',str(descriptor)], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=life.private_env(store),close_fds=True,pass_fds=(peer.fileno(),descriptor))
        peer.close()
        before = abandon.packet(channel,{'case','identity'},'before')
        require(before['case'] == case)
        identity = before['identity']; validate_identity(identity,case,phase)
        require(identity['host_pid'] == process.pid)
        host_fd = os.pidfd_open(process.pid,0); pidfds.append(host_fd)
        missing = phase == 'writer' and case == 'missing-capabilities'
        host_start = verify_process(process.pid,os.getpid(),executable,host_fd,phase,missing=missing,single_thread=phase == 'writer' and case == 'lost-capabilities')
        children = []
        for child in identity['children']:
            fd = os.pidfd_open(child['pid'],0); pidfds.append(fd)
            started = verify_process(child['pid'],process.pid,executable,fd,phase)
            children.append((child,fd,started))
        channel.sendall(b'G')
        action = parse_action_packet(channel.recv(8193)); validate_result(action,case,phase,identity,observation=True)
        verify_process(process.pid,os.getpid(),executable,host_fd,phase,host_start,missing=missing or case == 'lost-capabilities')
        for child,fd,started in children:
            if case in DEATH_CASES: require(abandon.pidfd_ready(fd,3))
            else: verify_process(child['pid'],process.pid,executable,fd,phase,started)
        if before_tree is not None: require(reader.fixture_snapshot(root) == before_tree)
        if scaffold_before is not None: require_scaffold_unchanged(scaffold_before,scaffold_snapshot(root))
        digest = binding_digest(root)
        channel.sendall(b'G')
        result = abandon.decode(channel.recv(8193)); validate_result(result,case,phase,identity)
        fields = WRITER_FIELDS if phase == 'writer' else READER_FIELDS
        for field in fields - {'child_exits','owned_exits_observed','resources_closed_after_exit','pressure_iterations'}:
            require(result[field] == action[field])
        for _,fd,_ in children: require(abandon.pidfd_ready(fd,3))
        require(abandon.pidfd_ready(host_fd,3))
        output = launcher.bounded_output(process,time.monotonic()+3)
        stdout_result = abandon.decode(output)
        validate_result(stdout_result,case,phase,identity)
        require(process.wait(timeout=2) == 0 and stdout_result == result)
        require(binding_digest(root) == digest)
        result.update(process_identities_verified=True,privilege_profile_verified=True,host_retained_until_observation=True,
            children_observed_alive_before_action=bool(children),children_retained_until_observation=phase == 'writer' and case == 'concurrent-writers',
            children_exits_observed_externally=True,host_exit_code=0,resources_closed_after_observed_exit=True,final_binding_sha256=digest)
        if phase == 'writer':
            if scaffold_before is not None: require_scaffold_unchanged(scaffold_before,scaffold_snapshot(root))
            result['scaffold_unchanged'] = True if scaffold_before is not None else None
        validate_case(result,phase); observed = True
        return result
    finally:
        if not observed: UNCERTAIN.append((process,channel,peer,descriptor,pidfds))
        else:
            for value in (channel,peer,process.stdout,process.stderr): value.close()
            for value in (descriptor,*pidfds): os.close(value)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--phase',choices=('writer','reader'),required=True)
    for name in ('launcher-helper','store-root','report'): parser.add_argument('--'+name,type=Path,required=True)
    args = parser.parse_args()
    report = dict.fromkeys(SCOPE_FIELDS,False)
    report.update(version=1,experiment='rust-provision',phase=args.phase,status='incomplete',cases=[],resources_closed_after_observed_exit=False,
        candidate_provision_writer_observed=False,candidate_provision_reader_observed=False)
    case = 'none'
    try:
        require(sys.platform == 'linux' and os.environ.get('VIVAC_OPENSSH_DISPOSABLE') == '1')
        life.require_isolation(); validate_privileges(process_credentials(os.getpid()),args.phase)
        report['kernel_release'] = os.uname().release
        report['resource_limits'] = life.require_resources(expected_script='rust_provision_probe.py')
        session.require_report_directory(args.report)
        metadata = args.report.parent.lstat()
        require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == metadata.st_gid == (0 if args.phase == 'writer' else 65534)
            and stat.S_IMODE(metadata.st_mode) == 0o700)
        require(args.store_root == Path('/store/run') and not args.store_root.is_symlink())
        info = args.store_root.stat()
        require(info.st_uid == info.st_gid == 0 and stat.S_IMODE(info.st_mode) == 0o755)
        package = session.package_backend()
        require(package.is_mountpoint(Path('/store')) and package.filesystem_magic(args.store_root) == package.EXT_MAGIC)
        require({item.name for item in args.store_root.iterdir()} == CASES)
        report['helper_sha256'] = hashlib.sha256(args.launcher_helper.read_bytes()).hexdigest()
        report['observer_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        for case in sorted(CASES): report['cases'].append(run_case(args.launcher_helper,args.store_root,case,args.phase))
        report.update(status='observed',resources_closed_after_observed_exit=True)
        report['candidate_provision_'+args.phase+'_observed'] = True
        validate_observation(report,args.phase)
    except Exception as error:
        report.update(status='incomplete',resources_closed_after_observed_exit=False,candidate_provision_writer_observed=False,candidate_provision_reader_observed=False,
            failure={'kind':type(error).__name__,'case':case})
        if type(error) is ReaderRefusal: report['failure']['diagnostic'] = error.diagnostic
    life.private_write(args.report,json.dumps(report,sort_keys=True).encode('ascii')+b'\n')
    return 0 if report['status'] == 'observed' else 1

if __name__ == '__main__': sys.exit(main())
