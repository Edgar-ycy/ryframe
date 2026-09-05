"""固定 post-copy 登记、来源绑定和 runtime 准备；只使用隔离文件与 mock。"""
from contextlib import nullcontext
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_post as post
import devex_clone_run as run
from devex_clone_capture import write_json
from devex_clone_factory_context import Environments
from devex_clone_post_context import Context
from devex_clone_post_model import administrator, environment_delta
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
        self.original = {'APP_ENV': 'test', 'APP_SCOPE_ID': 'target-one', 'APP_JOBS_MODE': 'external',
                         'APP_JOBS_SCHEDULER_ENABLED': 'false', 'APP_DATABASE_URL': 'test-only'}
        self.api = {**self.original, 'APP_JOBS_SCHEDULER_ENABLED': 'true', 'SNOWFLAKE_WORKER_ID': '1',
                    'APP_CORS_ALLOW_ORIGINS': 'http://127.0.0.1:4190', 'RYFRAME_TEST_ADMIN_PASSWORD': 'unit-only'}
        self.admin = dict(subject_id='9', tenant_id='system', username='admin',
                          password_env='RYFRAME_TEST_ADMIN_PASSWORD', client_address='198.19.20.1')
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
            'frontend_root': str(self.frontend), 'pacing': {'contract': {},
                'bindings': {'python': build['path'], 'login_budget_state': str(self.directory / 'post-copy/login-budget.json')},
                'python_sha256': build['sha256'], 'login_budget_module_sha256': budget_module['sha256']},
            'reference_plan': self.file('reference', reference),
            'dataset': self.file('dataset', {'plan_sha256': plan_hash(reference), 'source_scope_id': 'source-one'})}
        self.selected = {'api_url': 'http://127.0.0.1:18210', 'frontend_url': 'http://127.0.0.1:4190'}
        self.addCleanup(patch.stopall)
        patch('devex_clone_run.require_target_copy', return_value=SimpleNamespace(plan={
            'copy_stage': 'source_to_seed', 'source_scope': 'source-one'})).start()
        patch('devex_clone_factory_context.initialization_history', return_value=({'generation': {'selected': self.selected}}, {})).start()

    def write(self, path, value):
        write_json(path, value)
        return binding(path)

    def file(self, name, value):
        return self.write(self.local / (name + '.json'), value)

    def register(self):
        filename = self.file('request', self.request)
        number = begin(self.directory, 'post-copy', 'register', {})
        result = post.register(self.backend, self.directory, self.value, Path(filename['path']))
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

    def test_register_preserves_original_inputs_and_binds_own_stage(self):
        old = {field: binding(Path(self.request[field]['path'])) for field in ('run_manifest', 'copy_result', 'ledger_head', 'dataset')}
        result = self.register()
        self.assertEqual(result['status'], 'post_copy_registered')
        self.assertEqual(post.registered(self.backend, self.directory), self.request)
        self.assertEqual({key: binding(Path(item['path'])) for key, item in old.items()}, old)

    def test_wrong_latest_copy_receipt_ledger_or_build_rejected(self):
        for field in ('copy_stage_receipt', 'copy_result', 'ledger_head', 'backend_build'):
            with self.subTest(field=field):
                value = {**self.request, field: self.file('wrong-' + field, {'other': True})}
                with self.assertRaises(ValueError): post.validate_registration(self.backend, self.directory, self.value, value)

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
            'request': {'source': {'scope_id': 'seed-one'}},
            'manifest': {'source_request': original_request}}
        previous = SimpleNamespace(request={'reference_plan': self.request['reference_plan'],
                                             'dataset': self.request['dataset']})
        plan = {'copy_stage': 'seed_to_arm', 'source_scope': 'seed-one'}
        with patch('devex_clone_seed_source.published_source', return_value=inherited) as published, \
                patch.object(post, 'resolve_post_registration', return_value=previous) as resolved:
            post._validate_business_lineage(self.backend, value, self.request, plan)
        published.assert_called_once_with(self.backend, source_registration)
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

    def test_derivation_rejects_database_changes_disabled_routes_and_nonexternal_worker(self):
        environment_delta(self.original, self.api, self.selected['frontend_url'])
        for field, value in (('APP_DATABASE_URL', 'other'), ('APP_JOBS_MODE', 'embedded'),
                             ('APP_JOBS_SCHEDULER_ENABLED', 'false'), ('APP_SCOPE_ID', 'other'), ('SNOWFLAKE_WORKER_ID', '2')):
            with self.subTest(field=field), self.assertRaises(ValueError):
                environment_delta(self.original, {**self.api, field: value}, self.selected['frontend_url'])

    def test_admin_requires_explicit_password_env_and_actual_subject(self):
        administrator(self.admin, self.api)
        for admin in ({**self.admin, 'subject_id': '0'}, {**self.admin, 'password_env': 'RYFRAME_MISSING_PASSWORD'},
                      {**self.admin, 'tenant_id': 'other'}, {**self.admin, 'password': 'unit-only'}):
            with self.assertRaises(ValueError): administrator(admin, self.api)

    def test_budget_cannot_write_outside_fixed_run_or_use_unbound_module(self):
        request = {**self.request, 'pacing': {**self.request['pacing'], 'bindings': {
            **self.request['pacing']['bindings'], 'login_budget_state': str(self.local / 'other.json')}}}
        with self.assertRaises(ValueError): post.pacing_binding(self.backend, self.directory, request)
        request = {**self.request, 'pacing': {**self.request['pacing'], 'login_budget_module_sha256': '0' * 64}}
        with self.assertRaises(ValueError): post.pacing_binding(self.backend, self.directory, request)

    def test_unchanged_registration_retry_can_publish_but_other_content_never_overwrites(self):
        filename = self.file('request', self.request)
        result = post.register(self.backend, self.directory, self.value, Path(filename['path']))
        self.assertEqual(post.register(self.backend, self.directory, self.value, Path(filename['path'])), result)
        saved = binding(self.directory / 'post-copy.json')
        changed = self.file('changed-request', {**self.request, 'kind': 'other'})
        with patch.object(post, 'validate_registration'), self.assertRaises(ValueError):
            post.register(self.backend, self.directory, self.value, Path(changed['path']))
        self.assertEqual(binding(self.directory / 'post-copy.json'), saved)

    def test_cleanup_uses_minimal_registration_when_dataset_or_original_target_missing(self):
        self.register()
        runtime = self.prepared_runtime()
        Path(self.request['dataset']['path']).unlink()
        Path(self.value['target_environment']['path']).unlink()
        observed, env = post.runtime_inputs(self.backend, self.directory)
        self.assertEqual(observed['runtime_dir'], str(runtime))
        self.assertEqual(env, self.api)
        with patch('devex_clone_runtime.control', return_value={'status': 'stopped'}) as control:
            result = run.run_runtime(self.backend, self.value, Environments({}, {}), 'target', 'stop', ('api',), run_directory=self.directory)
        self.assertEqual(result['status'], 'stopped')
        control.assert_called_once()

    def test_post_runtime_always_rejects_worker(self):
        self.register()
        with patch('devex_clone_runtime.control') as control, self.assertRaises(ValueError):
            run.run_runtime(self.backend, self.value, Environments({}, {}), 'target', 'start', ('worker',), run_directory=self.directory)
        control.assert_not_called()

    def test_later_failed_registration_blocks_write_but_preserves_exact_cleanup(self):
        self.register()
        self.prepared_runtime()
        number = begin(self.directory, 'post-copy', 'register', {})
        finish(self.directory, number, error=ValueError('离线模拟来源终检失败'))
        with self.assertRaises(ValueError): post.registered(self.backend, self.directory)
        with patch('devex_clone_runtime.control', return_value={'status': 'stopped'}) as control, \
                patch('devex_clone_post_process.require_quiet', side_effect=ValueError('历史 Node 身份未知')) as producer_guard:
            observed = run.run_runtime(self.backend, self.value, Environments({}, {}), 'target', 'stop', ('api',), run_directory=self.directory)
        self.assertEqual(observed['status'], 'stopped')
        control.assert_called_once()
        producer_guard.assert_not_called()

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
        context.backend, context.value, context.directory_root = self.backend, self.value, self.directory
        context.output, context.runtime, context.run = self.directory, self.directory / 'runtime', None
        context.target, context.review, context.target_config = {}, {}, {'scope_id': 'target-one'}
        context.initial, context.environments = {'generation': {'physical': 'physical'}}, Environments({}, {})
        context.selected = {**self.selected, 'worker_ready_url': 'http://127.0.0.1:19210',
                            'runtime_dir': str(self.directory / 'old-runtime')}
        tools = SimpleNamespace(verify_databases=lambda _: None, verify_objects=lambda _: None)
        resources, external = SimpleNamespace(storage_identity=lambda: None, tools=tools), self.local / 'storage-run'
        with patch('devex_clone_post_context.source_binding', return_value='physical'), \
                patch('devex_clone_post_context.verify_api_address'), \
                patch('devex_clone_post_context.worker_ready_url', return_value=context.selected['worker_ready_url']), \
                patch('devex_clone_post_context.require_closed_port'), \
                patch('devex_clone_run.target_storage_run', return_value=external), \
                patch('devex_clone_post_context.Resources', return_value=resources) as created:
            self.assertIs(context.target_guard(api=False), resources)
        self.assertEqual(created.call_args.kwargs['storage_run'], external)

    def test_prepare_is_read_only_remote_and_reuses_exact_partial_local_runtime(self):
        context = Context.__new__(Context)
        context.backend, context.runtime = self.backend, self.directory / 'runtime'
        context.request_binding, context.selected = {'sha256': 'a' * 64}, self.selected
        context.environments = Environments({}, self.api)
        context.build = {'artifacts': {'api': {'executable': 'unit-api'}, 'worker': {'executable': 'unit-worker'}}}
        context.before_api = lambda: None
        context.target_guard = lambda **_: None
        context.bindings = lambda: {'artifacts': {'reset': {'executable': 'unit-reset'}, 'migrate': {'executable': 'unit-migrate'}}}
        def register_runtime(_backend, runtime):
            path = runtime / 'runtime.json'
            if not path.exists(): write_json(path, {'configuration': 'unit'})
        with patch('devex_clone_post_context.register_runtime', side_effect=register_runtime):
            first = context.prepare()
            self.assertEqual(context.prepare(), first)
            path = context.runtime / 'binaries.json'
            value = json.loads(path.read_text(encoding='utf-8'))
            value['ryframe'] = 'other-api'
            path.write_text(json.dumps(value), encoding='utf-8')
            with self.assertRaises(ValueError): context.prepare()

    def test_prepare_refuses_existing_producer_history(self):
        context = Context.__new__(Context)
        context.runtime = self.directory / 'runtime'
        context.runtime.mkdir()
        write_json(context.runtime / 'producer-history.json', {'event': 'started'})
        context.before_api = lambda: None
        context.bindings = lambda: {}
        with self.assertRaises(ValueError): context.prepare()

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
