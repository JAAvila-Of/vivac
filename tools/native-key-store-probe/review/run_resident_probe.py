"""Run one disposable resident snapshot case under a Windows RustWork lease."""
import argparse
import datetime
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import threading
import time
import uuid

import run_lifecycle_probe as base
import rust_resident_probe as probe

RunnerError=base.RunnerError
REVIEW=Path(__file__).resolve().parent
LIMITS={'setup':(512,128,1,128),'reader':(320,96,.75,96),'mutate':(128,16,.25,16)}
COMMAND_RESULTS=[]
rdp_events=base.rdp_events


def check(value):
    if not value: raise RunnerError('resident snapshot evidence was refused')


def command(arguments,timeout=15,payload=None,merge_stderr=False,optional=False):
    process=subprocess.Popen(arguments,stdin=subprocess.DEVNULL if payload is None else subprocess.PIPE,
        stdout=subprocess.PIPE,stderr=subprocess.STDOUT if merge_stderr else subprocess.PIPE)
    output=bytearray();errors=bytearray();failures=[]
    def read(stream,destination):
        try:
            while True:
                part=stream.read(4096)
                if not part:return
                if len(destination)+len(part) > 131072:
                    failures.append('output-bound');process.kill();return
                destination.extend(part)
        except Exception:failures.append('read')
    def write():
        try:process.stdin.write(payload);process.stdin.close()
        except Exception:failures.append('write')
    workers=[threading.Thread(target=read,args=(process.stdout,output),daemon=True)]
    if process.stderr is not None:workers.append(threading.Thread(target=read,args=(process.stderr,errors),daemon=True))
    if payload is not None:workers.append(threading.Thread(target=write,daemon=True))
    for worker in workers:worker.start()
    try:
        code=process.wait(timeout=timeout)
        for worker in workers:worker.join(timeout=2)
        check(not any(worker.is_alive() for worker in workers) and not failures)
    finally:
        if process.poll() is None:process.kill();process.wait(timeout=3)
        for stream in (process.stdout,process.stderr,process.stdin):
            if stream is not None:stream.close()
        COMMAND_RESULTS.append({'command':arguments[:2],'exit_code':process.returncode})
    if code:
        if optional:return None
        raise RunnerError('control command failed: '+arguments[0]+' exit='+str(code))
    return bytes(output)


def archive_entry(archive,name,mode,owner=0,*,kind=tarfile.DIRTYPE,data=b''):
    item=tarfile.TarInfo(name); item.mode=mode; item.uid=item.gid=owner; item.type=kind; item.size=len(data)
    archive.addfile(item,io.BytesIO(data) if data else None)


def copy_entries(container,target,entries):
    output=io.BytesIO()
    with tarfile.open(fileobj=output,mode='w') as archive:
        for name,mode,owner,data in entries:
            archive_entry(archive,name,mode,owner,kind=tarfile.DIRTYPE if data is None else tarfile.REGTYPE,data=b'' if data is None else data)
    command(['docker','cp','-',container+':'+target],payload=output.getvalue())


def initialize(container,phase,setup=None):
    owner=0 if phase == 'setup' else 65534
    copy_entries(container,'/reports',[('run',0o700,owner,None)])
    if phase == 'setup':
        copy_entries(container,'/var/lib/vivac',[('.',0o755,0,None)])
        copy_entries(container,'/decoy',[(name,0o755,0,None) for name in ('.','var','var/lib','var/lib/vivac')])
    if phase == 'reader':
        check(type(setup) is bytes and len(setup) <= 16384)
        copy_entries(container,'/reports/run',[('setup.json',0o600,65534,setup)])


def mount_specs(helper,real,decoy,reports,phase,case,fixture):
    specs=[dict(Type='bind',Source=REVIEW.as_posix(),Target='/probe',ReadOnly=True),
        dict(Type='volume',Source=helper,Target='/launcher-build',ReadOnly=True,VolumeOptions={'NoCopy':True}),
        dict(Type='volume',Source=real,Target='/var/lib/vivac',ReadOnly=phase == 'reader',VolumeOptions={'NoCopy':True})]
    if phase != 'mutate':specs.append(dict(Type='volume',Source=decoy,Target='/decoy',ReadOnly=phase == 'reader',VolumeOptions={'NoCopy':True}))
    specs.append(dict(Type='volume',Source=reports,Target='/reports',ReadOnly=False,VolumeOptions={'NoCopy':True}))
    specs.append(dict(Type='bind',Source=Path(fixture).as_posix(),Target='/fixture',ReadOnly=True))
    if phase == 'reader' and case.startswith('mount-'):
        subpath=probe.MOUNT_PATHS[case]
        specs.append(dict(Type='volume',Source=real,Target='/var/lib/vivac/'+subpath,ReadOnly=True,
            VolumeOptions={'NoCopy':True,'Subpath':subpath}))
    return specs


