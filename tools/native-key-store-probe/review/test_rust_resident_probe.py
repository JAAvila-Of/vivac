import copy
import unittest
import rust_resident_probe as probe


class ResidentTests(unittest.TestCase):
    def test_snapshot_packet_requires_bounded_unique_exact_schema(self):
        self.assertEqual(probe.parse_snapshot_packet(b'{"stage":"snapshot-held","payload":{"version":1}}'), {'version':1})
        for raw in (b'{"stage":"snapshot-held","payload":{},"extra":0}',
                    b'{"stage":"snapshot-held","payload":[],"payload":{}}',
                    b'{"stage":"snapshot-held","payload":[]}',
                    b'{"stage":"observation","payload":{}}',
                    b'{"stage":"snapshot-held","payload":{"padding":"'+b'a'*8192+b'"}}'):
            with self.assertRaises(ValueError): probe.parse_snapshot_packet(raw)

    def test_snapshot_refusal_preserves_only_bounded_ascii_diagnostic(self):
        with self.assertRaises(probe.reader.ReaderRefusal) as caught:
            probe.parse_snapshot_packet(b'{"stage":"reader-refusal","diagnostic":"candidate stub"}')
        self.assertEqual(caught.exception.diagnostic,'candidate stub')
        for diagnostic in ('','a'*513,'\u00e9'):
            raw = probe.encoded(dict(stage='reader-refusal',diagnostic=diagnostic))
            with self.assertRaises(ValueError) as caught:
                probe.parse_snapshot_packet(raw)
            self.assertIsNot(type(caught.exception),probe.reader.ReaderRefusal)

    def evidence(self):
        return {path:dict(device=1,inode=index+1,uid=65534,gid=65534,mode=0o700 if path in {probe.PACKAGE,probe.RECORD} else 0o600,nlink=2 if path in {probe.PACKAGE,probe.RECORD} else 1,size=4096 if path in {probe.PACKAGE,probe.RECORD} else 32,sha256=None if path in {probe.PACKAGE,probe.RECORD} else 'a'*64,kind='directory' if path in {probe.PACKAGE,probe.RECORD} else 'file',acl=False) for index,path in enumerate(probe.PATHS)}

    def test_mutations_require_exact_object_and_content_changes(self):
        for case in probe.MUTATIONS:
            before = self.evidence(); after = copy.deepcopy(before)
            target = probe.RECORD+'/record.json' if case.startswith('record-') else probe.PACKAGE+'/key.openssh' if case.startswith('key-') else probe.PACKAGE+'/manifest.json'
            if case == 'package-replacement':
                for path in (probe.PACKAGE,probe.PACKAGE+'/key.openssh',probe.PACKAGE+'/manifest.json'): after[path]['inode'] += 100
            elif case != 'resident-unrelated': after[target]['sha256' if case.endswith('in-place') else 'inode'] = 'b'*64 if case.endswith('in-place') else 100
            value = dict(version=1,case=case,request_token='a'*32,status='observed',before=before,after=after,unrelated_created=case == 'resident-unrelated')
            probe.validate_mutation(value,case,'a'*32)
            with self.assertRaises(ValueError): probe.validate_mutation(value,case,'b'*32)
            wrong = copy.deepcopy(value); wrong['after'][probe.RECORD]['mode'] = 0o755
            with self.assertRaises(ValueError): probe.validate_mutation(wrong,case,'a'*32)
            if case != 'resident-unrelated':
                wrong = copy.deepcopy(value); wrong['after'] = copy.deepcopy(before)
                with self.assertRaises(ValueError): probe.validate_mutation(wrong,case,'a'*32)

    def test_independent_metadata_refuses_bad_owner_links_acl_and_shapes(self):
        for path in probe.PATHS:
            for key,value in [('uid',0),('gid',0),('mode',0o777),('nlink',3),('acl',True)]:
                evidence = self.evidence(); evidence[path][key] = value
                with self.assertRaises(ValueError): probe.validate_evidence(evidence,'resident-valid')
        evidence = self.evidence(); evidence['unexpected'] = None
        with self.assertRaises(ValueError): probe.validate_evidence(evidence,'resident-valid')

    def test_exact_cases_and_record_contract(self):
        self.assertEqual(len(probe.CASES), 23)
        value = probe.record_value('00000000000000000000000001', {'public_key':'a'*64}, b'key', b'{}')
        self.assertEqual(list(value), ['version','installation_id','creation_id','device_id','credential_id','public_key','ciphertext_sha256','manifest_sha256','ciphertext_len','manifest_len','creation_profile_revision','secret_policy_revision'])
        self.assertEqual(value['credential_id'], probe.CRED)
        self.assertEqual(value['ciphertext_len'], 3)

    def test_reader_schema_and_negatives(self):
        for case in probe.CASES:
            value = dict.fromkeys(probe.SCOPE_FIELDS, False)
            opened = case not in probe.NEGATIVES
            value.update(version=1,case=case,status='observed',identity=dict(host_pid=7,creator_tid=7,children=[]),resources_closed_after_exit=True,pressure_threads=4,pressure_iterations=1,initial_opened=opened,initial_validated=opened,selected_installation_id='00000000000000000000000001' if opened else None,selected_credential_id=probe.CRED if opened else None,initial_error=probe.initial_error(case),snapshot_invalidated=case in probe.INVALIDATIONS,validation_error='invalid-data' if case in probe.INVALIDATIONS else 'none')
            probe.validate_result(value, case)
            for field in ('creation_admission','producer_origin_checked','custody_accredited'):
                wrong = copy.deepcopy(value); wrong[field] = True
                with self.assertRaises(ValueError): probe.validate_result(wrong, case)
            wrong = copy.deepcopy(value); wrong['unexpected'] = False
            with self.assertRaises(ValueError): probe.validate_result(wrong, case)

    def test_tokens_and_exact_resource_limits(self):
        request = dict(version=1,case='resident-unrelated',request_token='a'*32,reader_host_pid=3,reader_starttime=4)
        probe.validate_request(request, 'resident-unrelated')
        for token in ('A'*32,'a'*31,True):
            wrong = dict(request, request_token=token)
            with self.assertRaises(ValueError): probe.validate_request(wrong, 'resident-unrelated')
        limits = dict(memory_bytes=320*1048576,swap_bytes=0,pids=96,cpu_quota=75000,cpu_period=100000,temporary_bytes=96*1048576)
        probe.validate_limits(limits, 'reader')
        for key in limits:
            wrong = dict(limits); wrong[key] += 1
            with self.assertRaises(ValueError): probe.validate_limits(wrong, 'reader')


if __name__ == '__main__': unittest.main()
