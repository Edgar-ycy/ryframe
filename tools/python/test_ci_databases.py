import copy
import json
import os
import shutil
import sys
import tomllib
import unittest
import uuid
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_full_stack_databases as databases
from ci_full_stack_databases import prepare_databases

ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / '.local-tests/python-unit'


class CiDatabaseTests(unittest.TestCase):
    def setUp(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        self.root = TEMP / f'ci-databases-{uuid.uuid4().hex}'
        self.root.mkdir()
        self.addCleanup(shutil.rmtree, self.root)
        self.source = self.root / 'input'
        self.source.mkdir()
        self.output = self.root / 'output'
        self.output.mkdir()
        (self.source / 'app.toml').write_text('[tenant_data]\n', encoding='utf-8')
        self.settings = {'scope_id': 'ci-42-3', 'snowflake_worker_id': 3, 'database': {'primary': {
            'host': '127.0.0.1', 'port': 3306, 'database': 'ryframe_e2e_42_3',
            'username': 'root', 'password': 'unit-test-only', 'tls_mode': 'disabled'}}}
        self.write_configuration()
        self.environment = {'GITHUB_ACTIONS': 'true', 'GITHUB_RUN_ID': '42',
                            'GITHUB_RUN_ATTEMPT': '3', 'APP_ENV': 'test'}
        self.container = 'a' * 64
        self.loader = patch.object(databases, 'load_app_table', side_effect=lambda *a, **k: copy.deepcopy(self.settings))
        self.load = self.loader.start()
        self.addCleanup(self.loader.stop)
        environment = patch.dict(os.environ, self.environment, clear=True)
        environment.start()
        self.addCleanup(environment.stop)

    def write_configuration(self):
        text = f'scope_id = {json.dumps(self.settings["scope_id"])}\nsnowflake_worker_id = {self.settings["snowflake_worker_id"]}\n[database.primary]\n'
        text += '\n'.join(f'{key} = {json.dumps(value)}' for key, value in self.settings['database']['primary'].items())
        (self.source / 'app.test.toml').write_text(text, encoding='utf-8')

    def prepare(self, run):
        return prepare_databases(run, os.environ.__getitem__, ROOT, self.output, config_dir=self.source, container_id=self.container)

    def test_exact_new_targets_and_credentials_use_stdin_and_managed_toml(self):
        run = Mock()
        result = self.prepare(run)
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[:4], ['docker', 'exec', '-i', 'a' * 64])
        self.assertIn('--ssl-mode=DISABLED', arguments)
        self.assertNotIn('unit-test-only', ' '.join(arguments))
        self.assertFalse(any(key.startswith('APP_') or key == 'MYSQL_PWD' for key in run.call_args.kwargs['env']))
        sql = run.call_args.kwargs['input']
        self.assertTrue(sql.startswith('[client]\npassword="unit-test-only"\n'))
        for suffix in ('', '_shared', '_dedicated_a', '_dedicated_b'):
            self.assertIn(f'CREATE DATABASE `ryframe_e2e_42_3{suffix}`', sql)
        self.assertEqual(sql.count('CREATE DATABASE'), 4)
        self.assertNotIn('IF NOT EXISTS', sql)
        settings = tomllib.loads((result / 'app.test.toml').read_text(encoding='utf-8'))
        self.assertEqual(settings['database']['primary']['password'], 'unit-test-only')
        worker = tomllib.loads((self.output / 'worker-config/app.test.toml').read_text(encoding='utf-8'))
        self.assertEqual(settings['snowflake_worker_id'], 3)
        self.assertEqual(worker['snowflake_worker_id'], 2)
        self.assertEqual({k: v for k, v in settings.items() if k != 'snowflake_worker_id'},
                         {k: v for k, v in worker.items() if k != 'snowflake_worker_id'})
        self.assertEqual((self.output / 'worker-config/app.toml').read_bytes(), (result / 'app.toml').read_bytes())
        if os.name != 'nt':
            for directory in (result, self.output / 'worker-config'):
                self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
                self.assertEqual((directory / 'app.test.toml').stat().st_mode & 0o777, 0o600)
        targets = settings['tenant_data']['targets']
        self.assertEqual([target['mode'] for target in targets], ['shared', 'dedicated', 'dedicated'])
        self.assertTrue(all(target['password'] == 'unit-test-only' for target in targets))
        self.assertNotIn('APP_CONFIG_DIR', os.environ)
        self.load.assert_called_once_with(ROOT, {'APP_ENV': 'test'}, config_dir=self.source)
        receipt_text = (self.output / 'database-targets.json').read_text(encoding='utf-8')
        self.assertEqual(json.loads(receipt_text)['state'], 'created')
        self.assertNotIn('unit-test-only', receipt_text)

    def test_unsafe_ci_metadata_fails_before_creation(self):
        for key, value in {'GITHUB_ACTIONS': 'false', 'APP_ENV': 'prod', 'GITHUB_RUN_ID': '42;DROP',
                           'GITHUB_RUN_ATTEMPT': '0'}.items():
            with self.subTest(key=key), patch.dict(os.environ, {key: value}):
                run = Mock()
                with self.assertRaises(ValueError):
                    self.prepare(run)
                run.assert_not_called()
                self.assertEqual(list(self.output.iterdir()), [])

    def test_invalid_file_fields_fail_without_files_or_database_writes(self):
        for field, value in (('host', 'remote.example'), ('database', 'business'), ('port', 70000),
                             ('port', '3306x'), ('port', True), ('username', ''), ('password', ''),
                             ('password', 'secret\x00'), ('password_env', 'PRIVATE'),
                             ('tls_mode', 'preferred'), ('tls_mode', 'verify_identity')):
            with self.subTest(field=field):
                settings = copy.deepcopy(self.settings)
                settings['database']['primary'][field] = value
                run = Mock()
                with patch.object(databases, 'load_app_table', return_value=settings), self.assertRaises(ValueError) as raised:
                    self.prepare(run)
                self.assertNotIn('PRIVATE', str(raised.exception))
                run.assert_not_called()
                self.assertEqual(list(self.output.iterdir()), [])

    def test_legacy_environment_cannot_override_file_credentials(self):
        run = Mock()
        with patch.dict(os.environ, {'APP_DATABASE_PASSWORD': 'host-secret', 'MYSQL_PWD': 'host-secret',
                                    'APP_DATABASE_NAME': 'business', 'APP_CONFIG_DIR': 'elsewhere'}):
            self.prepare(run)
        self.assertNotIn('host-secret', str(run.call_args))

    def test_partial_creation_retains_unknown_receipt_and_redacts_failure(self):
        run = Mock(side_effect=RuntimeError('unit-test-only'))
        with self.assertRaisesRegex(RuntimeError, '创建结果未知') as raised:
            self.prepare(run)
        self.assertNotIn('unit-test-only', str(raised.exception))
        receipt = (self.output / 'database-targets.json').read_text(encoding='utf-8')
        self.assertEqual(json.loads(receipt)['state'], 'creation-unknown')
        self.assertNotIn('unit-test-only', receipt)
        self.assertFalse((self.output / 'config/app.test.toml').exists())
        with self.assertRaisesRegex(ValueError, '拒绝自动重放'):
            self.prepare(Mock())

    def test_existing_plan_or_config_prevents_replay(self):
        for existing in ('database-targets.json', 'config', 'worker-config'):
            with self.subTest(existing=existing):
                path = self.output / existing
                path.mkdir() if existing != 'database-targets.json' else path.write_text('{}', encoding='utf-8')
                run = Mock()
                with self.assertRaisesRegex(ValueError, '拒绝自动重放'):
                    self.prepare(run)
                run.assert_not_called()
                path.rmdir() if path.is_dir() else path.unlink()

    def test_plan_is_read_only_and_binds_configuration_snapshot(self):
        planned = databases.plan_databases(os.environ.__getitem__, ROOT, self.output, config_dir=self.source, container_id=self.container)
        self.assertEqual(list(self.output.iterdir()), [])
        self.assertNotIn('unit-test-only', repr(planned))
        (self.source / 'app.test.toml').write_text('scope_id="unrelated"', encoding='utf-8')
        prepare_databases(Mock(), os.environ.__getitem__, ROOT, self.output, plan=planned)
        result = tomllib.loads((self.output / 'config/app.test.toml').read_text(encoding='utf-8'))
        self.assertEqual(result['scope_id'], 'ci-42-3')

    def test_duplicate_or_invalid_instance_id_prevents_all_writes(self):
        for value in (2, True, -1, 1024, '3', None):
            with self.subTest(value=value):
                settings = {**self.settings, 'snowflake_worker_id': value}
                run = Mock()
                with patch.object(databases, 'load_app_table', return_value=settings), self.assertRaisesRegex(ValueError, 'snowflake_worker_id'):
                    self.prepare(run)
                run.assert_not_called()
                self.assertEqual(list(self.output.iterdir()), [])

    def test_missing_configuration_binding_never_uses_backend_defaults(self):
        run = Mock()
        with self.assertRaisesRegex(ValueError, '显式绑定'):
            prepare_databases(run, os.environ.__getitem__, ROOT, self.output)
        run.assert_not_called()
        self.load.assert_not_called()
        self.assertEqual(list(self.output.iterdir()), [])

    def test_worker_override_does_not_change_nested_fields(self):
        text = 'scope_id="test-scope"\n[app]\nsnowflake_worker_id = 99\n'
        rewritten = databases._instance_text(text, 2)
        parsed = tomllib.loads(rewritten)
        self.assertEqual(parsed['snowflake_worker_id'], 2)
        self.assertEqual(parsed['app']['snowflake_worker_id'], 99)
        for key in ('snowflake_worker_id', '"snowflake_worker_id"', "'snowflake_worker_id'"):
            with self.subTest(key=key):
                self.assertEqual(tomllib.loads(databases._instance_text(f'  {key} = 3\n[app]\nport=8080\n', 2))['snowflake_worker_id'], 2)

    def test_required_tls_is_explicit_on_creation_and_targets(self):
        self.settings['database']['primary']['tls_mode'] = 'required'
        self.write_configuration()
        run = Mock()
        result = self.prepare(run)
        self.assertIn('--ssl-mode=REQUIRED', run.call_args.args[0])
        targets = tomllib.loads((result / 'app.test.toml').read_text(encoding='utf-8'))['tenant_data']['targets']
        self.assertTrue(all(target['tls_mode'] == 'required' for target in targets))

    def test_selected_toml_is_read_by_the_existing_rust_entry(self):
        self.loader.stop()
        with patch.dict(os.environ, {'APP_DATABASE_NAME': 'foreign', 'APP_DATABASE_PASSWORD': 'foreign'}):
            planned = databases.plan_databases(os.environ.__getitem__, ROOT, self.output, config_dir=self.source, container_id=self.container)
        self.assertEqual(planned.database, 'ryframe_e2e_42_3')
        self.assertEqual(planned.password, 'unit-test-only')
        self.assertEqual(list(self.output.iterdir()), [])
        (self.source / 'app.test.toml').unlink()
        with self.assertRaises((ValueError, FileNotFoundError)) as raised:
            databases.plan_databases(os.environ.__getitem__, ROOT, self.output, config_dir=self.source, container_id=self.container)
        self.assertNotIn('unit-test-only', str(raised.exception))

    def test_unicode_and_quoted_credentials_roundtrip_in_generated_toml(self):
        password = '中文🔐"\\line\nnext'
        self.settings['database']['primary']['password'] = password
        run = Mock()
        result = self.prepare(run)
        targets = tomllib.loads((result / 'app.test.toml').read_text(encoding='utf-8'))['tenant_data']['targets']
        self.assertTrue(all(target['password'] == password for target in targets))
        self.assertNotIn(password, ' '.join(run.call_args.args[0]))

    def test_verified_tls_parameters_and_job_identity_are_preserved(self):
        self.settings['database']['primary'].update(tls_mode='verify_identity', tls_ca='/tmp/ci-ca.pem')
        run = Mock()
        self.prepare(run)
        self.assertIn('--ssl-mode=VERIFY_IDENTITY', run.call_args.args[0])
        self.assertIn('--ssl-ca=/tmp/ci-ca.pem', run.call_args.args[0])

    def test_predeclared_targets_or_wrong_table_types_fail_before_writes(self):
        for update in ({'tenant_data': {'targets': []}}, {'tenant_data': 'private-value'},
                       {'database': {'primary': 'private-value'}}):
            run = Mock()
            settings = {**self.settings, **update}
            with patch.object(databases, 'load_app_table', return_value=settings), self.assertRaises(ValueError) as raised:
                self.prepare(run)
            self.assertNotIn('private-value', str(raised.exception))
            run.assert_not_called()
            self.assertEqual(list(self.output.iterdir()), [])

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