def create_arguments(image,helper,real,decoy,reports,phase,case,fixture,token=None):
    check(phase in LIMITS and case in probe.CASES and (phase != 'mutate' or case in probe.MUTATIONS and probe.hexadecimal(token,32)))
    memory,pids,cpu,tmp=LIMITS[phase]; owner='0' if phase == 'setup' else '65534'
    result=['docker','create','--read-only','--network','none','--cap-drop','ALL']
    if phase == 'setup': result+=['--cap-add','CHOWN','--cap-add','DAC_READ_SEARCH']
    result+=['--user',owner+':'+owner,'--security-opt','no-new-privileges','--cpus='+str(cpu),
        '--memory='+str(memory)+'m','--memory-swap='+str(memory)+'m','--pids-limit='+str(pids),'--ulimit','core=0',
        '--log-driver','json-file','--log-opt','max-size=1m','--log-opt','max-file=1',
        '--tmpfs','/tmp:rw,nosuid,nodev,mode=0700,uid='+owner+',gid='+owner+',size='+str(tmp)+'m',
        '--env','VIVAC_OPENSSH_DISPOSABLE=1']
    for item in mount_specs(helper,real,decoy,reports,phase,case,fixture):
        specification='type='+item['Type']+',source='+item['Source']+',target='+item['Target']
        if item['ReadOnly']:specification+=',readonly'
        if item['Type'] == 'volume':
            specification+=',volume-nocopy'
            if 'Subpath' in item['VolumeOptions']:specification+=',volume-subpath='+item['VolumeOptions']['Subpath']
        result+=['--mount',specification]
    result+=['--entrypoint','/usr/bin/timeout',image,'--signal=TERM','--kill-after=5s','30s' if phase == 'mutate' else '120s',
        'python3','-B','/probe/rust_resident_probe.py','--phase',phase,'--case',case,
        '--launcher-helper','/launcher-build/launcher189/debug/rust-launcher-probe','--report','/reports/run/observations.json']
    if phase == 'reader':result+=['--expected','/reports/run/setup.json']
    if phase == 'mutate':result+=['--request-token',token]
    return result


def expected_config(image,helper,real,decoy,reports,phase,case,fixture,token=None):
    memory,pids,cpu,tmp=LIMITS[phase]; owner='0' if phase == 'setup' else '65534'
    args=create_arguments(image,helper,real,decoy,reports,phase,case,fixture,token); offset=args.index('--entrypoint')
    specs=mount_specs(helper,real,decoy,reports,phase,case,fixture)
    mounts=[]
    for item in specs:
        selected=dict(Type=item['Type'],Destination=item['Target'],RW=not item['ReadOnly'])
        if item['Type'] == 'bind':selected.update(Source=item['Source'],Propagation='rprivate')
        else:selected['Name']=item['Source']
        mounts.append(selected)
    return dict(Image=image,Config=dict(User=owner+':'+owner,Entrypoint=['/usr/bin/timeout'],Cmd=args[offset+3:],
        Env=['PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin','VIVAC_OPENSSH_DISPOSABLE=1']),
        HostConfig=dict(Memory=memory*1048576,MemorySwap=memory*1048576,NanoCpus=int(cpu*1000000000),PidsLimit=pids,
            ReadonlyRootfs=True,NetworkMode='none',Privileged=False,PidMode='',IpcMode='private',CapDrop=['ALL'],
            CapAdd=['CAP_CHOWN','CAP_DAC_READ_SEARCH'] if phase == 'setup' else [],SecurityOpt=['no-new-privileges'],
            LogConfig={'Type':'json-file','Config':{'max-size':'1m','max-file':'1'}},
            Tmpfs={'/tmp':'rw,nosuid,nodev,mode=0700,uid='+owner+',gid='+owner+',size='+str(tmp)+'m'},
            Ulimits=[{'Name':'core','Hard':0,'Soft':0}],Mounts=specs),Mounts=mounts)


