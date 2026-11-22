import copy
import unittest
from pathlib import Path
from unittest.mock import patch
import run_provision_probe as runner


def config(phase='writer'):
    owner = '0' if phase == 'writer' else '65534'
    args = runner.create_arguments('sha256:image','helper','store','reports',phase)
    offset = args.index('--entrypoint')
    return dict(Image='sha256:image',Config=dict(User=owner+':'+owner,Entrypoint=['/usr/bin/timeout'],Cmd=args[offset+3:]),
        HostConfig=dict(Memory=536870912,MemorySwap=536870912,NanoCpus=1000000000,PidsLimit=128,ReadonlyRootfs=True,
            NetworkMode='none',Privileged=False,PidMode='',IpcMode='private',CapDrop=['ALL'],
            CapAdd=['CAP_CHOWN','CAP_DAC_READ_SEARCH'] if phase == 'writer' else [],SecurityOpt=['no-new-privileges'],
            LogConfig=dict(Type='json-file',Config={'max-size':'1m','max-file':'1'}),Ulimits=[dict(Name='core',Hard=0,Soft=0)],
            Tmpfs={'/tmp':'rw,nosuid,nodev,mode=0700,uid='+owner+',gid='+owner+',size=128m'}),
        Mounts=[dict(Destination='/probe',Type='bind',RW=False,Source=Path(runner.__file__).resolve().parent.as_posix(),Propagation='rprivate')]+[
            dict(Destination=destination,Type='volume',RW=writable,Name=name) for destination,name,writable in (
                ('/launcher-build','helper',False),('/store','store',phase == 'writer'),('/reports','reports',True))])


class ConfigTests(unittest.TestCase):
    def test_exact_source_capabilities_mounts_and_deadline_required(self):
        for phase in ('writer','reader'):
            value = config(phase)
            runner.verify_config(value,'helper','store','reports',phase,'sha256:image')
            changes = []
            for field,replacement in (('CapAdd',['CAP_SYS_ADMIN']),('MemorySwap',1073741824),('PidsLimit',129),('Privileged',True),('IpcMode','host')):
                wrong = copy.deepcopy(value); wrong['HostConfig'][field] = replacement; changes.append(wrong)
            wrong = copy.deepcopy(value); wrong['Mounts'][0]['Source'] = '/elsewhere/vivac/tools/native-key-store-probe/review'; changes.append(wrong)
            wrong = copy.deepcopy(value); wrong['Mounts'][1]['RW'] = True; changes.append(wrong)
            wrong = copy.deepcopy(value); wrong['Config']['Cmd'][2] = '999s'; changes.append(wrong)
            wrong = copy.deepcopy(value); wrong['Mounts'].append(dict(Destination='/extra',Type='bind',RW=False)); changes.append(wrong)
            for wrong in changes:
                with self.subTest(phase=phase,wrong=wrong),self.assertRaises(runner.RunnerError):
                    runner.verify_config(wrong,'helper','store','reports',phase,'sha256:image')

    def test_initializers_never_reseed_reader_store_or_use_archive_flag(self):
        with patch.object(runner,'command') as command:
            runner.initialize('a'*64,'reader')
        self.assertEqual(command.call_count,1)
        self.assertEqual(command.call_args.args[0],['docker','cp','-','a'*64+':/reports'])
        with patch.object(runner,'command') as command:
            runner.initialize('a'*64,'writer')
        self.assertEqual(command.call_count,3)
        for call in command.call_args_list: self.assertNotIn('--archive',call.args[0])

if __name__ == '__main__': unittest.main()
