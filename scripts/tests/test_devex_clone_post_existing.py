"""原业务数据集到当前复制目标的边界核验；Node 输出为纯 mock。"""
import json
from contextlib import contextmanager, nullcontext
from pathlib import Path
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_post as post
from devex_clone_post_context import Context
from devex_clone_capture import write_json
from devex_clone_factory_context import Environments
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash


class ExistingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        backend = Path(temporary.name).resolve()
        output = backend / '.local-tests/existing'
        output.mkdir(parents=True)
        self.plan = {'source': {'scope_id': 'source-one'}}
        self.dataset = {'tenants': [{'files': [{'file_path': 'system/a.txt', 'bytes': 4, 'sha256': 'a' * 64}]}]}
        self.files = {}
        for key, value in (('reference_plan', self.plan), ('dataset', self.dataset)):
            path = output / (key + '.json')
            write_json(path, value)
            self.files[key] = binding(path)
        self.target = {'format_version': 1, 'kind': 'devex-copy-business-target', 'source_plan_sha256': plan_hash(self.plan),
            'source_scope_id': 'source-one', 'target': {'scope_id': 'target-one', 'api_url': 'http://127.0.0.1:18210',
            'frontend_url': 'http://127.0.0.1:4190'}, 'copy': {key: 'b' * 64 for key in
            ('plan_sha256', 'generation_sha256', 'source_export_sha256', 'fresh_target_sha256', 'stage_receipt_sha256', 'ledger_head_sha256')}}
        self.context = SimpleNamespace(backend=backend, output=output, request={**self.files, 'node': {'path': 'unit-node'}},
            request_binding={'sha256': 'c' * 64}, environments=Environments({}, {}), guard=lambda **_: None,
            business_binding=lambda: self.target, business_objects=lambda **_: {'business_objects': 1, 'additional_objects': 1})
        self.response_change = {}
        self.commands = []
        def execute(command, **kwargs):
            self.commands.append(command)
            value = {'format_version': 1, 'status': 'copy_existing_data_verified', 'side': 'copy_target',
                'scope_id': 'target-one', 'plan_sha256': plan_hash(self.plan), 'source_scope_id': 'source-one',
                'target_binding_sha256': plan_hash(self.target), 'copy': self.target['copy'], 'clone_verified': False,
                'restore_success': False, 'tenants': 11, 'posts': 33, 'files': 256,
                'input_files': {'plan_sha256': self.files['reference_plan']['sha256'],
                    'dataset_sha256': self.files['dataset']['sha256'], 'target_binding_sha256': binding(output / 'business-target.json')['sha256']}}
            return SimpleNamespace(stdout=json.dumps({**value, **self.response_change}).encode())
        self.context.run = execute
        self.addCleanup(patch.stopall)
        patch('devex_clone_post_actions.verify_confirmations', return_value=[]).start()
        patch('devex_clone_post.require_schedule_stage').start()
        producer = patch('devex_clone_post_process.Producer').start().return_value
        producer.communicate.side_effect = lambda **_: execute(['--target-binding']).stdout
        self.producer = producer

    def test_node_receipt_keeps_original_plan_and_dataset_bytes(self):
        old = {key: binding(Path(item['path'])) for key, item in self.files.items()}
        result = post.verify_existing(self.context)
        self.assertEqual(result['status'], 'post_copy_existing_data_verified')
        self.assertEqual({key: binding(Path(item['path'])) for key, item in old.items()}, old)
        self.assertIn('--target-binding', self.commands[0])
        self.assertNotIn('restore_reference_dataset.mjs', str(self.commands[0]))
        self.assertFalse(result['restore_qualified'])

    def test_wrong_target_node_scope_or_original_file_digest_rejected(self):
        self.response_change = {'input_files': {'plan_sha256': '0' * 64}}
        with self.assertRaises(ValueError): post.verify_existing(self.context)
        self.assertFalse((self.context.output / 'business-result.json').exists())

    def test_incomplete_schedule_blocks_before_node(self):
        with patch('devex_clone_post_actions.verify_confirmations', side_effect=ValueError('未确认')):
            with self.assertRaises(ValueError): post.verify_existing(self.context)
        self.assertEqual(self.commands, [])

    def test_changed_guard_after_business_reads_cannot_report_success(self):
        calls = []
        def guard(**_):
            calls.append(True)
            if len(calls) == 2: raise ValueError('现场变化')
        self.context.guard = guard
        with self.assertRaises(ValueError): post.verify_existing(self.context)
        self.assertTrue((self.context.output / 'business-result.json').exists())

    def test_node_failure_and_cleanup_failure_preserve_original_error(self):
        self.producer.communicate.side_effect = TimeoutError('原请求未知')
        self.producer.close.side_effect = ValueError('精确清理失败')
        with self.assertRaisesRegex(TimeoutError, '原请求未知'):
            post.verify_existing(self.context)
        self.assertTrue((self.context.output / 'session-cleanup-error.json').exists())
        self.assertFalse((self.context.output / 'business-result.json').exists())

    def test_changed_published_schedule_binding_after_node_cannot_report_success(self):
        with patch.object(post, 'require_schedule_stage', side_effect=[None, ValueError('确认来源变化')]) as require:
            with self.assertRaises(ValueError): post.verify_existing(self.context)
        self.assertEqual(require.call_count, 2)
        self.assertTrue((self.context.output / 'business-result.json').exists())

    def object_context(self):
        context = Context.__new__(Context)
        context.backend, context.output, context.request = self.context.backend, self.context.output, self.context.request
        context.target_config = {'scope_id': 'target-one'}
        entries = [dict(bucket='uploads', target_key='target-one/system/a.txt', artifact={'bytes': 4, 'sha256': 'a' * 64}),
                   dict(bucket='uploads', target_key='target-one/system/probe.txt', artifact={'bytes': 4, 'sha256': 'd' * 64})]
        context.copy = {'plan': {'objects': entries}}
        context.environments = Environments({}, {})
        context.tool_plan, context.run = {}, lambda *_: None
        context.target_guard = lambda **_: SimpleNamespace(object_keys=lambda bucket: {'target-one/.ryframe-owner'} | {
            item['target_key'] for item in entries if item['bucket'] == bucket})
        context.directory = lambda label: context.output / label
        return context

    def test_business_dataset_matches_planned_objects_and_extra_probe_is_precisely_read(self):
        context = self.object_context()
        with patch('devex_clone_post_context.ExternalTools'), patch('devex_clone_post_context.observe_target_object') as observe:
            result = context.business_objects(remaining=True)
        self.assertEqual(result['business_objects'], 1)
        self.assertEqual(result['additional_objects'], 1)
        self.assertEqual(observe.call_args.args[2:4], ('uploads', 'target-one/system/probe.txt'))

    def test_wrong_original_dataset_sha_cannot_select_different_object_bytes(self):
        context = self.object_context()
        context.copy['plan']['objects'][0]['artifact']['sha256'] = 'e' * 64
        with self.assertRaises(ValueError): context.business_objects()

    def test_extra_unregistered_object_fails_scope_snapshot(self):
        context = self.object_context()
        context.target_guard = lambda **_: SimpleNamespace(object_keys=lambda _: {'target-one/unknown'})
        with patch('devex_clone_post_context.ExternalTools'), self.assertRaises(ValueError):
            context.business_objects(remaining=True)

    def test_before_api_objects_are_bounded_and_finish_inside_parent_environment(self):
        for failure in (False, True):
            with self.subTest(failure=failure):
                context = self.object_context()
                context.copy['plan']['objects'] = [dict(bucket='uploads', target_key=f'target-one/{index}',
                    artifact={'bytes': 4, 'sha256': 'a' * 64}) for index in range(9)]
                context.target_config['databases'] = []
                context.target = {'maintenance_build': {}}
                context.initial = {'inventory': {}}
                context.bindings = lambda: None
                context.source_guard = lambda **_: None
                context.target_guard = lambda **_: SimpleNamespace(object_keys=lambda bucket: {'target-one/.ryframe-owner'} | {
                    item['target_key'] for item in context.copy['plan']['objects'] if item['bucket'] == bucket})
                allocations, calls, active = [], [], [False]
                @contextmanager
                def environment(_side):
                    active[0] = True
                    try: yield {}
                    finally: active[0] = False
                context.old = context.environments = SimpleNamespace(use=environment)
                parent = threading.get_ident()
                def directory(label):
                    self.assertEqual(threading.get_ident(), parent)
                    value = context.output / f'{label}-{len(allocations)}'
                    allocations.append(value)
                    return value
                context.directory = directory
                def observe(_backend, _tools, _bucket, key, output, **limits):
                    self.assertTrue(active[0])
                    self.assertNotEqual(threading.get_ident(), parent)
                    self.assertIn(output, allocations)
                    self.assertEqual(limits, {'expected': {'bytes': 4, 'sha256': 'a' * 64}, 'max_bytes': 4})
                    calls.append(key)
                    time.sleep(.02)
                    self.assertTrue(active[0])
                    if failure and key == 'target-one/0': raise ValueError('first batch failed')
                with patch('devex_clone_post_context.source_check', return_value=nullcontext()), \
                        patch('devex_clone_post_context.bound_file', return_value=context.output / 'maintenance.json'), \
                        patch('devex_clone_post_context.capture_side_inventory', return_value=SimpleNamespace(observations=[])), \
                        patch('devex_clone_post_context.ExternalTools'), \
                        patch('devex_clone_post_context.observe_target_object', side_effect=observe):
                    if failure:
                        with self.assertRaises(ValueError): context.before_api()
                    else: context.before_api()
                self.assertFalse(active[0])
                self.assertEqual(len(calls), 4 if failure else 9)
                self.assertEqual(len(set(allocations)), 10)


if __name__ == '__main__':
    unittest.main()
