import unittest
import rust_provision_probe as probe
import run_provision_probe as runner

class ProvisionEvidenceTests(unittest.TestCase):
    def test_opening_collection_sync_failure_is_unpublished_in_both_phases(self):
        case = 'error-openings-sync'
        self.assertIn(case, probe.CASES)
        for phase in ('writer', 'reader'):
            value = evidence(case, phase)
            probe.validate_result(value, case, phase)
            if phase == 'writer':
                self.assertEqual(value['publication_results'], 0)
                self.assertEqual(value['unpublished_refusals'], 1)
                self.assertFalse(value['durability_indeterminate'])
            else:
                self.assertFalse(value['reader_opened'])

    def test_capability_profiles_reject_incomplete_and_extra_capabilities(self):
        fields = {'Uid':[0]*4, 'Gid':[0]*4, 'NoNewPrivs':[1], 'CapEff':[5], 'CapPrm':[5], 'CapBnd':[5], 'CapInh':[0], 'CapAmb':[0]}
        probe.validate_privileges(fields, 'writer')
        for key in fields:
            changed = dict(fields, **{key:[4]})
            with self.subTest(key=key), self.assertRaises(ValueError):
                probe.validate_privileges(changed, 'writer')
        missing = dict(fields); del missing['CapPrm']
        with self.assertRaises(ValueError): probe.validate_privileges(missing, 'writer')
        dropped = dict(fields, CapEff=[0], CapPrm=[0])
        probe.validate_privileges(dropped, 'writer', missing=True)
        with self.assertRaises(ValueError): probe.validate_privileges(dropped, 'writer')

    def test_fixture_archive_preserves_mixed_owners_and_has_no_valid_final_seed(self):
        import io, tarfile
        with tarfile.open(fileobj=io.BytesIO(runner.fixture_archive())) as archive:
            members = {item.name:item for item in archive.getmembers()}
        self.assertEqual({name.split('/')[0] for name in members}, probe.CASES)
        self.assertEqual(members['wrong-scaffold-owner/var/lib/vivac/custody-v1'].uid, 65534)
        self.assertEqual(members['wrong-scaffold-owner/var/lib/vivac/custody-v1'].gid, 65534)
        self.assertEqual(members['wrong-scaffold-mode/var/lib/vivac/custody-v1'].mode, 0o555)
        self.assertNotIn('existing-destination/var/lib/vivac', members)
        self.assertTrue(members['existing-symlink/var/lib/vivac/custody-v1/users/65534'].issym())
        self.assertEqual(members['existing-malformed/var/lib/vivac/custody-v1/users/65534/binding.json'].size, 2)


def evidence(case, phase='writer', observation=False):
    value = dict.fromkeys(probe.SCOPE_FIELDS,False)
    children = []
    if phase == 'writer' and case in {'concurrent-writers','crash-before-rename','crash-after-rename','lost-reply'}:
        count = 2 if case == 'concurrent-writers' else 1
        children = [dict(pid=30+i,token=100+i,ordinal=i+1) for i in range(count)]
    opened = case in {'new-slot','existing-destination','concurrent-writers','error-after-rename','error-parent-sync','error-after-parent-sync','crash-after-rename','lost-reply'}
    value.update(version=1,case=case,status='observed',identity=dict(host_pid=20,creator_tid=20,children=children),
        resources_closed_after_exit=not observation,pressure_threads=4,pressure_iterations=0 if observation else 10)
    if phase == 'reader': value['reader_opened'] = opened
    else:
        dead = case in {'crash-before-rename','crash-after-rename','lost-reply'}
        exits = children if not observation or dead else []
        value.update(publication_results=int(case in {'new-slot','existing-destination','concurrent-writers'}),
            unpublished_refusals=4 if case == 'invalid-selectors' else int(not opened or case in {'existing-destination','concurrent-writers'} and not dead),
            durability_indeterminate=case in {'error-after-rename','error-parent-sync','error-after-parent-sync','crash-after-rename'},
            reply_lost=case == 'lost-reply',final_name_occupied=opened or case in {'existing-malformed','existing-symlink'},
            existing_unchanged=case in {'existing-destination','existing-malformed','existing-symlink'},access_policy_checks=int(case == 'scaffold-acl'),
            injected_failure=case if case.startswith('error-') else 'none',owned_exits_observed=not observation or dead,
            child_exits=[dict(pid=child['pid'],token=child['token'],code=None if dead else 0,signal=9 if dead else None) for child in exits])
        if case == 'crash-before-rename': value['unpublished_refusals'] = 0
    return value