def same_bind_source(actual, expected):
    if actual == expected: return True
    if type(actual) is not str or type(expected) is not str: return False
    if len(expected) < 4 or expected[0] not in 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz' or expected[1:3] != ':/': return False
    components = expected[3:].split('/')
    if any(component in {'','.','..'} for component in components): return False
    return actual == '/run/desktop/mnt/host/'+expected[0].lower()+'/'+ '/'.join(components)


def verify_config(config,helper,real,decoy,reports,phase,case,image,fixture,token=None):
    expected=expected_config(image,helper,real,decoy,reports,phase,case,fixture,token)
    check(config.get('Image') == image)
    for key,value in expected['Config'].items():
        if key == 'Env':
            actual=config['Config'].get(key)
            check(type(actual) is list and all(type(item) is str and '=' in item for item in actual))
            entries=[item.split('=',1) for item in actual]
            check(len({name for name,_ in entries}) == len(entries))
            check(dict(entries) == dict(item.split('=',1) for item in value))
        else:check(config['Config'].get(key) == value)
    host=config['HostConfig']
    for key,value in expected['HostConfig'].items():
        if key == 'CapAdd':check(set(host.get(key) or []) == set(value))
        elif key == 'Mounts':
            actual=host.get(key); check(type(actual) is list and len(actual) == len(value))
            check(len({item.get('Target') for item in actual}) == len(value))
            actual={item['Target']:item for item in actual}
            for item in value:
                found=actual.get(item['Target']);check(type(found) is dict)
                for field in ('Type','Source','Target','ReadOnly'):
                    if field == 'Source' and item['Type'] == 'bind': check(same_bind_source(found.get(field),item[field]))
                    else: check(found.get(field,False if field == 'ReadOnly' else None) == item[field])
                options=found.get('VolumeOptions')
                if item['Type'] == 'volume':
                    check(type(options) is dict and set(options) <= set(item['VolumeOptions']) | {'DriverConfig'})
                    check(options.get('NoCopy') is True)
                    check(options.get('DriverConfig') is None or type(options.get('DriverConfig')) is dict and not options['DriverConfig'])
                    check({key:value for key,value in options.items() if key != 'DriverConfig'} == item['VolumeOptions'])
                else:check(options is None)
                check(not found.get('TmpfsOptions') and not found.get('ImageOptions'))
        else:check(host.get(key) == value)
    for field in ('Devices','DeviceRequests','VolumesFrom','Links','ExtraHosts','PortBindings','Binds','GroupAdd','DeviceCgroupRules'):
        check(not host.get(field))
    mounts=config['Mounts'];check(type(mounts) is list and len(mounts) == len(expected['Mounts']))
    check(len({item.get('Destination') for item in mounts}) == len(mounts))
    actual={item['Destination']:item for item in mounts}
    for item in expected['Mounts']:
        check(item['Destination'] in actual)
        for key,value in item.items():
            if key == 'Source' and item['Type'] == 'bind': check(same_bind_source(actual[item['Destination']].get(key),value))
            else: check(actual[item['Destination']].get(key) == value)


def parse_request_archive(data,case):
    check(type(data) is bytes and 0 < len(data) <= 32768)
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:') as archive:
        members=archive.getmembers();check(len(members) == 1)
        item=members[0]
        check(item.name == 'request.json' and item.type in (tarfile.REGTYPE,tarfile.AREGTYPE)
            and item.uid == item.gid == 65534 and item.mode == 0o600 and 0 < item.size <= 512 and not item.linkname
            and item.sparse is None and not any('sparse' in key.lower() for key in item.pax_headers))
        if 'SCHILY.nlink' in item.pax_headers:check(item.pax_headers['SCHILY.nlink'] == '1')
        stream=archive.extractfile(item);check(stream is not None)
        value=probe.abandon.decode(stream.read(513))
        probe.validate_request(value,case)
        return value


