import copy
import unittest
import run_fixed_root_probe as runner


class RunnerTests(unittest.TestCase):
    def test_docker_environment_order_and_empty_driver_metadata(self):
        for driver in ('absent',None,{}):
            config=runner.expected_config('sha256:image','helper','real','decoy','reports','reader','mount-users')
            config['Config']['Env'].reverse()
            for item in config['HostConfig']['Mounts']:
                if item['Type'] == 'volume' and driver != 'absent':item['VolumeOptions']['DriverConfig']=driver
            runner.verify_config(config,'helper','real','decoy','reports','reader','mount-users','sha256:image')
        for change in ('duplicate','new','changed','driver-name','driver-options','unknown-volume-option'):
            config=runner.expected_config('sha256:image','helper','real','decoy','reports','reader','mount-users')
            if change == 'duplicate':config['Config']['Env'].append(config['Config']['Env'][0])
            elif change == 'new':config['Config']['Env'].append('HOME=/decoy')
            elif change == 'changed':config['Config']['Env'][0]='PATH=/decoy'
            elif change == 'driver-name':config['HostConfig']['Mounts'][1]['VolumeOptions']['DriverConfig']={'Name':'local'}
            elif change == 'driver-options':config['HostConfig']['Mounts'][1]['VolumeOptions']['DriverConfig']={'Options':{}}
            else:config['HostConfig']['Mounts'][1]['VolumeOptions']['Unknown']=True
            with self.subTest(change=change),self.assertRaises(runner.RunnerError):
                runner.verify_config(config,'helper','real','decoy','reports','reader','mount-users','sha256:image')

    def test_control_output_is_bounded_and_failure_exit_is_retained(self):
        import sys
        self.assertEqual(runner.command([sys.executable,'-c','print("bounded")']),b'bounded\r\n' if sys.platform == 'win32' else b'bounded\n')
        self.assertIsNone(runner.command([sys.executable,'-c','raise SystemExit(3)'],optional=True))
        self.assertEqual(runner.COMMAND_RESULTS[-1]['exit_code'],3)
        with self.assertRaises(runner.RunnerError):runner.command([sys.executable,'-c','import sys;sys.stdout.write("x"*131073)'])

    def test_reader_mutator_budget_stays_below_parallel_ceiling(self):
        reader=runner.LIMITS['reader']; mutate=runner.LIMITS['mutate']
        self.assertEqual(tuple(a+b for a,b in zip(reader,mutate)),(448,112,1,112))

    def test_config_rejects_rw_alias_and_wrong_volume_subpath(self):
        for phase in ('setup','reader','mutate'):
            case='binding-mode' if phase == 'mutate' else 'mount-users'
            config=runner.expected_config('sha256:image','helper','real','decoy','reports',phase,case,'a'*32)
            runner.verify_config(config,'helper','real','decoy','reports',phase,case,'sha256:image','a'*32)
            variants=[]
            for field,value in [('Privileged',True),('MemorySwap',1073741824),('CapAdd',['CAP_SYS_ADMIN'])]:
                wrong=copy.deepcopy(config);wrong['HostConfig'][field]=value; variants.append(wrong)
            wrong=copy.deepcopy(config);wrong['Mounts'].append(dict(Destination='/alias',Type='volume',Name='real',RW=True));variants.append(wrong)
            wrong=copy.deepcopy(config);wrong['Mounts'][0]['Source']='/somewhere/review';variants.append(wrong)
            if phase == 'reader':
                wrong=copy.deepcopy(config);wrong['HostConfig']['Mounts'][-1]['VolumeOptions']['Subpath']='custody-v1';variants.append(wrong)
                wrong=copy.deepcopy(config);wrong['Mounts'][2]['RW']=True;variants.append(wrong)
            for wrong in variants:
                with self.subTest(phase=phase),self.assertRaises(runner.RunnerError):
                    runner.verify_config(wrong,'helper','real','decoy','reports',phase,case,'sha256:image','a'*32)

    def test_request_tar_refuses_links_extra_members_and_wrong_owner(self):
        import io,json,tarfile
        request=dict(version=1,case='binding-mode',request_token='a'*32,reader_host_pid=9,reader_starttime=10)
        def archive(owner=65534,kind=tarfile.REGTYPE,extra=False):
            out=io.BytesIO()
            with tarfile.open(fileobj=out,mode='w') as tar:
                for name in (['request.json','other'] if extra else ['request.json']):
                    runner.archive_entry(tar,name,0o600,owner,kind=kind,data=json.dumps(request).encode('ascii') if kind == tarfile.REGTYPE else b'')
            return out.getvalue()
        self.assertEqual(runner.parse_request_archive(archive(),'binding-mode'),request)
        for data in (archive(owner=0),archive(kind=tarfile.SYMTYPE),archive(extra=True)):
            with self.assertRaises((ValueError,runner.RunnerError)):runner.parse_request_archive(data,'binding-mode')


if __name__ == '__main__':unittest.main()