class SchemaTests(unittest.TestCase):
    def test_all_boundaries_and_phases_reject_bool_counters_and_false_acknowledgements(self):
        import copy
        self.assertEqual(len(probe.CASES),33)
        self.assertIn('error-endpoints-sync',probe.CASES)
        for phase in ('writer','reader'):
            for case in probe.CASES:
                for observation in (False,True):
                    value = evidence(case,phase,observation)
                    probe.validate_result(value,case,phase,observation=observation)
                    for field in ('version','pressure_threads','pressure_iterations') + (('publication_results','unpublished_refusals','access_policy_checks') if phase == 'writer' else ()):
                        wrong = copy.deepcopy(value); wrong[field] = bool(value[field])
                        with self.subTest(case=case,phase=phase,field=field), self.assertRaises(ValueError):
                            probe.validate_result(wrong,case,phase,observation=observation)
                    for field in probe.SCOPE_FIELDS:
                        wrong = dict(value,**{field:True})
                        with self.assertRaises(ValueError): probe.validate_result(wrong,case,phase,observation=observation)
                    wrong = dict(value,unreviewed=True)
                    with self.assertRaises(ValueError): probe.validate_result(wrong,case,phase,observation=observation)

    def test_receipt_and_exit_evidence_cannot_change_pid_token_or_creator(self):
        import copy
        value = evidence('concurrent-writers')
        for path in ('pid','token'):
            wrong = copy.deepcopy(value); wrong['child_exits'][0][path] += 1
            with self.assertRaises(ValueError): probe.validate_result(wrong,'concurrent-writers','writer')
        wrong = copy.deepcopy(value); wrong['identity']['creator_tid'] += 1
        with self.assertRaises(ValueError): probe.validate_result(wrong,'concurrent-writers','writer')
        wrong = copy.deepcopy(value); wrong['identity']['children'][1]['pid'] = wrong['identity']['children'][0]['pid']
        with self.assertRaises(ValueError): probe.validate_result(wrong,'concurrent-writers','writer')
        wrong = copy.deepcopy(value); wrong['child_exits'][0]['code'] = False
        with self.assertRaises(ValueError): probe.validate_result(wrong,'concurrent-writers','writer')

    def test_reader_refusal_remains_bounded_diagnostic_not_a_positive(self):
        import json
        for diagnostic in ('x'*513,'\u00f1',False,''):
            with self.assertRaises(ValueError): probe.parse_action_packet(json.dumps(dict(stage='reader-refusal',diagnostic=diagnostic)).encode('ascii'))
        with self.assertRaises(probe.ReaderRefusal): probe.parse_action_packet(b'{"stage":"reader-refusal","diagnostic":"directory-4: errno=22"}')

    def test_exact_capability_parser_rejects_duplicates(self):
        from unittest.mock import patch
        from pathlib import Path
        raw = b'Uid:\t0\t0\t0\t0\nCapEff:\t5\nCapEff:\t0\n'
        with patch.object(Path,'read_bytes',return_value=raw),self.assertRaises(ValueError): probe.process_credentials(20)