def compare_phases(setup,reader,mutation=None):
    probe.validate_observation(setup,'setup');probe.validate_observation(reader,'reader')
    case=setup['case'];check(reader['case'] == case)
    for field in ('helper_sha256','observer_sha256','kernel_release'):check(setup[field] == reader[field])
    a,b=setup['result']['fixed']['result'],reader['result']
    check(b['before_after']['before'] == setup['result']['fixture_evidence'])
    check(a['decoy_binding_sha256'] == b['decoy_binding_sha256'])
    check(b['selected_installation_id'] == (a['real_installation_id'] if case not in probe.NEGATIVES else None))
    if case in probe.MUTATIONS:
        check(mutation is not None);probe.validate_observation(mutation,'mutate',case)
        for field in ('helper_sha256','observer_sha256','kernel_release'):check(mutation[field] == setup[field])
        check(mutation['result']['before'] == b['before_after']['before'])
        check(mutation['result']['after'] == b['before_after']['after'])
    else:check(mutation is None)
    check(a['real_binding_sha256'] == b['real_binding_sha256'])


def run(args):
    check(sys.platform == 'win32' and bool(os.environ.get('RUST_WORK_JOB')))
    check(args.case in probe.CASES and args.reports_dir.is_absolute() and not args.reports_dir.exists())
    check(args.fixture_dir.is_absolute() and args.fixture_dir.is_dir() and not args.fixture_dir.is_symlink())
    check({item.name for item in args.fixture_dir.iterdir()} == {'key.openssh','manifest.json'})
    for name,limit in [('key.openssh',65536),('manifest.json',2048)]:
        item=args.fixture_dir/name
        check(item.is_file() and not item.is_symlink() and 0 < item.stat().st_size <= limit)
    args.reports_dir.mkdir(parents=True)
    token=uuid.uuid4().hex
    real='vivac-resident-real-'+token;decoy='vivac-resident-decoy-'+token
    COMMAND_RESULTS.clear()
    report=dict.fromkeys(probe.SCOPE_FIELDS,False)
    report.update(version=1,experiment='rust-resident',case=args.case,status='incomplete',candidate_resident_snapshot_observed=False,
        phases=[],real_volume=real,decoy_volume=decoy,launcher_build_volume=args.launcher_build_volume,command_results=COMMAND_RESULTS)
    created=[];owned={};observations={};requests=[]
    since=datetime.datetime.now(datetime.timezone.utc).isoformat()
    baseline=(datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(minutes=5)).isoformat()

    def state(container):return json.loads(command(['docker','inspect','--format','{{json .State}}',container]))
    def events():
        found=rdp_events(since)
        if found:report['rdp_events']=found;raise RunnerError('new session errors interrupted experiment')
    def cleanup(current):
        container=current['container_id'];directory=args.reports_dir/current['phase']
        try:
            status=state(container)
            if status['Running']:
                try:command(['docker','stop','--time','2',container],timeout=12)
                except Exception:command(['docker','kill',container],timeout=12)
            status=state(container);current['final_state']=status;check(not status['Running']);current['stopped']=True
            for name,operation in [('logs',lambda:(directory/'container.log').write_bytes(command(['docker','logs','--tail','40',container],merge_stderr=True))),
                ('reports',lambda:command(['docker','cp',container+':/reports/run/.',str(directory)])),
                ('remove',lambda:command(['docker','rm',container]))]:
                try:operation()
                except Exception as error:current[name+'_failure']=type(error).__name__;current['status']='incomplete'
            check(current['status'] == 'observed')
        except Exception:
            current['status']='incomplete';raise

    def launch(phase,request=None):
        events()
        current=dict(phase=phase,status='incomplete',report_volume='vivac-resident-'+phase+'-'+token)
        report['phases'].append(current);directory=args.reports_dir/phase;directory.mkdir()
        command(['docker','volume','create',current['report_volume']])
        nonce=None if request is None else request['request_token']
        container=command(create_arguments(report['image_id'],args.launcher_build_volume,real,decoy,current['report_volume'],phase,args.case,args.fixture_dir,nonce)).decode('ascii').strip()
        check(probe.hexadecimal(container,64));current['container_id']=container;owned[phase]=current
        config=json.loads(command(['docker','inspect',container]))[0];check(config['Id'] == container)
        verify_config(config,args.launcher_build_volume,real,decoy,current['report_volume'],phase,args.case,report['image_id'],args.fixture_dir,nonce)
        current['limits_verified']=True
        initialize(container,phase,None if phase != 'reader' else json.dumps(observations['setup'],sort_keys=True).encode('ascii'))
        (directory/'config.json').write_text(json.dumps(config,sort_keys=True,indent=2)+'\n',encoding='utf-8')
        events();command(['docker','start',container]);current['started']=True
        return current

    def wait(current,deadline):
        container=current['container_id']
        while True:
            events();current['state']=status=state(container)
            if not status['Running']:
                check(not status.get('OOMKilled') and type(status['ExitCode']) is int and status['ExitCode'] == 0)
                current['status']='observed';cleanup(current)
                observation=json.loads((args.reports_dir/current['phase']/'observations.json').read_text(encoding='utf-8'))
                probe.validate_observation(observation,current['phase'],args.case);observations[current['phase']]=observation
                return
            check(time.monotonic() < deadline)
            time.sleep(.25)

    try:
        check(not rdp_events(baseline))
        image=json.loads(command(['docker','image','inspect','--format','{{json .Id}}',args.image]))
        check(type(image) is str and image.startswith('sha256:') and probe.hexadecimal(image[7:],64));report['image_id']=image
        command(['docker','volume','inspect',args.launcher_build_volume])
        for volume in (real,decoy):
            check(command(['docker','volume','inspect',volume],optional=True) is None)
            command(['docker','volume','create',volume]);created.append(volume)
        setup=launch('setup');wait(setup,time.monotonic()+150)
        check(setup.get('stopped') is True and setup['status'] == 'observed')
        current=launch('reader');deadline=time.monotonic()+150
        if args.case in probe.MUTATIONS:
            request=None
            while request is None:
                events();status=state(current['container_id']);check(status['Running'] and time.monotonic() < deadline)
                data=command(['docker','cp',current['container_id']+':/reports/run/request.json','-'],optional=True)
                if data is not None:request=parse_request_archive(data,args.case)
                else:time.sleep(.25)
            check(request['request_token'] not in requests);requests.append(request['request_token']);current['request']=request
            mutator=launch('mutate',request);wait(mutator,min(deadline,time.monotonic()+40))
            mutation=observations['mutate'];probe.validate_mutation(mutation['result'],args.case,request['request_token'])
            check(mutation['result']['before'] == observations['setup']['result']['fixture_evidence'])
            events();check(state(current['container_id'])['Running'])
            ack=dict(version=1,case=args.case,request_token=request['request_token'],status='observed')
            copy_entries(current['container_id'],'/reports/run',[('ack.json',0o600,65534,json.dumps(ack,separators=(',',':')).encode('ascii'))])
        wait(current,deadline)
        compare_phases(observations['setup'],observations['reader'],observations.get('mutate'))
        events();report.update(status='observed',candidate_resident_snapshot_observed=True)
    except Exception as error:
        report['failure']=str(error) if isinstance(error,RunnerError) else type(error).__name__
    finally:
        for current in owned.values():
            if current.get('stopped') is not True:
                try:cleanup(current)
                except Exception as error:current['cleanup_failure']=type(error).__name__;report['status']='incomplete'
        if all(current.get('stopped') is True for current in owned.values()):
            for volume in created:
                try:command(['docker','volume','rm',volume])
                except Exception as error:report.update(status='incomplete',cleanup_failure=type(error).__name__)
            report['stores_removed']=len(created) == 2 and 'cleanup_failure' not in report
        else:report.update(status='incomplete',stores_removed=False)
        if report['status'] != 'observed':report['candidate_resident_snapshot_observed']=False
        (args.reports_dir/'runner.json').write_text(json.dumps(report,sort_keys=True,indent=2)+'\n',encoding='utf-8')
    return 0 if report['status'] == 'observed' else 1


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case',choices=sorted(probe.CASES),required=True)
    parser.add_argument('--image',required=True)
    parser.add_argument('--launcher-build-volume',required=True)
    parser.add_argument('--reports-dir',type=Path,required=True)
    parser.add_argument('--fixture-dir',type=Path,required=True)
    return run(parser.parse_args())


if __name__ == '__main__':sys.exit(main())
