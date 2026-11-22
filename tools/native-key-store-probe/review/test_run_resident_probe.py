import copy
import unittest
import run_resident_probe as runner


class ResidentRunnerTests(unittest.TestCase):
    def test_desktop_drive_mapping_preserves_exact_bind_identity(self):
        fixture = 'H:/public-fixture/resident'
        mapped = '/run/desktop/mnt/host/h/public-fixture/resident'
        config = runner.expected_config('sha256:image','helper','real','decoy','reports','setup','resident-valid',fixture)
        def change_source(value, source):
            next(item for item in value['HostConfig']['Mounts'] if item['Target'] == '/fixture')['Source'] = source
            next(item for item in value['Mounts'] if item['Destination'] == '/fixture')['Source'] = source
        change_source(config,mapped)
        runner.verify_config(config,'helper','real','decoy','reports','setup','resident-valid','sha256:image',fixture)
        for source in ('/run/desktop/mnt/host/g/public-fixture/resident',
                       '/run/desktop/mnt/host/h/public-fixture/resident-other',
                       '/run/desktop/mnt/host/h/public-fixture/../resident',
                       '/run/desktop/mnt/host/h/public-fixture/resident/',
                       '/mnt/host/h/public-fixture/resident',
                       'h:/public-fixture/resident'):
            wrong = copy.deepcopy(config); change_source(wrong,source)
            with self.subTest(source=source), self.assertRaises(runner.RunnerError):
                runner.verify_config(wrong,'helper','real','decoy','reports','setup','resident-valid','sha256:image',fixture)
        wrong = copy.deepcopy(config)
        next(item for item in wrong['HostConfig']['Mounts'] if item['Target'] == '/fixture')['ReadOnly'] = False
        with self.assertRaises(runner.RunnerError): runner.verify_config(wrong,'helper','real','decoy','reports','setup','resident-valid','sha256:image',fixture)

    def test_exact_configs_roles_and_fixture_mount(self):
        for phase in runner.LIMITS:
            config = runner.expected_config('sha256:image','helper','real','decoy','reports',phase,'record-in-place','/public-fixture','a'*32)
            runner.verify_config(config,'helper','real','decoy','reports',phase,'record-in-place','sha256:image','/public-fixture','a'*32)
            self.assertEqual(config['Config']['User'], '0:0' if phase == 'setup' else '65534:65534')
            self.assertEqual(config['HostConfig']['CapAdd'], ['CAP_CHOWN','CAP_DAC_READ_SEARCH'] if phase == 'setup' else [])
            fixture = next(item for item in config['HostConfig']['Mounts'] if item['Target'] == '/fixture')
            self.assertTrue(fixture['ReadOnly'])
            for key,value in [('Privileged',True),('MemorySwap',0),('CapAdd',['CAP_SYS_ADMIN'])]:
                wrong = copy.deepcopy(config); wrong['HostConfig'][key] = value
                with self.assertRaises(runner.RunnerError): runner.verify_config(wrong,'helper','real','decoy','reports',phase,'record-in-place','sha256:image','/public-fixture','a'*32)

    def test_same_volume_mounts_and_parallel_budget(self):
        for case,subpath in runner.probe.MOUNT_PATHS.items():
            specs = runner.mount_specs('helper','real','decoy','reports','reader',case,'/public-fixture')
            overlay = specs[-1]
            self.assertEqual(overlay['Source'], 'real'); self.assertTrue(overlay['ReadOnly'])
            self.assertEqual(overlay['VolumeOptions']['Subpath'], subpath)
            self.assertEqual(overlay['Target'], '/var/lib/vivac/'+subpath)
        self.assertEqual(tuple(a+b for a,b in zip(runner.LIMITS['reader'],runner.LIMITS['mutate'])), (448,112,1,112))

    def test_configuration_refuses_fixture_write_alias_and_mount_substitution(self):
        config = runner.expected_config('sha256:image','helper','real','decoy','reports','reader','mount-key-leaf','/public-fixture')
        changes = []
        wrong = copy.deepcopy(config); wrong['HostConfig']['Mounts'][-1]['VolumeOptions']['Subpath'] = 'custody-v1'; changes.append(wrong)
        wrong = copy.deepcopy(config); wrong['Mounts'].append(dict(Destination='/alias',Type='volume',Name='real',RW=True)); changes.append(wrong)
        wrong = copy.deepcopy(config); next(item for item in wrong['HostConfig']['Mounts'] if item['Target'] == '/fixture')['ReadOnly'] = False; changes.append(wrong)
        wrong = copy.deepcopy(config); wrong['Config']['User'] = '0:0'; changes.append(wrong)
        for wrong in changes:
            with self.assertRaises(runner.RunnerError): runner.verify_config(wrong,'helper','real','decoy','reports','reader','mount-key-leaf','sha256:image','/public-fixture')

    def test_request_archive_cannot_replay_another_case_or_link(self):
        import io
        import json
        import tarfile
        request = dict(version=1,case='record-in-place',request_token='a'*32,reader_host_pid=9,reader_starttime=10)
        def archive(kind=tarfile.REGTYPE, owner=65534):
            out = io.BytesIO()
            with tarfile.open(fileobj=out,mode='w') as stream:
                runner.archive_entry(stream,'request.json',0o600,owner,kind=kind,data=json.dumps(request).encode('ascii') if kind == tarfile.REGTYPE else b'')
            return out.getvalue()
        self.assertEqual(runner.parse_request_archive(archive(),'record-in-place'),request)
        for data,case in [(archive(),'key-in-place'),(archive(owner=0),'record-in-place'),(archive(kind=tarfile.SYMTYPE),'record-in-place')]:
            with self.assertRaises((ValueError,runner.RunnerError)): runner.parse_request_archive(data,case)


if __name__ == '__main__': unittest.main()
