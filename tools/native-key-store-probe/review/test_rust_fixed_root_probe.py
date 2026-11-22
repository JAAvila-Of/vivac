import copy
import unittest
from unittest.mock import patch
import rust_fixed_root_probe as probe


class EvidenceTests(unittest.TestCase):
    def test_installation_identifier_uses_only_canonical_lowercase_crockford(self):
        self.assertTrue(probe.installation('7'+'z'*25))
        for character in '0123456789abcdefghjkmnpqrstvwxyz':
            self.assertTrue(probe.installation('0'+character*25))
        for character in 'ABCDEFGHIJKLMNOPQRSTUVWXYZilou':
            self.assertFalse(probe.installation('0'+character*25))
        for value in ('8'+'z'*25,'0'*25,'0'*27,'0'*25+'\n'):
            self.assertFalse(probe.installation(value))
        value=evidence('fixed-valid','setup')
        value.update(real_installation_id='0'+'a'*25,decoy_installation_id='7'+'z'*25)
        probe.validate_result(value,'fixed-valid','setup')
        value['decoy_installation_id']=value['real_installation_id']
        with self.assertRaises(ValueError):probe.validate_result(value,'fixed-valid','setup')

    def test_failure_diagnostic_exposes_only_allowlisted_metadata(self):
        value=probe.failure_diagnostic(ValueError('/private/path bytes'),dict(case='fixed-valid',phase='setup',stage='action-validation',path='/private'))
        self.assertEqual(value,dict(kind='ValueError',case='fixed-valid',phase='setup',stage='action-validation'))
        value=probe.failure_diagnostic(RuntimeError('private'),dict(case='/private',phase='outside',stage='/private'))
        self.assertEqual(value,dict(kind='Exception',case='none',phase='none',stage='unknown'))
        self.assertNotIn('private',str(value))

    def test_canonical_public_binding_rejects_aliases_and_boolean_integer(self):
        data = b'{"version":1,"installation_id":"00000000000000000000000001","owner_uid":65534,"owner_gid":65534,"profile_revision":1}'
        self.assertEqual(probe.parse_binding(data), '00000000000000000000000001')
        for wrong in (data+b'\n', data.replace(b'"version":1',b'"version":true'),data.replace(b'65534',b'65535',1)):
            with self.assertRaises(ValueError): probe.parse_binding(wrong)

    def test_request_identity_and_nonce_are_bound(self):
        value = dict(version=1,case='binding-mode',request_token='a'*32,reader_host_pid=9,reader_starttime=10)
        probe.validate_request(value,'binding-mode')
        for field,replacement in [('reader_host_pid',True),('request_token','../path'),('case','fixed-valid'),('path','/outside')]:
            wrong = dict(value); wrong[field] = replacement
            with self.assertRaises(ValueError): probe.validate_request(wrong,'binding-mode')

    def test_mutation_evidence_requires_real_object_and_operation(self):
        before = dict(device=1,inode=2,uid=0,gid=0,mode=420,nlink=1,size=128,sha256='a'*64)
        after = dict(before,mode=384)
        probe.validate_mutation(dict(version=1,case='binding-mode',request_token='a'*32,status='observed',operation='chmod',before=before,after=after),'binding-mode','a'*32)
        for field,value in [('inode',3),('sha256','b'*64),('uid',True)]:
            wrong = dict(after); wrong[field]=value
            with self.assertRaises(ValueError): probe.validate_mutation(dict(version=1,case='binding-mode',request_token='a'*32,status='observed',operation='chmod',before=before,after=wrong),'binding-mode','a'*32)

    def test_same_filesystem_mount_requires_distinct_mount_identity(self):
        raw='10 1 8:1 / /var/lib/vivac ro - ext4 /dev/sda ro\n11 10 8:1 /custody-v1/users /var/lib/vivac/custody-v1/users ro - ext4 /dev/sda ro\n'
        self.assertEqual(probe.mount_identity(raw,'/var/lib/vivac/custody-v1/users'),(11,10))
        with self.assertRaises(ValueError): probe.mount_identity(raw.replace('11 10','10 10'),'/var/lib/vivac/custody-v1/users')

    def test_all_result_stages_reject_boolean_counters_and_incomplete_claims(self):
        for case in probe.CASES:
            for phase in ('setup','reader'):
                for observation in (True,False):
                    held_values=(False,True) if phase == 'reader' and observation else (False,)
                    for held in held_values:
                        value=evidence(case,phase,observation,held)
                        probe.validate_result(value,case,phase,observation=observation,held=held)
                        for field in ('version','pressure_threads','pressure_iterations')+(('real_publications','decoy_publications') if phase == 'setup' else ()):
                            wrong=dict(value);wrong[field]=bool(value[field])
                            with self.subTest(case=case,phase=phase,field=field),self.assertRaises(ValueError):
                                probe.validate_result(wrong,case,phase,observation=observation,held=held)
                        for field in probe.SCOPE_FIELDS:
                            wrong=dict(value);wrong[field]=True
                            with self.assertRaises(ValueError):probe.validate_result(wrong,case,phase,observation=observation,held=held)

    def test_phase_resources_and_mutator_credentials_have_strict_upper_bounds(self):
        for phase,(memory,pids,cpu,tmp) in {'setup':(512,128,100000,128),'reader':(320,96,75000,96),'mutate':(128,16,25000,16)}.items():
            value=dict(memory_bytes=memory*1048576,swap_bytes=0,pids=pids,cpu_quota=cpu,cpu_period=100000,temporary_bytes=tmp*1048576)
            probe.validate_limits(value,phase)
            for field in ('memory_bytes','swap_bytes','pids','cpu_quota','temporary_bytes'):
                wrong=dict(value);wrong[field]+=1
                with self.assertRaises(ValueError):probe.validate_limits(wrong,phase)
        fields=dict(Uid=[0]*4,Gid=[0]*4,NoNewPrivs=[1],CapEff=[0],CapPrm=[0],CapBnd=[0],CapInh=[0],CapAmb=[0])
        probe.validate_privileges(fields,'mutate')
        for key in fields:
            wrong=dict(fields);wrong[key]=[1]*len(wrong[key])
            if wrong == fields:wrong[key]=[0]
            with self.assertRaises(ValueError):probe.validate_privileges(wrong,'mutate')

    def test_mutation_uses_owned_parent_and_no_truncate_or_pwrite(self):
        import os
        from contextlib import ExitStack
        data=b'{"version":1,"installation_id":"00000000000000000000000001","owner_uid":65534,"owner_gid":65534,"profile_revision":1}'
        before=dict(device=1,inode=2,uid=0,gid=0,mode=420,nlink=1,size=len(data),sha256=probe.hashlib.sha256(data).hexdigest())
        changed=data.replace(b'00000000000000000000000001',b'00000000000000000000000002')
        after=dict(before,sha256=probe.hashlib.sha256(changed).hexdigest())
        with ExitStack() as stack:
            parent=stack.enter_context(patch.object(probe,'binding_parent',return_value=10))
            read=stack.enter_context(patch.object(probe,'read_binding_at',side_effect=[(11,data,'00000000000000000000000001',before),(12,changed,'00000000000000000000000002',after)]))
            stack.enter_context(patch.object(probe.os,'lseek',return_value=0))
            write=stack.enter_context(patch.object(probe.os,'write',return_value=len(data)))
            stack.enter_context(patch.object(probe.os,'fsync'))
            close=stack.enter_context(patch.object(probe.os,'close'))
            truncate=stack.enter_context(patch.object(probe.os,'ftruncate'))
            pwrite=stack.enter_context(patch.object(probe.os,'pwrite',create=True))
            probe.mutate('binding-in-place','a'*32)
        parent.assert_called_once_with(probe.REAL)
        self.assertEqual(read.call_args_list[0].kwargs,{'writable':True})
        write.assert_called_once_with(11,changed);truncate.assert_not_called();pwrite.assert_not_called()
        self.assertEqual({call.args[0] for call in close.call_args_list},{10,11,12})

    def test_binding_read_checks_actual_metadata_and_descriptor_flags(self):
        import stat
        from contextlib import ExitStack
        from types import SimpleNamespace
        data=b'{"version":1,"installation_id":"00000000000000000000000001","owner_uid":65534,"owner_gid":65534,"profile_revision":1}'
        original=dict(st_mode=stat.S_IFREG|0o644,st_dev=1,st_ino=2,st_uid=0,st_gid=0,st_nlink=1,st_size=len(data))
        for changes in ({},{'st_nlink':2},{'st_mode':stat.S_IFIFO|0o644},{'st_uid':65534},{'st_size':513}):
            with self.subTest(changes=changes),ExitStack() as stack:
                for name,value in [('O_NOFOLLOW',0x20000),('O_CLOEXEC',0x80000),('O_NONBLOCK',0x800)]:
                    stack.enter_context(patch.object(probe.os,name,value,create=True))
                opened=stack.enter_context(patch.object(probe.os,'open',return_value=10))
                stack.enter_context(patch.object(probe.os,'fstat',return_value=SimpleNamespace(**dict(original,**changes))))
                stack.enter_context(patch.object(probe.os,'stat',return_value=SimpleNamespace(**original)))
                stack.enter_context(patch.object(probe.os,'read',return_value=data))
                closed=stack.enter_context(patch.object(probe.os,'close'))
                if changes:
                    with self.assertRaises(ValueError):probe.read_binding_at(9)
                    closed.assert_called_once_with(10)
                else:
                    self.assertEqual(probe.read_binding_at(9)[2],'00000000000000000000000001')
                self.assertEqual(opened.call_args.args[0],'binding.json')
                self.assertEqual(opened.call_args.kwargs,{'dir_fd':9})
                self.assertEqual(opened.call_args.args[1] & (0x20000|0x80000|0x800),0x20000|0x80000|0x800)

    def test_replacement_uses_exclusive_file_in_pinned_parent(self):
        from contextlib import ExitStack
        from types import SimpleNamespace
        data=b'{"version":1,"installation_id":"00000000000000000000000001","owner_uid":65534,"owner_gid":65534,"profile_revision":1}'
        before=dict(device=1,inode=2,uid=0,gid=0,mode=420,nlink=1,size=len(data),sha256=probe.hashlib.sha256(data).hexdigest())
        after=dict(before,inode=3)
        with ExitStack() as stack:
            for name,value in [('O_NOFOLLOW',0x20000),('O_CLOEXEC',0x80000)]:stack.enter_context(patch.object(probe.os,name,value,create=True))
            stack.enter_context(patch.object(probe,'binding_parent',return_value=9))
            stack.enter_context(patch.object(probe,'read_binding_at',side_effect=[(10,data,'00000000000000000000000001',before),(12,data,'00000000000000000000000001',after)]))
            opened=stack.enter_context(patch.object(probe.os,'open',return_value=11))
            stack.enter_context(patch.object(probe.os,'fchmod',create=True))
            stack.enter_context(patch.object(probe.os,'write',return_value=len(data)))
            stack.enter_context(patch.object(probe.os,'fsync'))
            stack.enter_context(patch.object(probe.os,'fstat',return_value=SimpleNamespace(st_dev=1,st_ino=3,st_uid=0,st_gid=0)))
            replaced=stack.enter_context(patch.object(probe.os,'replace'))
            stack.enter_context(patch.object(probe.os,'close'))
            probe.mutate('binding-replacement','a'*32)
            flags=opened.call_args.args[1]
            self.assertTrue(flags & probe.os.O_EXCL and flags & probe.os.O_CREAT)
            self.assertFalse(flags & probe.os.O_TRUNC)
            self.assertEqual(opened.call_args.kwargs,{'dir_fd':9})
            replaced.assert_called_once_with('.fixed-root-'+'a'*32,'binding.json',src_dir_fd=9,dst_dir_fd=9)


def evidence(case,phase,observation=False,held=False):
    value=dict.fromkeys(probe.SCOPE_FIELDS,False)
    value.update(version=1,case=case,status='observed',identity=dict(host_pid=9,creator_tid=9,children=[]),
        resources_closed_after_exit=not observation,pressure_threads=4,pressure_iterations=0 if observation else 3)
    if phase == 'setup':value.update(real_publications=int(case != 'missing-fixed-root'),decoy_publications=1,
        real_installation_id=None if case == 'missing-fixed-root' else '00000000000000000000000001',decoy_installation_id='00000000000000000000000002')
    else:value.update(initial_opened=case not in probe.NEGATIVES,initial_validated=case not in probe.NEGATIVES,
        selected_installation_id='00000000000000000000000001' if case not in probe.NEGATIVES else None,
        initial_error='not-found' if case == 'missing-fixed-root' else 'crosses-devices' if case.startswith('mount-') else 'none',
        snapshot_invalidated=case in probe.MUTATIONS and not held,validation_error='invalid-input' if case in probe.MUTATIONS and not held else 'none')
    return value


if __name__ == '__main__': unittest.main()
