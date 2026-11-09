"""配额 bridge 的固定参数与原 post 会话初始化兼容职责，不执行 HTTP。"""
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from devex_clone_post_actions import Bridge
from devex_clone_post_process import KINDS, PHASES, producer_command


class QuotaProducerContractTests(unittest.TestCase):
    def setUp(self):
        from rate_limit_tool import tool_binding, verified_tool
        selected = tool_binding()
        self.assertIsNotNone(selected, "必须显式绑定本次编译的原生工具")
        with verified_tool(selected):
            native = {key: selected[key] for key in ("path", "bytes", "sha256")}
        previous = __import__('devex_clone_post_process')._TEST_TOOL
        __import__('devex_clone_post_process')._TEST_TOOL = native
        self.addCleanup(setattr, __import__('devex_clone_post_process'), '_TEST_TOOL', previous)
    def test_quota_kinds_bind_only_seed_modes_identity_plan_and_sha(self):
        root = Path(__file__).resolve().parents[2]
        native = __import__('devex_clone_post_process')._TEST_TOOL
        request = {'identity_tool': {'path': native['path'], 'sha256': native['sha256']},
                   'identity_plan': {'path': str(root / 'plan.json'), 'sha256': 'a' * 64}}
        for mode in ('plan', 'apply', 'reconcile'):
            kind = 'quota-' + mode
            self.assertEqual(KINDS[kind], 'xtask/src/data/performance_identities/clone_bridge.rs')
            self.assertEqual(PHASES[kind], ('seed-runtime', {'quotas-' + mode}))
            argv = producer_command(root, request, root / 'run', 4, kind, root / 'output')
            self.assertEqual(argv[2:], ['--run-dir', str(root / 'run'), '--attempt', '4', '--quota-mode', mode,
                '--identity-plan', request['identity_plan']['path'], '--identity-plan-sha256', 'a' * 64])
        with self.assertRaises(ValueError):
            producer_command(root, request, root / 'run', 4, 'arbitrary-http', root / 'output')

    def test_bridge_post_request_is_only_initialization_and_seed_registration_stays_exact(self):
        root = Path.cwd()
        seed = {'identity_plan': {'path': 'fixed'}}
        post = {'pacing': {'contract': {'delay': 1}, 'bindings': {'python': 'fixed'}},
                'frontend_root': str(root / 'frontend'), 'source_admin': {
                    'subject_id': '1', 'tenant_id': 'system', 'username': 'admin',
                    'client_address': '198.19.20.1'}}
        bridge = Bridge.__new__(Bridge)
        config_dir = root / 'execution/config'
        target = {'config_dir': str(config_dir), 'execution_backend': {'path': str(root / 'execution')}}
        bridge.context = SimpleNamespace(request=seed, backend=root, directory_root=root / 'run',
            selected={'api_url': 'http://127.0.0.1:9000', 'frontend_url': 'http://127.0.0.1:4000'},
            target=target, target_config={'scope_id': 'fixture'}, output=root / 'run/seed-runtime/attempt-0004')
        configuration = patch('devex_clone_post_actions.application_config_directory', return_value=config_dir)
        selected_config = configuration.start()
        self.addCleanup(configuration.stop)
        bridge.producer_owner = None
        bridge.call = Mock(return_value={'subject_id': '1'})
        tenants = ['system', *['tenant-' + str(index) for index in range(10)]]
        self.assertEqual(bridge.login(post_request=post, tenant_ids=tenants), {'subject_id': '1'})
        self.assertIs(bridge.context.request, seed)
        bridge.call.assert_called_once()
        self.assertEqual(bridge.call.call_args.args, ('initialize',))
        arguments = bridge.call.call_args.kwargs
        self.assertEqual(arguments['tenant_ids'], tenants)
        self.assertEqual(arguments['identity'], post['source_admin'])
        self.assertEqual(arguments['attempt'], 4)
        self.assertEqual(arguments['run_dir'], str(root / 'run'))
        self.assertEqual(arguments['config']['contract']['pacing'], post['pacing']['contract'])
        self.assertEqual(arguments['frontend'], post['frontend_root'])
        self.assertEqual(arguments['config']['bindings'], {
            'scope_id': 'fixture', 'api_url': bridge.context.selected['api_url'],
            'frontend_url': bridge.context.selected['frontend_url'], 'pacing': post['pacing']['bindings'],
            'app_config_dir': str(config_dir)})
        selected_config.assert_called_once_with(root, target)
        bridge.context.request = post
        bridge.call.reset_mock()
        bridge.login()
        self.assertNotIn('tenant_ids', bridge.call.call_args.kwargs)
        self.assertEqual(bridge.call.call_args.kwargs['config']['bindings']['app_config_dir'], str(config_dir))
        self.assertIs(bridge.context.target, target)
        with self.assertRaises(ValueError):
            bridge.login(template_identities=[])
        bridge.call.reset_mock()
        selected_config.side_effect = ValueError('目标配置绑定已漂移')
        with self.assertRaisesRegex(ValueError, '配置绑定已漂移'):
            bridge.login()
        bridge.call.assert_not_called()


if __name__ == '__main__':
    unittest.main()