def completed_report(phase):
    result = dict.fromkeys(probe.SCOPE_FIELDS,False)
    cases = []
    for case in sorted(probe.CASES):
        value = evidence(case,phase)
        digest = 'a'*64 if case in {'new-slot','existing-destination','existing-malformed','concurrent-writers','error-after-rename','error-parent-sync','error-after-parent-sync','crash-after-rename','lost-reply'} else None
        value.update(process_identities_verified=True,privilege_profile_verified=True,host_retained_until_observation=True,
            children_observed_alive_before_action=bool(value['identity']['children']),children_retained_until_observation=phase == 'writer' and case == 'concurrent-writers',
            children_exits_observed_externally=True,host_exit_code=0,resources_closed_after_observed_exit=True,final_binding_sha256=digest)
        if phase == 'writer': value['scaffold_unchanged'] = True if case in {'wrong-scaffold-owner','wrong-scaffold-mode','scaffold-symlink','scaffold-acl'} else None
        cases.append(value)
    result.update(version=1,experiment='rust-provision',phase=phase,status='observed',cases=cases,resources_closed_after_observed_exit=True,
        candidate_provision_writer_observed=phase == 'writer',candidate_provision_reader_observed=phase == 'reader',helper_sha256='1'*64,observer_sha256='2'*64,
        kernel_release='6.8.0-fixture',resource_limits=dict(memory_bytes=536870912,swap_bytes=0,pids=128,cpu_quota=100000,cpu_period=100000,temporary_bytes=134217728))
    return result


class ManifestTests(unittest.TestCase):
    def test_complete_matrix_and_phase_closure_required_without_retroactive_ack(self):
        import copy
        for phase in ('writer','reader'):
            value = completed_report(phase); probe.validate_observation(value,phase)
            for field,replacement in [('status','incomplete'),('candidate_provision_'+phase+'_observed',False),
                    ('candidate_provision_'+('reader' if phase == 'writer' else 'writer')+'_observed',True),
                    ('resources_closed_after_observed_exit',False),('phase','reader' if phase == 'writer' else 'writer')]:
                wrong = copy.deepcopy(value); wrong[field] = replacement
                with self.assertRaises(ValueError): probe.validate_observation(wrong,phase)
            wrong = copy.deepcopy(value); wrong['cases'][-1] = wrong['cases'][0]
            with self.assertRaises(ValueError): probe.validate_observation(wrong,phase)
            wrong = copy.deepcopy(value); wrong['cases'][0]['host_exit_code'] = False
            with self.assertRaises(ValueError): probe.validate_observation(wrong,phase)
        writer, reader = completed_report('writer'),completed_report('reader')
        runner.compare_phases(writer,reader)
        reader['cases'][0]['final_binding_sha256'] = 'b'*64
        with self.assertRaises(runner.RunnerError): runner.compare_phases(writer,reader)
        reader = completed_report('reader'); reader['helper_sha256'] = '3'*64
        with self.assertRaises(runner.RunnerError): runner.compare_phases(writer,reader)


class ThreadProfileTests(unittest.TestCase):
    def test_lost_capabilities_rejects_residual_privileged_threads(self):
        from pathlib import Path
        from unittest.mock import patch
        root = dict(Uid=[0]*4,Gid=[0]*4,NoNewPrivs=[1],CapEff=[5],CapPrm=[5],CapBnd=[5],CapInh=[0],CapAmb=[0])
        dropped = dict(root,CapEff=[0],CapPrm=[0])
        with patch.object(Path,'iterdir',return_value=[Path('20'),Path('21')]), \
                patch.object(probe,'process_credentials',side_effect=[dropped,root]),self.assertRaises(ValueError):
            probe.verify_threads(20,'writer',missing=True)
        with patch.object(Path,'iterdir',return_value=[Path('20'),Path('21')]),self.assertRaises(ValueError):
            probe.verify_threads(20,'writer',single_thread=True)
        with patch.object(Path,'iterdir',return_value=[Path('20')]),patch.object(probe,'process_credentials',return_value=root):
            probe.verify_threads(20,'writer',single_thread=True)
        with patch.object(Path,'iterdir',return_value=[Path(str(pid)) for pid in range(20,25)]),patch.object(probe,'process_credentials',return_value=dropped):
            probe.verify_threads(20,'writer',missing=True)


