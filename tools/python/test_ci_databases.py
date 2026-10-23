"""CI 协调器协议测试；MySQL 校验和进程场景由 ci::databases 的 Rust 测试覆盖。"""
import json
import os
import sys
import tomllib
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_full_stack_databases as databases
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / '.local-tests/python-unit'
TEMP.mkdir(parents=True, exist_ok=True)


class NativeCiDatabaseProtocolTests(unittest.TestCase):
    def test_frozen_plan_uses_private_request_and_explicit_configuration(self):
        with WorkspaceDirectory(TEMP, prefix='ci-protocol-') as root:
            source = Path(root) / 'input'
            source.mkdir()
            (source / 'app.toml').write_text('[tenant_data]\n', encoding='utf-8')
            (source / 'app.test.toml').write_text('scope_id="ci-42-3"\n', encoding='utf-8')
            settings = {'scope_id': 'ci-42-3', 'database': {'primary': {'password': 'private-password'}}}
            context = {'APP_ENV': 'test', 'GITHUB_ACTIONS': 'true', 'GITHUB_RUN_ID': '42', 'GITHUB_RUN_ATTEMPT': '3'}
            output = Path(root) / 'output'
            with patch.object(databases, 'load_app_table', return_value=settings) as loader, patch.object(databases, '_native') as native:
                native.return_value = {'scope_id': 'ci-42-3'}
                plan = databases.plan_databases(context.__getitem__, ROOT, output, config_dir=source, container_id='a' * 64)
                loader.assert_called_once_with(ROOT, {'APP_ENV': 'test'}, config_dir=source)
                self.assertEqual(native.call_args.args[0], 'plan')
                self.assertNotIn('private-password', repr(plan))
                (source / 'app.test.toml').write_text('scope_id="foreign"', encoding='utf-8')
                settings['database']['primary']['password'] = 'changed'
                native.return_value = {'config_dir': str(output / 'config')}
                self.assertEqual(databases.prepare_databases(context.__getitem__, ROOT, output, plan=plan), output / 'config')
                request = native.call_args.args[1]
                self.assertEqual(request['settings']['database']['primary']['password'], 'private-password')
                self.assertIn('ci-42-3', request['environment_text'])
                self.assertNotIn('foreign', request['environment_text'])
                with self.assertRaisesRegex(ValueError, '执行目录'):
                    databases.prepare_databases(context.__getitem__, ROOT, output / 'other', plan=plan)
            self.assertFalse(output.exists())

    def test_missing_explicit_directory_never_loads_defaults(self):
        with patch.object(databases, 'load_app_table') as loader, self.assertRaisesRegex(ValueError, '显式绑定'):
            databases.prepare_databases(lambda key: 'test', ROOT, ROOT / '.local-tests/unused')
        loader.assert_not_called()

    def process(self, response, code=0):
        process = Mock(returncode=code)
        process.communicate.return_value = (json.dumps(response).encode(), None)
        return process

    def test_credentials_are_only_in_stdin_and_host_overrides_are_not_forwarded(self):
        process = self.process({'ok': True, 'result': {'scope_id': 'ci-42-3'}})
        with patch.object(databases, 'verified_tool', return_value=nullcontext('bound-xtask')), patch.object(databases.subprocess, 'Popen', return_value=process) as launch, patch.dict(os.environ, {'APP_DATABASE_PASSWORD': 'host-secret', 'MYSQL_PWD': 'host-secret'}):
            result = databases._native('plan', {'password': 'private-password'})
        self.assertEqual(result, {'scope_id': 'ci-42-3'})
        self.assertEqual(launch.call_args.args[0], ['bound-xtask', '__ci-databases', 'plan'])
        self.assertNotIn('host-secret', str(launch.call_args))
        self.assertNotIn('private-password', str(launch.call_args))
        self.assertIn(b'private-password', process.communicate.call_args.args[0])

    def test_unknown_write_is_not_retried_and_public_error_is_preserved(self):
        process = self.process({'ok': False, 'error': {'code': 'creation_unknown', 'message': '创建结果未知，禁止自动重放'}}, 1)
        with patch.object(databases, 'verified_tool', return_value=nullcontext('bound-xtask')), patch.object(databases.subprocess, 'Popen', return_value=process) as launch, self.assertRaisesRegex(RuntimeError, '创建结果未知'):
            databases._native('prepare', {})
        self.assertEqual(launch.call_count, 1)

    def test_malformed_or_inconsistent_protocol_is_rejected(self):
        for response, code in [({'ok': True, 'result': {}}, 1), ({'ok': False, 'error': 'secret'}, 1), ({'other': True}, 0)]:
            with self.subTest(response=response), patch.object(databases, 'verified_tool', return_value=nullcontext('bound-xtask')), patch.object(databases.subprocess, 'Popen', return_value=self.process(response, code)), self.assertRaises(RuntimeError):
                databases._native('plan', {})

    def test_timeout_requests_cancellation_before_kill_without_replay(self):
        process = self.process({})
        process.communicate.side_effect = [databases.subprocess.TimeoutExpired('hidden', 180), (b'', None)]
        with patch.object(databases, 'verified_tool', return_value=nullcontext('bound-xtask')), patch.object(databases.subprocess, 'Popen', return_value=process) as launch, self.assertRaises(databases.subprocess.TimeoutExpired):
            databases._native('prepare', {})
        process.send_signal.assert_called_once()
        process.kill.assert_not_called()
        self.assertEqual(launch.call_count, 1)

    def test_oversized_request_is_rejected_before_launch(self):
        with patch.object(databases.subprocess, 'Popen') as launch, self.assertRaises(ValueError):
            databases._native('plan', {'password': 'x' * databases.LIMIT})
        launch.assert_not_called()

    def test_shared_worker_override_retains_nested_fields(self):
        for key in ('snowflake_worker_id', '"snowflake_worker_id"', "'snowflake_worker_id'"):
            parsed = tomllib.loads(databases._instance_text(f'{key}=3\n[app]\nsnowflake_worker_id=99\n', 2))
            self.assertEqual(parsed['snowflake_worker_id'], 2)
            self.assertEqual(parsed['app']['snowflake_worker_id'], 99)


