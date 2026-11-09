"""Context 消费真实已验证计划与完成账本；所有远端端口使用离线 fixture。"""
import copy
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
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
        target_request = self.fixture.bind(self.fixture.target / 'request.json', self.fixture.target_request)
        write_json(self.fixture.target / 'prepare.json', {'request': target_request})
        env = {'APP_ENV': 'test'}
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
        patch.object(context_module, 'registration', return_value=SimpleNamespace(
            request=self.request, descriptor=binding(self.directory / 'post-copy.json'),
            runtime=self.directory / 'post-copy/runtime')).start()
        patch.object(context_module, 'initialization_history', return_value=(self.fixture.initial, self.fixture.target_request)).start()
        patch.object(context_module, 'request_binding', return_value=({}, self.fixture.initial['generation']['selected'])).start()
        patch.object(context_module, 'verify_source_export', return_value=source).start()
        # 这里只隔离已经在登记单测覆盖的字段检查；正式发布/账本门禁和语义解析都实际执行。
        self.validate = patch.object(context_module, 'validate_registration',
                                     side_effect=lambda backend, directory, value, _, owned_lock_identity=None:
                                     run.require_target_copy(backend, directory, value, ('api',))).start()

    def test_context_retains_verified_plan_and_complete_copy_ledger(self):
        context = context_module.Context(self.backend, self.directory, self.value, 2)
        self.assertIsInstance(context.verified_plan, clone.VerifiedPlan)
        self.assertEqual(context.ledger_state['status'], 'ledger_evidence_complete')
        self.assertEqual(context.verified_plan.plan, context.copy['plan'])

    def test_changed_payload_between_publication_gate_and_context_reuse_is_rejected(self):
        payload = self.fixture.case.fixture.root / 'file.bin'
        def validate(backend, directory, value, _, owned_lock_identity=None):
            token = run.require_target_copy(backend, directory, value, ('api',))
            self.assertIsInstance(token, clone.VerifiedPlan)
            payload.write_bytes(b'evil')
            return token
        self.validate.side_effect = validate
        with self.assertRaisesRegex(ValueError, '^复制证据已变化或未完整写入$'):
            context_module.Context(self.backend, self.directory, self.value, 2)
        self.assertEqual(payload.read_bytes(), b'evil')

    def test_latest_failed_copy_blocks_context_instead_of_reusing_old_success(self):
        number = begin(self.directory, 'copy', 'reconcile', {})
        finish(self.directory, number, error=ValueError('离线模拟新核对失败'))
        with self.assertRaisesRegex(ValueError, '^最新复制阶段尚未通过完整来源和控制器收尾核验$'):
            context_module.Context(self.backend, self.directory, self.value, 3)

    def test_different_valid_plan_cannot_reuse_old_completed_result_and_ledger(self):
        source = self.fixture.output / 'input.json'
        value = json.loads(source.read_text(encoding='utf-8'))
        value['copy_id'] = 'another-copy'
        source.write_text(json.dumps(value), encoding='utf-8')
        wrapper = clone.inspect_input(self.backend, str(source))
        (self.fixture.output / 'plan.json').write_text(json.dumps(wrapper), encoding='utf-8')
        self.assertEqual(wrapper['plan']['pending_target_actions'], [])
        with self.assertRaisesRegex(ValueError, '^目标数据复制尚未发布有效的完整账本收据$'):
            run.require_target_copy(self.backend, self.directory, self.value, ('api',))


if __name__ == '__main__':
    unittest.main()