class ManagedFullStackControlTests(unittest.TestCase):
    def test_control_uses_file_and_rejects_missing_invalid_or_ambiguous_values(self):
        from full_stack_migration_mysql import load_control
        from workspace_directory import WorkspaceDirectory
        with WorkspaceDirectory(TEMP, prefix="control-") as root:
            backend = Path(root)
            path = backend / '.local-tests/full-stack-control.json'
            path.parent.mkdir()
            valid = {'format_version': 1, 'runtime_dir': str(backend / 'runtime'),
                     **{f'{role}_container_id': letter * 64 for role, letter in [('mysql', 'a'), ('redis', 'b'), ('s3', 'c')]}}
            path.write_text(json.dumps(valid), encoding='utf-8')
            with patch.dict(os.environ, {'RYFRAME_CI_MYSQL_CONTAINER_ID': 'd' * 64}):
                self.assertEqual(load_control(backend), valid)
            for changed in ({**valid, 'format_version': True}, {**valid, 'runtime_dir': 'relative'},
                            {**valid, 'mysql_container_id': 'bad;command'}, {**valid, 'extra': 'value'}):
                path.write_text(json.dumps(changed), encoding='utf-8')
                with self.subTest(changed=changed), self.assertRaises(ValueError):
                    load_control(backend)
            path.unlink()
            with self.assertRaises(ValueError):
                load_control(backend)

    def test_native_client_path_is_bound_by_managed_control_file(self):
        from full_stack_migration_mysql import load_control
        from workspace_directory import WorkspaceDirectory
        with WorkspaceDirectory(TEMP, prefix="native-control-") as root:
            backend = Path(root)
            executable = backend / "mysql.exe"
            executable.write_bytes(b"fixture")
            path = backend / ".local-tests/full-stack-control.json"
            path.parent.mkdir()
            value = {"format_version": 1, "runtime_dir": str(backend / "runtime"), "mysql_client": str(executable)}
            path.write_text(json.dumps(value), encoding="utf-8")
            with patch.dict(os.environ, {"RYFRAME_E2E_MYSQL_CLIENT": "ignored"}):
                self.assertEqual(load_control(backend)["mysql_client"], str(executable))
            value["mysql_client"] = "relative/mysql"
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "绝对路径"):
                load_control(backend)

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
