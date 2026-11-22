"""Run separate disposable administrative writer and target-user reader phases."""
import argparse
import datetime
import io
import json
import os
from pathlib import Path
import sys
import subprocess
import tarfile
import time
import uuid

import run_lifecycle_probe as base
import rust_provision_probe as probe

RunnerError = base.RunnerError
COMMAND_RESULTS = []


def command(arguments, timeout=15, payload=None, merge_stderr=False):
    result = subprocess.run(arguments, stdin=subprocess.DEVNULL if payload is None else None, input=payload,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT if merge_stderr else subprocess.PIPE, timeout=timeout, check=False)
    COMMAND_RESULTS.append({'command':arguments[:2], 'exit_code':result.returncode})
    if result.returncode: raise RunnerError('control command failed: ' + arguments[0] + ' exit=' + str(result.returncode))
    if len(result.stdout) > 128*1024 or result.stderr is not None and len(result.stderr) > 128*1024:
        raise RunnerError('control output exceeded its bound')
    return result.stdout
rdp_events = base.rdp_events


def archive_entry(archive,name,mode,owner=0,*,kind=tarfile.DIRTYPE,data=b'',target=''):
    item = tarfile.TarInfo(name)
    item.mode = mode; item.uid = item.gid = owner; item.type = kind
    item.size = len(data); item.linkname = target
    archive.addfile(item,io.BytesIO(data) if data else None)


def fixture_archive():
    output = io.BytesIO()
    with tarfile.open(fileobj=output,mode='w') as archive:
        for case in sorted(probe.CASES):
            for suffix in ('','/var','/var/lib'): archive_entry(archive,case+suffix,0o755)
            if case not in {'wrong-scaffold-owner','wrong-scaffold-mode','scaffold-symlink','scaffold-acl','existing-malformed','existing-symlink'}: continue
            vivac = case+'/var/lib/vivac'
            archive_entry(archive,vivac,0o755)
            custody = vivac+'/custody-v1'
            if case == 'scaffold-symlink':
                archive_entry(archive,custody,0o777,kind=tarfile.SYMTYPE,target='custody-real')
                custody = vivac+'/custody-real'
            archive_entry(archive,custody,0o555 if case == 'wrong-scaffold-mode' else 0o755,65534 if case == 'wrong-scaffold-owner' else 0)
            archive_entry(archive,custody+'/users',0o755)
            if case == 'existing-malformed':
                slot = custody+'/users/65534'
                archive_entry(archive,slot,0o755)
                archive_entry(archive,slot+'/binding.json',0o644,kind=tarfile.REGTYPE,data=b'{}')
            elif case == 'existing-symlink':
                archive_entry(archive,custody+'/users/spare',0o755)
                archive_entry(archive,custody+'/users/65534',0o777,kind=tarfile.SYMTYPE,target='spare')
    return output.getvalue()


def initialize(container,phase):
    output = io.BytesIO()
    with tarfile.open(fileobj=output,mode='w') as archive:
        archive_entry(archive,'run',0o700,0 if phase == 'writer' else 65534)
    command(['docker','cp','-',container+':/reports'],payload=output.getvalue())
    if phase == 'writer':
        output = io.BytesIO()
        with tarfile.open(fileobj=output,mode='w') as archive: archive_entry(archive,'run',0o755)
        command(['docker','cp','-',container+':/store'],payload=output.getvalue())
        command(['docker','cp','-',container+':/store/run'],payload=fixture_archive())


def create_arguments(image,helper,store,reports,phase):
    owner = '0' if phase == 'writer' else '65534'
    return ['docker','create','--read-only','--network','none','--cap-drop','ALL'] + (
        ['--cap-add','CHOWN','--cap-add','DAC_READ_SEARCH'] if phase == 'writer' else []) + [
        '--user',owner+':'+owner,'--security-opt','no-new-privileges','--cpus=1','--memory=512m','--memory-swap=512m',
        '--pids-limit=128','--ulimit','core=0','--log-driver','json-file','--log-opt','max-size=1m','--log-opt','max-file=1',
        '--tmpfs','/tmp:rw,nosuid,nodev,mode=0700,uid='+owner+',gid='+owner+',size=128m',
        '--env','VIVAC_OPENSSH_DISPOSABLE=1','--mount','type=bind,source='+Path(__file__).resolve().parent.as_posix()+',target=/probe,readonly',
        '--mount','type=volume,source='+helper+',target=/launcher-build,readonly',
        '--mount','type=volume,source='+store+',target=/store'+(',readonly' if phase == 'reader' else ''),
        '--mount','type=volume,source='+reports+',target=/reports','--entrypoint','/usr/bin/timeout',image,
        '--signal=TERM','--kill-after=5s','240s','python3','-B','/probe/rust_provision_probe.py','--phase',phase,
        '--launcher-helper','/launcher-build/launcher189/debug/rust-launcher-probe','--store-root','/store/run',
        '--report','/reports/run/observations.json']


