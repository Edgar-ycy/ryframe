"""CI 协调器协议测试；MySQL 校验和进程场景由 ci::databases 的 Rust 测试覆盖。"""
import sys
import tomllib
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / '.local-tests/python-unit'
TEMP.mkdir(parents=True, exist_ok=True)


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