class ScaffoldSnapshotTests(unittest.TestCase):
    def test_changed_owner_mode_entry_bytes_target_or_acl_refuses(self):
        import copy
        baseline = ({'.':((0o40755,0,0,3,4,2),None), './link':((0o120777,0,0,3,5,1),'custody-real'),
                     './binding':((0o100644,0,0,3,6,1),b'{}')}, {'.':{'system.posix_acl_access':b'acl'}})
        for index,replacement in ((0,0o40700),(1,65534),(2,65534),(3,9),(4,9),(5,3)):
            changed = copy.deepcopy(baseline); metadata=list(changed[0]['.'][0]);metadata[index]=replacement
            changed[0]['.']=(tuple(metadata),None)
            with self.assertRaises(ValueError): probe.require_scaffold_unchanged(baseline,changed)
        for relative, replacement in (('./binding',((0o100644,0,0,3,6,1),b'other')),('./link',((0o120777,0,0,3,5,1),'other'))):
            changed=copy.deepcopy(baseline);changed[0][relative]=replacement
            with self.assertRaises(ValueError): probe.require_scaffold_unchanged(baseline,changed)
        changed=copy.deepcopy(baseline);del changed[0]['./binding']
        with self.assertRaises(ValueError): probe.require_scaffold_unchanged(baseline,changed)
        changed=copy.deepcopy(baseline);changed[1]['.']['system.posix_acl_access']=b'changed'
        with self.assertRaises(ValueError): probe.require_scaffold_unchanged(baseline,changed)
        probe.require_scaffold_unchanged(baseline,copy.deepcopy(baseline))

    def test_scaffold_evidence_is_exact_nullable_writer_only(self):
        import copy
        writer=completed_report('writer')
        for value in writer['cases']:
            expected = True if value['case'] in {'wrong-scaffold-owner','wrong-scaffold-mode','scaffold-symlink','scaffold-acl'} else None
            value['scaffold_unchanged']=expected
            probe.validate_case(value,'writer')
            for invalid in (False,1,'true',None if expected is True else True):
                wrong=copy.deepcopy(value);wrong['scaffold_unchanged']=invalid
                with self.assertRaises(ValueError): probe.validate_case(wrong,'writer')
        value=completed_report('reader')['cases'][0];value['scaffold_unchanged']=None
        with self.assertRaises(ValueError): probe.validate_case(value,'reader')


class ScaffoldAclCaptureTests(unittest.TestCase):
    def test_snapshot_captures_acl_bytes_from_retained_directory_without_following(self):
        from contextlib import ExitStack
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        metadata=(0o40755,0,0,3,4,2)
        entries={'.':(metadata,None)}
        info=SimpleNamespace(st_mode=metadata[0],st_uid=0,st_gid=0,st_dev=3,st_ino=4,st_nlink=2)
        with ExitStack() as stack:
            stack.enter_context(patch.object(probe.reader,'fixture_snapshot',return_value=entries))
            for name,value in [('O_DIRECTORY',65536),('O_NOFOLLOW',131072),('O_CLOEXEC',524288)]:
                stack.enter_context(patch.object(probe.os,name,value,create=True))
            opened=stack.enter_context(patch.object(probe.os,'open',return_value=10))
            stack.enter_context(patch.object(probe.os,'dup',return_value=11))
            stack.enter_context(patch.object(probe.os,'close'))
            stack.enter_context(patch.object(probe.os,'fstat',return_value=info))
            stack.enter_context(patch.object(probe.os,'listxattr',return_value=['system.posix_acl_access','user.ignored'],create=True))
            acl=stack.enter_context(patch.object(probe.os,'getxattr',return_value=b'original',create=True))
            before=probe.scaffold_snapshot(Path('/fixture'))
            self.assertEqual(before[1],{'.':{'system.posix_acl_access':b'original'}})
            self.assertTrue(opened.call_args.args[1] & probe.os.O_NOFOLLOW)
            acl.return_value=b'modified'
            after=probe.scaffold_snapshot(Path('/fixture'))
            with self.assertRaises(ValueError): probe.require_scaffold_unchanged(before,after)

if __name__ == '__main__': unittest.main()