def verify_config(config,helper,store,reports,phase,image):
    def check(value):
        if not value: raise RunnerError('container configuration was refused')
    host = config['HostConfig']; owner = '0' if phase == 'writer' else '65534'
    expected = {'Memory':536870912,'MemorySwap':536870912,'NanoCpus':1000000000,'PidsLimit':128,
        'ReadonlyRootfs':True,'NetworkMode':'none','Privileged':False,'PidMode':'','IpcMode':'private'}
    check(all(host.get(key) == value for key,value in expected.items()))
    check(config['Config'].get('User') == owner+':'+owner and config['Image'] == image)
    check(host.get('CapDrop') == ['ALL'] and set(host.get('CapAdd') or []) == ({'CAP_CHOWN','CAP_DAC_READ_SEARCH'} if phase == 'writer' else set()))
    check(host.get('SecurityOpt') == ['no-new-privileges'])
    check(host.get('LogConfig') == {'Type':'json-file','Config':{'max-size':'1m','max-file':'1'}})
    check(host.get('Tmpfs') == {'/tmp':'rw,nosuid,nodev,mode=0700,uid='+owner+',gid='+owner+',size=128m'})
    check(host.get('Ulimits') == [{'Name':'core','Hard':0,'Soft':0}])
    mounts = config['Mounts']; check(len(mounts) == 4)
    mounts = {item['Destination']:item for item in mounts}
    check(set(mounts) == {'/probe','/launcher-build','/store','/reports'})
    check(mounts['/probe']['Type'] == 'bind' and not mounts['/probe']['RW'])
    check(mounts['/probe']['Source'] == Path(__file__).resolve().parent.as_posix() and mounts['/probe'].get('Propagation') == 'rprivate')
    for destination,name,writable in (('/launcher-build',helper,False),('/store',store,phase == 'writer'),('/reports',reports,True)):
        check(mounts[destination]['Type'] == 'volume' and mounts[destination].get('Name') == name and mounts[destination]['RW'] is writable)
    args = create_arguments(image,helper,store,reports,phase)
    offset = args.index('--entrypoint')
    check(config['Config'].get('Entrypoint') == [args[offset+1]] and config['Config'].get('Cmd') == args[offset+3:])
    check(not host.get('Devices') and not host.get('DeviceRequests') and not host.get('VolumesFrom') and not host.get('Links'))


def compare_phases(writer,reader):
    probe.validate_observation(writer,'writer'); probe.validate_observation(reader,'reader')
    if writer['helper_sha256'] != reader['helper_sha256'] or writer['observer_sha256'] != reader['observer_sha256']:
        raise RunnerError('phase executable identity changed')
    first = {case['case']:case['final_binding_sha256'] for case in writer['cases']}
    if any(case['final_binding_sha256'] != first[case['case']] for case in reader['cases']):
        raise RunnerError('published binding changed between phases')


