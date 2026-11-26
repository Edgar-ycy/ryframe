"""原业务数据集到当前复制目标的共享对象与来源核验。"""
import os
from contextlib import contextmanager, nullcontext
from pathlib import Path
import sys
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from devex_clone_post_context import Context
from devex_clone_capture import write_json
from process_environment import Environments
from devex_clone_run_state import binding
from workspace_directory import WorkspaceDirectory


class ExistingTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(Path(__file__).resolve().parents[2] / '.local-tests/tmp')
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.output = self.backend / '.local-tests/existing'
        self.output.mkdir(parents=True)
        dataset = self.output / 'dataset.json'
        write_json(dataset, {'tenants': [{'files': [
            {'file_path': 'system/a.txt', 'bytes': 4, 'sha256': 'a' * 64},
        ]}]})
        self.request = {'dataset': binding(dataset)}

    def object_context(self):
        context = Context.__new__(Context)
        context.backend, context.output, context.request = self.backend, self.output, self.request
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
                        patch('devex_clone_post_context.execution_root', return_value=context.backend), \
                        patch('devex_clone_post_context.capture_side_inventory', return_value=SimpleNamespace(observations=[])), \
                        patch('devex_clone_post_context.ExternalTools'), \
                        patch('devex_clone_post_context.observe_target_object', side_effect=observe):
                    if failure:
                        with self.assertRaises(ValueError): context.before_api()
                    else: context.before_api()
                self.assertFalse(active[0])
                self.assertEqual(len(calls), 4 if failure else 9)
                self.assertEqual(len(set(allocations)), 10)

    def test_before_api_verifies_target_tree_outside_current_repo_window(self):
        context = self.object_context()
        context.copy['plan']['objects'] = []
        context.target = {'maintenance_build': {}}
        context.bindings = lambda: None
        context.source_guard = lambda **_: None
        context.target_guard = lambda **_: SimpleNamespace(object_keys=lambda _: {'target-one/.ryframe-owner'})
        context.old = context.environments = SimpleNamespace(use=lambda _: nullcontext({}))
        windows = []

        @contextmanager
        def window(_root):
            windows.append(True)
            try:
                yield None
            finally:
                windows.pop()

        def capture(*_args, **_kwargs):
            self.assertFalse(windows, "目标执行树核验必须在当前仓源码窗口之外")
            return SimpleNamespace(observations=[])

        with patch('devex_clone_post_context.source_check', window), \
                patch('devex_clone_post_context.bound_file', return_value=context.output / 'maintenance.json'), \
                patch('devex_clone_post_context.execution_root', return_value=context.backend), \
                patch('devex_clone_post_context.capture_side_inventory', side_effect=capture) as observed, \
                patch('devex_clone_post_context.ExternalTools'), \
                patch('devex_clone_post_context.observe_target_object'):
            context.before_api()
        self.assertEqual(observed.call_count, 1)
        self.assertEqual(windows, [])

    def test_guard_verifies_source_outside_current_repo_window(self):
        context = Context.__new__(Context)
        context.backend, context.output = self.backend, self.output
        context.bindings = lambda: None
        context.target_guard = lambda **_: None
        windows = []

        @contextmanager
        def window(_root):
            windows.append(True)
            try:
                yield None
            finally:
                windows.pop()

        def source_guard(**kwargs):
            self.assertFalse(windows, "来源执行树核验必须在当前仓源码窗口之外")

        context.source_guard = source_guard
        with patch('devex_clone_post_context.source_check', window):
            context.guard()
        self.assertEqual(windows, [])

    def test_source_guard_uses_the_registered_run_and_rejects_changed_generation(self):
        context = Context.__new__(Context)
        context.backend, context.output = self.backend, self.output
        context.directory_root = context.backend / '.local-tests' / 'registered-run'
        context.environments = Environments({'APP_ENV': 'test'}, {'APP_ENV': 'prod'})
        context.source = {'generation': {'fixture': 'original'}}
        context.tool_plan = {}

        def observe(backend, run_dir, runner):
            self.assertEqual((backend, run_dir, runner), (context.backend, context.directory_root, context.run))
            self.assertEqual(os.environ['APP_ENV'], 'test')
            return {'fixture': 'changed'}

        with patch('devex_clone_post_context.copy_source_generation', side_effect=observe), \
                patch('devex_clone_post_context.ExternalTools') as tools, \
                self.assertRaisesRegex(ValueError, '停止的来源代次变化'):
            context.source_guard()
        tools.assert_not_called()


if __name__ == '__main__':
    unittest.main()
