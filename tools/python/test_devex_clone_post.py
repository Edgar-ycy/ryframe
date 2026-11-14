"""固定 post-copy 登记、来源绑定和 runtime 准备；只使用隔离文件与 mock。"""
from contextlib import contextmanager, nullcontext
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_post as post
from devex_clone_capture import write_json
from process_environment import Environments
from devex_clone_post_context import Context
from devex_clone_run_state import binding, initialize_state, begin, finish, load_state
from restore_reference_plan import plan_hash


class PostTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        self.backend = root / 'backend'
        self.frontend = root / 'ryframe-vue3'
        self.frontend.mkdir()
        (self.frontend / 'scripts').mkdir()
        budget_module = self.write(self.frontend / 'scripts/browser-login-budget.mjs', {'fixture': True})
        self.local = self.backend / '.local-tests'
        self.directory = self.local / 'run'
        self.directory.mkdir(parents=True)
        self.copy = self.local / 'copy'
        (self.copy / 'ledger').mkdir(parents=True)
        self.write(self.copy / 'result.json', {'status': 'data_steps_verified'})
        self.write(self.copy / 'ledger/head.json', {'sequence': 2})
        self.original = {'APP_ENV': 'test'}
        self.api = {'APP_ENV': 'test'}
        self.admin = dict(subject_id='9', tenant_id='system', username='admin',
                          client_address='198.19.20.1')
        credentials = self.local / 'administrator.toml'
        credentials.write_text('[administrator]\ntenant_id="system"\nusername="admin"\npassword="Current!Changed123"\n', encoding='utf-8')
        self.credentials = binding(credentials)
        build = self.file('build', {'kind': 'fixture-build'})
        bridge = self.file('bridge', {'build': {key: build[key] for key in ('path', 'sha256')}})
        self.value = {'copy_directory': str(self.copy), 'initialized': self.file('initialized', {'fixture': True}),
            'target_environment': self.file('target', {'environment': self.original}),
            'copy_stage': 'source_to_seed',
            'build_bridges': [{key: bridge[key] for key in ('path', 'sha256')}]}
        self.write(self.directory / 'manifest.json', self.value)
        initialize_state(self.directory)
        number = begin(self.directory, 'copy', 'resume', {})
        finish(self.directory, number, result={'status': 'data_steps_verified'})
        reference = {'source': {'scope_id': 'source-one'}}
        self.request = {'format_version': 1, 'kind': 'devex-clone-post-copy',
            'run_manifest': binding(self.directory / 'manifest.json'),
            'copy_stage_receipt': load_state(self.directory)['attempts'][-1]['result'],
            'copy_result': binding(self.copy / 'result.json'), 'ledger_head': binding(self.copy / 'ledger/head.json'),
            'api_environment': self.file('api', {'environment': self.api}), 'backend_build': build,
            'node': {key: build[key] for key in ('path', 'sha256')}, 'source_admin': self.admin,
            'administrator_credentials': self.credentials,
            'frontend_root': str(self.frontend), 'pacing': {'contract': {},
                'bindings': {'login_budget_state': str(self.directory / 'post-copy/login-budget.json')},
                'login_budget_module_sha256': budget_module['sha256']},
            'reference_plan': self.file('reference', reference),
            'dataset': self.file('dataset', {'plan_sha256': plan_hash(reference), 'source_scope_id': 'source-one'})}
        self.selected = {'api_url': 'http://127.0.0.1:18210', 'frontend_url': 'http://127.0.0.1:4190'}
        self.addCleanup(patch.stopall)
        # 文件绑定的正负路径由 Rust 行为测试核验；本宿主保留登记/代次/Context 保障。
        patch.object(post, 'native_invoke', return_value=(
            SimpleNamespace(returncode=0), {"ok": True, "result": {"value": None}})).start()
        patch('devex_clone_run.require_target_copy', return_value=SimpleNamespace(plan={
            'copy_stage': 'source_to_seed', 'source_scope': 'source-one'})).start()
        patch('devex_clone_factory_context.initialization_history', return_value=({'generation': {'selected': self.selected}}, {})).start()

    def write(self, path, value):
        write_json(path, value)
        return binding(path)

    def file(self, name, value):
        return self.write(self.local / (name + '.json'), value)

    def register(self):
        number = begin(self.directory, 'post-copy', 'register', {})
        self.write(self.directory / 'post-copy.json', self.request)
        result = {'status': 'post_copy_registered', 'registration': binding(self.directory / 'post-copy.json'),
                  'remote_writes': 0, 'worker_must_remain_stopped': True, 'restore_qualified': False}
        finish(self.directory, number, result=result)
        return result

    def prepared_runtime(self):
        runtime = self.directory / 'post-copy/runtime'
        runtime.mkdir(parents=True)
        self.write(runtime / 'runtime.json', {'fixture': 'runtime'})
        self.write(runtime / 'binaries.json', {'fixture': 'binaries'})
        handoff = {'registration': binding(self.directory / 'post-copy.json'),
            'runtime': binding(runtime / 'runtime.json'), 'binaries': binding(runtime / 'binaries.json'),
            'api_url': self.selected['api_url'], 'worker_started': False}
        self.write(runtime / 'handoff.json', handoff)
        number = begin(self.directory, 'post-copy', 'prepare', {})
        finish(self.directory, number, result={'status': 'post_copy_api_prepared',
               'handoff': binding(runtime / 'handoff.json'), 'worker_started': False})
        return runtime

    def prepared_environment(self, name, initial, api):
        self.directory = self.local / name
        self.directory.mkdir()
        self.value['target_environment'] = self.file(name + '-target', {'environment': initial})
        self.request['api_environment'] = self.file(name + '-api', {'environment': api})
        for descriptor in (self.value['target_environment'], self.request['api_environment']):
            descriptor['path'] = Path(descriptor['path']).as_posix()
        self.request['run_manifest'] = self.write(self.directory / 'manifest.json', self.value)
        initialize_state(self.directory)
        number = begin(self.directory, 'copy', 'run', {})
        finish(self.directory, number, result={'status': 'data_steps_verified'})
        self.request['copy_stage_receipt'] = load_state(self.directory)['attempts'][-1]['result']
        self.register()
        return self.prepared_runtime()

    def test_seed_to_arm_inherits_original_business_bindings(self):
        source_request = self.file('seed-source-request', {'source': True})
        source_environment = self.file('seed-source-environment', {'environment': {}})
        source_registration = self.file('seed-source-registration', {'published': True})
        original_request = self.file('original-source-request', {'source': {'scope_id': 'source-one'}})
        value = {**self.value, 'copy_stage': 'seed_to_arm', 'source_request': source_request,
                 'source_environment': source_environment, 'source_registration': source_registration}
        inherited = {'directory': self.local / 'seed-run',
            'registration': {'source_request': source_request, 'source_environment': source_environment,
                             'post_copy': self.file('old-post', {'post': True})},
            'source_request': source_request,
            'source_environment': source_environment,
            'request': {'source': {'scope_id': 'seed-one'}},
            'manifest': {'source_request': original_request}}
        previous = SimpleNamespace(request={'reference_plan': self.request['reference_plan'],
                                             'dataset': self.request['dataset']})
        plan = {'copy_stage': 'seed_to_arm', 'source_scope': 'seed-one'}
        with patch('devex_clone_seed_source.published_source', return_value=inherited) as published, \
                patch.object(post, 'resolve_post_registration', return_value=previous) as resolved:
            post._validate_business_lineage(self.backend, value, self.request, plan)
        published.assert_called_once_with(self.backend, source_registration, live_storage=False)
        resolved.assert_called_once_with(self.backend, inherited['directory'],
                                         descriptor=inherited['registration']['post_copy'], cleanup=True)

    def test_seed_to_arm_rejects_detached_source_and_business_bindings(self):
        source_request = self.file('seed-source-request', {'source': True})
        source_environment = self.file('seed-source-environment', {'environment': {}})
        value = {**self.value, 'copy_stage': 'seed_to_arm', 'source_request': source_request,
                 'source_environment': source_environment,
                 'source_registration': self.file('seed-source-registration', {'published': True})}
        inherited = {'directory': self.local / 'seed-run',
            'registration': {'source_request': source_request, 'source_environment': source_environment,
                             'post_copy': self.file('old-post', {'post': True})},
            'source_request': source_request,
            'source_environment': source_environment,
            'request': {'source': {'scope_id': 'seed-one'}},
            'manifest': {'source_request': self.file('original-source-request', {'source': {'scope_id': 'source-one'}})}}
        detached = SimpleNamespace(request={'reference_plan': self.file('other-plan', {'source': {'scope_id': 'source-one'}}),
                                            'dataset': self.request['dataset']})
        with patch('devex_clone_seed_source.published_source', return_value=inherited), \
                patch.object(post, 'resolve_post_registration', return_value=detached), self.assertRaises(ValueError):
            post._validate_business_lineage(self.backend, value, self.request,
                                            {'copy_stage': 'seed_to_arm', 'source_scope': 'seed-one'})
        with patch('devex_clone_seed_source.published_source', return_value=inherited), self.assertRaises(ValueError):
            post._validate_business_lineage(self.backend, {**value, 'source_request': self.request['dataset']},
                                            self.request, {'copy_stage': 'seed_to_arm', 'source_scope': 'seed-one'})

    def test_cleanup_uses_minimal_registration_when_dataset_or_original_target_missing(self):
        self.register()
        runtime = self.prepared_runtime()
        Path(self.request['dataset']['path']).unlink()
        Path(self.value['target_environment']['path']).unlink()
        observed, env = post.runtime_inputs(self.backend, self.directory, cleanup=True)
        self.assertEqual(observed['runtime_dir'], str(runtime))
        self.assertEqual(env, self.api)

    def test_later_failed_registration_blocks_write_but_preserves_exact_cleanup(self):
        self.register()
        runtime = self.prepared_runtime()
        number = begin(self.directory, 'post-copy', 'register', {})
        finish(self.directory, number, error=ValueError('离线模拟来源终检失败'))
        with self.assertRaises(ValueError): post.registered(self.backend, self.directory)
        observed, environment = post.runtime_inputs(self.backend, self.directory, cleanup=True)
        self.assertEqual(observed['runtime_dir'], str(runtime))
        self.assertEqual(environment, self.api)

    def test_target_guard_rejects_existing_copy_lock_without_resource_calls(self):
        target = Path(self.value['initialized']['path']).parent
        (target / 'initialize.lock').mkdir()
        with patch('devex_clone_run.target_storage_control', return_value=nullcontext()) as storage, \
                self.assertRaises(ValueError):
            with post.target_lock(self.backend, self.value, current_run=self.directory):
                self.fail('不能取得互斥')
        storage.assert_called_once_with(self.backend, self.directory, self.value)

    def test_arm_target_guard_uses_external_storage_run(self):
        context = Context.__new__(Context)
        context.target_descriptor = {"bound": "request"}
        context.backend, context.value, context.directory_root = self.backend, self.value, self.directory
        context.output, context.runtime, context.run = self.directory, self.directory / 'runtime', None
        (self.backend / 'config').mkdir(exist_ok=True)
        context.target, context.review, context.target_config = {'config_dir': str(self.backend / 'config')}, {}, {'scope_id': 'target-one'}
        context.initial, context.environments = {'generation': {'physical': 'physical'}}, Environments({}, {})
        context.selected = {**self.selected, 'worker_ready_url': 'http://127.0.0.1:19210',
                            'runtime_dir': str(self.directory / 'old-runtime')}
        tools = SimpleNamespace(verify_databases=lambda _: None, verify_objects=lambda _: None)
        resources, external = SimpleNamespace(storage_identity=lambda: None, tools=tools), self.local / 'storage-run'
        with patch('devex_clone_post_context.native_invoke', return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": 'physical'}})), \
                patch('devex_clone_post_context.execution_root', return_value=self.backend), \
                patch('devex_clone_post_context.verify_api_address'), \
                patch('devex_clone_post_context.worker_ready_url', return_value=context.selected['worker_ready_url']), \
                patch('devex_clone_post_context.require_closed_port'), \
                patch('devex_clone_run.target_storage_run', return_value=external), \
                patch('devex_clone_post_context.Resources', return_value=resources) as created:
            self.assertIs(context.target_guard(api=False), resources)
        self.assertEqual(created.call_args.kwargs['storage_run'], external)

    def test_source_to_seed_target_guard_uses_fixture_storage_run(self):
        context = Context.__new__(Context)
        context.target_descriptor = {"bound": "request"}
        context.backend, context.value, context.directory_root = self.backend, self.value, self.directory
        context.output, context.runtime, context.run = self.directory, self.directory / 'runtime', None
        (self.backend / 'config').mkdir(exist_ok=True)
        context.target, context.review, context.target_config = {'config_dir': str(self.backend / 'config')}, {}, {'scope_id': 'target-one'}
        context.initial, context.environments = {'generation': {'physical': 'physical'}}, Environments({}, {})
        context.owned_lock_identity = 4242
        context.selected = {**self.selected, 'worker_ready_url': 'http://127.0.0.1:19210',
                            'runtime_dir': str(self.directory / 'old-runtime')}
        tools = SimpleNamespace(verify_databases=lambda _: None, verify_objects=lambda _: None)
        resources = SimpleNamespace(storage_identity=lambda: None, tools=tools)
        fixture = self.local / 'fixture-service-run'
        with patch('devex_clone_post_context.native_invoke', return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": 'physical'}})), \
                patch('devex_clone_post_context.execution_root', return_value=self.backend), \
                patch('devex_clone_post_context.verify_api_address'), \
                patch('devex_clone_post_context.worker_ready_url', return_value=context.selected['worker_ready_url']), \
                patch('devex_clone_post_context.require_closed_port'), \
                patch('devex_clone_run.target_storage_run', return_value=None), \
                patch('devex_clone_run.target_fixture_storage_run', return_value=fixture) as pinned, \
                patch('devex_clone_post_context.Resources', return_value=resources) as created:
            self.assertIs(context.target_guard(api=False), resources)
        pinned.assert_called_once_with(self.backend, context.value['initialized'], owned_lock_identity=4242)
        self.assertEqual(created.call_args.kwargs['storage_run'], fixture)

    def test_resolved_storage_run_falls_back_to_the_current_run(self):
        context = Context.__new__(Context)
        context.backend, context.value, context.directory_root = self.backend, self.value, self.directory
        context.owned_lock_identity = 4242
        with patch('devex_clone_run.target_storage_run', return_value=None), \
                patch('devex_clone_run.target_fixture_storage_run', return_value=None) as pinned:
            self.assertEqual(context.resolved_storage_run(), self.directory)
        pinned.assert_called_once_with(self.backend, context.value['initialized'], owned_lock_identity=4242)

    def test_api_guard_requires_exact_executable_and_verified_contract_configuration_arguments(self):
        temporary = self.local / '私有 临时目录'
        temporary.mkdir()
        environment = {'APP_ENV': 'test', 'TEMP': str(temporary), 'TMP': str(temporary)}
        context = Context.__new__(Context)
        context.runtime = self.prepared_environment('api-frozen', environment, environment)
        context.backend, context.directory_root = self.backend, self.directory
        context.selected = self.selected
        context.target_config = {'scope_id': 'target-one'}
        executable = str(self.local / 'api.exe')
        identity = {'pid': 21000, 'started': '1234567', 'executable': executable}
        self.write(context.runtime / 'api.json', {'identity': identity})
        context.build = {'artifacts': {
            'api': {'executable': executable, 'sha256': 'a' * 64},
            'worker': {'executable': str(self.local / 'worker.exe'), 'sha256': 'b' * 64}}}
        configuration = str(self.local / '已绑定 配置/config')
        contract = {'api_config_dir': configuration, 'artifacts': {
            role: {'path': value['executable'], 'sha256': value['sha256']}
            for role, value in context.build['artifacts'].items()}}
        with patch('devex_clone_post_context.verify_runtime', return_value=contract), \
                patch('devex_clone_post_context.read_process', return_value=identity), \
                patch('devex_clone_post_context.process_identity', return_value=identity), \
                patch('devex_clone_post_context.evidence') as evidence, \
                patch('devex_clone_post_context.verify_listener') as listener:
            for arguments, valid in (
                    ([executable, '--config-dir', configuration], True),
                    ([executable], False),
                    ([executable, '--config-dir', str(self.local / 'other-config')], False)):
                with self.subTest(arguments=arguments):
                    listener.reset_mock()
                    evidence.return_value = {'actual_arguments_sha256': plan_hash(arguments)}
                    if valid:
                        context.api_guard(SimpleNamespace())
                        listener.assert_called_once_with(identity['pid'], context.selected['api_url'])
                    else:
                        with self.assertRaisesRegex(ValueError, 'API 内核身份、产物或实际命令发生变化'):
                            context.api_guard(SimpleNamespace())
                        listener.assert_not_called()

    def test_api_guard_rejects_changed_or_invalid_frozen_environment_before_process_reads(self):
        temporary = self.local / 'private-temp'
        temporary.mkdir()
        original = {'APP_ENV': 'test', 'TEMP': str(temporary), 'TMP': str(temporary)}
        missing = str(self.local / 'missing')
        outside = str(self.frontend)
        cases = (
            ('removed', original, {'APP_ENV': 'test'}, 'frozen_context_changed'),
            ('changed', original, {**original, 'TEMP': missing, 'TMP': missing}, 'frozen_context_changed'),
            ('injected', original, {**original, 'APP_JOBS_MODE': 'embedded'}, 'api_private'),
            ('selector', original, {**original, 'APP_ENV': 'dev'}, 'api_private'),
            ('unpaired', {'APP_ENV': 'test', 'TEMP': str(temporary)}, None, 'TEMP/TMP'),
            ('unequal', {**original, 'TMP': missing}, None, 'TEMP/TMP'),
            ('missing', {**original, 'TEMP': missing, 'TMP': missing}, None, 'TEMP/TMP'),
            ('outside', {**original, 'TEMP': outside, 'TMP': outside}, None, 'TEMP/TMP'),
            ('relative', {**original, 'TEMP': 'private-temp', 'TMP': 'private-temp'}, None, 'TEMP/TMP'),
            ('duplicate', {**original, 'temp': str(temporary)}, None, 'initial_private'),
        )
        with patch('devex_clone_post_context.verify_runtime') as verify:
            for name, initial, api, reason in cases:
                with self.subTest(name=name):
                    context = Context.__new__(Context)
                    context.runtime = self.prepared_environment('api-' + name, initial, initial if api is None else api)
                    context.backend, context.directory_root, context.selected = self.backend, self.directory, self.selected
                    with self.assertRaisesRegex(ValueError, reason):
                        context.api_guard(SimpleNamespace())
            context.runtime = self.prepared_environment('api-missing-receipt', original, original)
            context.directory_root = self.directory
            Path(self.value['target_environment']['path']).unlink()
            with self.assertRaises((OSError, ValueError)):
                context.api_guard(SimpleNamespace())
            verify.assert_not_called()



    def test_latest_failed_reconciliation_blocks_old_successful_schedule_stage(self):
        self.register()
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory,
                                  request_binding=binding(self.directory / 'post-copy.json'))
        number = begin(self.directory, 'post-copy', 'schedules', {})
        finish(self.directory, number, result={'status': 'target_schedule_actions_verified',
                'registration': context.request_binding, 'confirmed': []})
        post.require_schedule_stage(context, [])
        number = begin(self.directory, 'post-copy', 'reconcile', {})
        finish(self.directory, number, error=ValueError('离线核对失败'))
        with self.assertRaises(ValueError): post.require_schedule_stage(context, [])

    def test_published_schedule_result_must_match_exact_confirmations(self):
        self.register()
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory,
                                  request_binding=binding(self.directory / 'post-copy.json'))
        number = begin(self.directory, 'post-copy', 'schedules', {})
        finish(self.directory, number, result={'status': 'target_schedule_actions_verified',
                'registration': context.request_binding, 'confirmed': [{'sha256': '1' * 64}]})
        with self.assertRaises(ValueError): post.require_schedule_stage(context, [])


if __name__ == '__main__':
    unittest.main()