def run(args):
    if sys.platform != 'win32' or not os.environ.get('RUST_WORK_JOB'): raise RunnerError('run this Windows runtime suite through RustWork --family rust')
    if not args.reports_dir.is_absolute() or args.reports_dir.exists(): raise RunnerError('select a new absolute report directory')
    args.reports_dir.mkdir(parents=True)
    report = dict.fromkeys(probe.SCOPE_FIELDS,False)
    COMMAND_RESULTS.clear()
    report.update(command_results=COMMAND_RESULTS,version=1,experiment='rust-provision',status='incomplete',phases=[],candidate_provision_writer_observed=False,candidate_provision_reader_observed=False)
    token = uuid.uuid4().hex
    store = 'vivac-provision-store-'+token
    report.update(store_volume=store, launcher_build_volume=args.launcher_build_volume)
    store_created = False
    since = datetime.datetime.now(datetime.timezone.utc).isoformat()
    baseline = (datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(minutes=5)).isoformat()
    try:
        if rdp_events(baseline): raise RunnerError('recent session errors refuse a new experiment')
        image = json.loads(command(['docker','image','inspect','--format','{{json .Id}}',args.image]))
        if type(image) is not str or not image.startswith('sha256:'): raise RunnerError('local image identity was unavailable')
        report['image_id'] = image
        command(['docker','volume','inspect',args.launcher_build_volume])
        command(['docker','volume','create',store]); store_created = True
        for phase in ('writer','reader'):
            current = {'phase':phase,'status':'incomplete','report_volume':'vivac-provision-'+phase+'-'+token}
            report['phases'].append(current)
            directory = args.reports_dir / phase; directory.mkdir()
            container = None
            try:
                command(['docker','volume','create',current['report_volume']])
                container = command(create_arguments(image,args.launcher_build_volume,store,current['report_volume'],phase)).decode('ascii').strip()
                if len(container) != 64 or any(c not in '0123456789abcdef' for c in container):
                    container = None; raise RunnerError('created container identity was unavailable')
                current['container_id'] = container
                config = json.loads(command(['docker','inspect',container]))[0]
                if config['Id'] != container: raise RunnerError('container identity changed')
                verify_config(config,args.launcher_build_volume,store,current['report_volume'],phase,image)
                current['limits_verified'] = True
                initialize(container,phase)
                if rdp_events(since): raise RunnerError('new session errors refused startup')
                command(['docker','start',container])
                deadline = time.monotonic()+300
                while True:
                    current['events'] = rdp_events(since)
                    if current['events']: raise RunnerError('new session errors interrupted experiment')
                    state = json.loads(command(['docker','inspect','--format','{{json .State}}',container]))
                    current['state'] = state
                    if not state['Running']:
                        if state.get('OOMKilled') or state['ExitCode'] != 0: raise RunnerError('experiment did not complete successfully')
                        if rdp_events(since): raise RunnerError('new session errors interrupted experiment')
                        break
                    if time.monotonic() >= deadline: raise RunnerError('experiment exceeded its total deadline')
                    time.sleep(2)
                current['status'] = 'observed'
            finally:
                if container:
                    stopped = False
                    try:
                        state = json.loads(command(['docker','inspect','--format','{{json .State}}',container]))
                        if state['Running']:
                            try: command(['docker','stop','--time','2',container],timeout=12)
                            except Exception: command(['docker','kill',container],timeout=12)
                        state = json.loads(command(['docker','inspect','--format','{{json .State}}',container]))
                        current['final_state'] = state; stopped = not state['Running']
                        if not stopped: raise RunnerError('owned container did not stop')
                    except Exception as error:
                        current['shutdown_failure'] = type(error).__name__; current['status'] = 'incomplete'
                    current['stopped'] = stopped
                    if stopped:
                        for stage,operation in (
                            ('logs',lambda: (directory/'container.log').write_bytes(command(['docker','logs','--tail','40',container],merge_stderr=True))),
                            ('reports',lambda: command(['docker','cp',container+':/reports/run/.',str(directory)])),
                            ('remove',lambda: command(['docker','rm',container]))):
                            try: operation()
                            except Exception as error:
                                current[stage+'_failure'] = type(error).__name__; current['status'] = 'incomplete'
            if current['status'] != 'observed': raise RunnerError('phase was incomplete')
            observation = json.loads((directory/'observations.json').read_text(encoding='utf-8'))
            probe.validate_observation(observation,phase)
            report['candidate_provision_'+phase+'_observed'] = True
        compare_phases(*(json.loads((args.reports_dir/phase/'observations.json').read_text(encoding='utf-8')) for phase in ('writer','reader')))
        report['status'] = 'observed'
    except Exception as error:
        report['failure'] = str(error) if isinstance(error,RunnerError) else type(error).__name__
    finally:
        if store_created and all(phase.get('stopped') is True for phase in report['phases']):
            try: command(['docker','volume','rm',store]); report['store_removed'] = True
            except Exception as error: report.update(status='incomplete',cleanup_failure=type(error).__name__)
        else: report.update(status='incomplete',store_removed=False)
        (args.reports_dir/'runner.json').write_text(json.dumps(report,sort_keys=True,indent=2)+'\n',encoding='utf-8')
    return 0 if report['status'] == 'observed' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image',required=True)
    parser.add_argument('--launcher-build-volume',required=True)
    parser.add_argument('--reports-dir',type=Path,required=True)
    return run(parser.parse_args())

if __name__ == '__main__': sys.exit(main())