class CiDatabaseWorkflowTests(unittest.TestCase):
    def test_workflow_uses_private_fixture_and_excludes_credential_artifacts(self):
        import yaml
        workflow = yaml.safe_load((ROOT / '.github/workflows/extended-ci.yml').read_text(encoding='utf-8'))
        job = workflow['jobs']['full-stack-e2e']
        self.assertFalse(any(key.startswith('APP_DATABASE_') for key in job['env']))
        fixture = next(step for step in job['steps'] if step['name'] == '写入当前 Job 的隔离数据库文件配置')
        self.assertIn('umask 077', fixture['run'])
        self.assertIn('> .local-tests/full-stack-config/app.test.toml', fixture['run'])
        import re
        document = re.search(r'app\.test\.toml <<EOF\n(.*?)\nEOF', fixture['run'], re.S)[1]
        document = (document.replace('${GITHUB_RUN_ID}', '42').replace('${GITHUB_RUN_ATTEMPT}', '3')
                    .replace('$RUNNER_TEMP', '/tmp/runtime').replace('$GITHUB_SHA', 'a' * 40))
        document = re.sub(r'\$\(cat [^\n]+\)', '19210', document)
        settings = tomllib.loads(document)
        self.assertEqual(settings['snowflake_worker_id'], 3)
        self.assertEqual(settings['app'], {'host': '127.0.0.1', 'port': 8080})
        self.assertEqual(settings['jobs'], {'mode': 'external', 'health_host': '127.0.0.1', 'health_port': 19210})
        self.assertFalse(settings['api_docs']['enabled'])
        self.assertTrue(settings['multi_tenancy']['enabled'])
        self.assertEqual(settings['redis']['database'], 14)
        self.assertEqual(settings['object_storage']['backend'], 's3')
        self.assertGreaterEqual(len(settings['auth']['jwt_secret']), 32)
        self.assertTrue(settings['bootstrap']['admin_password'])
        self.assertEqual(settings['rate_limit']['api_limits']['POST /api/v1/auth/login'], 5)
        self.assertEqual({key for key in job['env'] if key.startswith('APP_')}, {'APP_ENV'})
        upload = next(step for step in job['steps'] if step['name'] == '保存全栈诊断产物')
        self.assertIn('!${{ runner.temp }}/ryframe-full-stack/config/**', upload['with']['path'])
        self.assertIn('!${{ runner.temp }}/ryframe-full-stack/worker-config/**', upload['with']['path'])
        budget = next(step for step in job['steps'] if step['name'] == '从当前隔离配置导出登录预算')
        self.assertIn('--tool-config-file "$RUNNER_TEMP/ryframe-full-stack/login-budget-config.json"', budget['run'])
        self.assertNotIn('RYFRAME_E2E_LOGIN_', str(job))
        frontend = next(step for step in job['steps'] if step['name'] == '写入前端受管浏览器配置')
        self.assertIn('**budget', frontend['run'])
        self.assertIn('.local-tests/frontend-tools.json', frontend['run'])
        self.assertIn('tools.chmod(0o600)', frontend['run'])
        self.assertIn('!**/.local-tests/frontend-tools.json', upload['with']['path'])
        image = next(step for step in job['steps'] if step['name'] == '构建生产镜像')
        self.assertIn('--build-arg "SOURCE_COMMIT=$GITHUB_SHA"', image['run'])
        self.assertNotIn('RYFRAME_BUILD_COMMIT', image['run'])

    def test_upload_globs_exclude_actual_plaintext_browser_and_both_runtime_configs(self):
        import glob
        import yaml
        from workspace_directory import WorkspaceDirectory
        workflow = yaml.safe_load((ROOT / '.github/workflows/extended-ci.yml').read_text(encoding='utf-8'))
        job = workflow['jobs']['full-stack-e2e']
        upload = next(step for step in job['steps'] if step['name'] == '保存全栈诊断产物')
        patterns = upload['with']['path'].splitlines()
        with WorkspaceDirectory(TEMP, prefix='upload-glob-') as root:
            base = Path(root)
            sensitive = [base / 'frontend/.local-tests/frontend-tools.json',
                         base / 'fixtures/business/frontend/.local-tests/frontend-tools.json',
                         base / 'runner/ryframe-full-stack/config/app.test.toml',
                         base / 'runner/ryframe-full-stack/worker-config/app.test.toml']
            public = base / 'runner/ryframe-full-stack/api.log'
            for path in [*sensitive, public]:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture', encoding='utf-8')
            excluded = set()
            for pattern in patterns:
                if pattern.startswith('!'):
                    selected = pattern[1:].replace('${{ runner.temp }}', str(base / 'runner'))
                    if not Path(selected).is_absolute():
                        selected = str(base / selected)
                    excluded.update(Path(path).resolve() for path in glob.glob(selected, recursive=True, include_hidden=True))
            self.assertTrue(all(path.resolve() in excluded for path in sensitive))
            self.assertNotIn(public.resolve(), excluded)
        text = str(job)
        self.assertNotIn('RYFRAME_CI_', text)
        self.assertNotIn('VITE_APP_BUILD_COMMIT', text)
        self.assertIn('build --source-commit', text)
