"""对象所属桶的 ownership 范围由可信调用方明确指定；全部为离线协议 fixture。"""
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from devex_clone_capture import ObjectCaptureError, capture_object, verify_capture
from restore_build import file_digest
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import ExternalTools
from restore_reference_plan import BUCKETS
from test_devex_clone_capture import ObjectReads

ALL = tuple(sorted(BUCKETS))
ONE = ('uploads',)


class OwnerScopeTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.store = ObjectReads()
        original = self.work / 'registered.bin'
        original.write_bytes(self.store.body)
        self.expected = file_digest(original)
        self.tools = ExternalTools(self.plan, self.work, self.store)
        self.output = self.work / 'capture'
        self.side = 'target'
        environment_patch = patch.dict('os.environ', {'TEST_ACCESS': 'access-fixture', 'TEST_SECRET': 'secret-fixture'})
        environment_patch.start()
        self.addCleanup(environment_patch.stop)

    def capture(self, **keywords):
        return capture_object(self.tools, self.side, 'uploads', self.side + '/system/registered.bin', self.output,
                              expected=self.expected, max_bytes=1024, **keywords)

    def verify(self, **keywords):
        return verify_capture(self.tools, self.side, 'uploads', self.side + '/system/registered.bin', self.output,
                              expected=self.expected, max_bytes=1024, **keywords)

    def commands(self):
        return [(command[command.index('s3api') + 1], command[command.index('--bucket') + 1],
                 command[command.index('--key') + 1]) for command, _ in self.store.calls]

    def test_explicit_own_bucket_reads_exactly_two_owners_two_heads_and_one_get(self):
        captured = self.capture(owner_buckets=ONE)
        expected = [('get-object', 'uploads', 'target/.ryframe-owner'),
                    ('head-object', 'uploads', 'target/system/registered.bin'),
                    ('get-object', 'uploads', 'target/system/registered.bin'),
                    ('head-object', 'uploads', 'target/system/registered.bin'),
                    ('get-object', 'uploads', 'target/.ryframe-owner')]
        self.assertEqual(self.commands(), expected)
        self.assertEqual([item['bucket'] for item in captured['ownership_before']], ['uploads'])
        self.assertEqual([item['bucket'] for item in captured['ownership_after']], ['uploads'])
        self.assertTrue(captured['stored_metadata_verified'])
        self.assertTrue(captured['bytes_verified'])
        self.assertFalse(captured['clone_verified'])
        before = {path.name: file_digest(path) for path in self.output.iterdir()}
        verified = self.verify(owner_buckets=ONE)
        self.assertEqual(verified['capture'], captured)
        self.assertFalse(verified['live_revalidated'])
        self.assertEqual(self.commands(), expected)
        self.assertEqual(before, {path.name: file_digest(path) for path in self.output.iterdir()})

    def test_empty_duplicate_unknown_other_bucket_and_noncanonical_scopes_are_rejected(self):
        invalid = ((), ('uploads', 'uploads'), ('unknown',), ('imports',), ('uploads', 'imports'),
                   tuple(reversed(ALL)), list(ALL), ['uploads'], set(ALL), None, 'uploads', True)
        for owners in invalid:
            with self.subTest(owners=repr(owners)), self.assertRaises(ValueError):
                self.capture(owner_buckets=owners)
            self.assertEqual(self.store.calls, [])
            self.assertFalse(self.output.exists())
        self.capture(owner_buckets=ONE)
        before = {path.name: file_digest(path) for path in self.output.iterdir()}
        for owners in invalid:
            with self.subTest(verify_owners=repr(owners)), self.assertRaises(ValueError):
                self.verify(owner_buckets=owners)
        self.assertEqual(len(self.store.calls), 5)
        self.assertEqual(before, {path.name: file_digest(path) for path in self.output.iterdir()})

    def test_own_bucket_failure_before_read_blocks_all_business_requests(self):
        self.store.owner_failure = 'uploads'
        with self.assertRaises(ObjectCaptureError):
            self.capture(owner_buckets=ONE)
        self.assertEqual(self.commands(), [('get-object', 'uploads', 'target/.ryframe-owner')])
        self.assertFalse((self.output / 'object.bin').exists())
        self.assertFalse((self.output / 'capture.json').exists())

    def test_own_bucket_change_after_get_preserves_bytes_and_fails(self):
        def read(command, **kwargs):
            response = self.store(command, **kwargs)
            if (command[command.index('s3api') + 1] == 'get-object'
                    and not command[command.index('--key') + 1].endswith('/.ryframe-owner')):
                self.store.owner_failure = 'uploads'
            return response
        self.tools.run = read
        with self.assertRaises(ObjectCaptureError):
            self.capture(owner_buckets=ONE)
        self.assertEqual(len(self.store.calls), 5)
        self.assertEqual((self.output / 'object.bin').read_bytes(), self.store.body)
        self.assertFalse((self.output / 'capture.json').exists())
        with self.assertRaises(ValueError): self.verify(owner_buckets=ONE)

    def test_scoped_capture_keeps_full_body_and_metadata_tamper_checks(self):
        for index, filename in enumerate(('object.bin', 'head-before.json', 'head-after.json', 'get.json')):
            with self.subTest(filename=filename):
                self.output = self.work / f'changed-{index}'
                self.store.heads = 0
                self.capture(owner_buckets=ONE)
                path = self.output / filename
                if filename == 'object.bin':
                    path.write_bytes(b'x' * self.expected['bytes'])
                else:
                    value = json.loads(path.read_text(encoding='utf-8'))
                    value['Metadata']['purpose'] = 'different-original-observation'
                    path.write_text(json.dumps(value), encoding='utf-8')
                calls = len(self.store.calls)
                with self.assertRaises(ValueError): self.verify(owner_buckets=ONE)
                self.assertEqual(len(self.store.calls), calls)

    def test_default_source_capture_still_contains_all_five_buckets_and_verifies_offline(self):
        self.side = 'source'
        captured = self.capture()
        self.assertEqual(len(self.store.calls), 13)
        self.assertEqual(tuple(item['bucket'] for item in captured['ownership_before']), ALL)
        self.assertEqual(tuple(item['bucket'] for item in captured['ownership_after']), ALL)
        verified = self.verify()
        self.assertEqual(verified['capture'], captured)
        self.assertEqual(len(self.store.calls), 13)

    def test_default_source_verification_rejects_every_other_bucket_owner_proof_tamper(self):
        self.side = 'source'
        for stage in ('before', 'after'):
            for bucket in sorted(BUCKETS - {'uploads'}):
                with self.subTest(stage=stage, bucket=bucket):
                    self.output = self.work / f'owner-{stage}-{bucket}'
                    self.store.heads = 0
                    self.capture()
                    (self.output / f'owner-{stage}-{bucket}.bin').write_bytes(b'other-resource-owner')
                    calls = len(self.store.calls)
                    with self.assertRaises(ValueError): self.verify()
                    with self.assertRaises(ValueError): self.verify(owner_buckets=ONE)
                    self.assertEqual(len(self.store.calls), calls)

    def test_one_bucket_receipt_cannot_choose_default_verification_scope(self):
        self.capture(owner_buckets=ONE)
        with self.assertRaises(ValueError): self.verify()
        self.assertEqual(len(self.store.calls), 5)
        self.assertEqual(self.verify(owner_buckets=ONE)['status'], 'capture_evidence_verified')
        self.assertEqual(len(self.store.calls), 5)

    def test_scope_does_not_change_request_facts_and_full_tuple_is_explicitly_supported(self):
        one = self.capture(owner_buckets=ONE)
        request = json.loads((self.output / 'intent.json').read_text(encoding='utf-8'))
        self.output = self.work / 'all-explicit'
        self.store.heads = 0
        all_buckets = self.capture(owner_buckets=ALL)
        full_request = json.loads((self.output / 'intent.json').read_text(encoding='utf-8'))
        self.assertEqual(request, full_request)
        self.assertNotIn('owner_buckets', request)
        self.assertEqual(one['request_sha256'], all_buckets['request_sha256'])
        self.assertEqual(len(self.store.calls), 18)
        self.assertEqual(self.verify(owner_buckets=ALL)['capture'], all_buckets)


if __name__ == '__main__':
    unittest.main()
