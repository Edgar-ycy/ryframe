"""真实本地计划与完成账本进入 Context 时只解析一次；所有远端端口使用离线 fixture。"""
import copy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone as clone
import devex_clone_run as run
import devex_clone_post_context as context_module
import test_devex_clone_factory as fixtures
from devex_clone_capture import write_json
from devex_clone_run_state import binding, initialize_state, begin, finish


class PostPlanTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.FactoryTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        result = self.fixture.execute()
        self.backend = self.fixture.backend
        self.directory = self.backend / '.local-tests/post-run'
        self.directory.mkdir()
        self.value = {'copy_directory': str(self.fixture.output), 'initialized': self.fixture.initialized,
                      'source_export': self.fixture.export_binding}
        env = {'FIXTURE_ACCESS': 'fixture-access', 'FIXTURE_SECRET': 'fixture-secret'}
        for side in ('source', 'target'):
            path = self.directory / (side + '-environment.json')
            write_json(path, {'environment': env})
            self.value[side + '_environment'] = binding(path)
        self.request = {'api_environment': self.value['target_environment'],
                        'backend_build': self.fixture.bind(self.directory / 'build.json', {'fixture': 'build'})}
        write_json(self.directory / 'manifest.json', self.value)
        write_json(self.directory / 'post-copy.json', self.request)
        initialize_state(self.directory)
        number = begin(self.directory, 'copy', 'resume', {})
        finish(self.directory, number, result=result)
        number = begin(self.directory, 'post-copy', 'register', {})
        finish(self.directory, number, result={'status': 'post_copy_registered',
               'registration': binding(self.directory / 'post-copy.json')})
        source = copy.deepcopy(self.fixture.verified)
        source['request']['source'] = self.fixture.case.tools.plan['source']
        self.addCleanup(patch.stopall)
        patch.object(context_module, 'registered', return_value=self.request).start()
        patch.object(context_module, 'initialization_history', return_value=(self.fixture.initial, self.fixture.target_request)).start()
        patch.object(context_module, 'request_binding', return_value=({}, self.fixture.initial['generation']['selected'])).start()
        patch.object(context_module, 'verify_source_export', return_value=source).start()
        # 这里只隔离已经在登记单测覆盖的字段检查；正式发布/账本门禁和语义解析都实际执行。
        self.validate = patch.object(context_module, 'validate_registration', side_effect=lambda backend, directory, value, _:
            run.require_target_copy(backend, directory, value, ('api',))).start()

    def test_actual_context_constructor_reuses_the_single_full_plan_and_complete_ledger(self):
        with patch.object(clone, 'create_plan', wraps=clone.create_plan) as parse:
            context = context_module.Context(self.backend, self.directory, self.value, 2)
            self.assertEqual(parse.call_count, 1)
            self.assertIsInstance(context.verified_plan, clone.VerifiedPlan)
            self.assertEqual(context.ledger_state['status'], 'ledger_evidence_complete')
            self.assertEqual(clone.reuse_plan(self.backend, str(context.copy_root / 'plan.json'), context.verified_plan)[0]['plan'], context.copy['plan'])
            self.assertEqual(parse.call_count, 1)

    def test_changed_payload_between_publication_gate_and_context_reuse_is_rejected(self):
        def validate(backend, directory, value, _):
            token = run.require_target_copy(backend, directory, value, ('api',))
            (self.fixture.case.fixture.root / 'file.bin').write_bytes(b'evil')
            return token
        self.validate.side_effect = validate
        with patch.object(clone, 'create_plan', wraps=clone.create_plan) as parse:
            with self.assertRaises(ValueError): context_module.Context(self.backend, self.directory, self.value, 2)
            self.assertEqual(parse.call_count, 1)

    def test_latest_failed_copy_still_blocks_context_before_semantic_parsing(self):
        number = begin(self.directory, 'copy', 'reconcile', {})
        finish(self.directory, number, error=ValueError('离线模拟新核对失败'))
        with patch.object(clone, 'create_plan', wraps=clone.create_plan) as parse:
            with self.assertRaises(ValueError): context_module.Context(self.backend, self.directory, self.value, 3)
            self.assertEqual(parse.call_count, 0)

    def test_different_valid_plan_cannot_reuse_old_completed_result_and_ledger(self):
        source = self.fixture.output / 'input.json'
        value = json.loads(source.read_text(encoding='utf-8'))
        value['copy_id'] = 'another-copy'
        source.write_text(json.dumps(value), encoding='utf-8')
        wrapper = clone.inspect_input(self.backend, str(source))
        (self.fixture.output / 'plan.json').write_text(json.dumps(wrapper), encoding='utf-8')
        self.assertEqual(wrapper['plan']['pending_target_actions'], [])
        with self.assertRaises(ValueError):
            run.require_target_copy(self.backend, self.directory, self.value, ('api',))


if __name__ == '__main__':
    unittest.main()
