"""身份准备工具只使用离线模型和本测试进程内的假后端。"""
import copy
import json
import subprocess
import unittest
from workspace_directory import WorkspaceDirectory
from pathlib import Path

from devex_identity_environment import quota_sql, validate_quota, validate_target_bindings


class IdentityPreparationTests(unittest.TestCase):
    def test_node_workflows(self):
        root = Path(__file__).resolve().parents[2]
        result = subprocess.run(["node", "--test", "scripts/tests/devex-identities.test.mjs",
                                 "scripts/tests/devex-quota-bridge.test.mjs"],
                                cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=120)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_quota_checks_current_non_deleted_users_and_explicit_limit(self):
        sql = quota_sql("fixture_control", "fixture-1")
        self.assertIn("u.del_flag='0'", sql)
        self.assertIn("t.tenant_id='fixture-1'", sql)
        with self.assertRaises(ValueError):
            quota_sql("fixture_control", "tenant'; DROP TABLE sys_user")
        group = {"tenant_id": "fixture-1", "slot": "tenant-01", "kind": "tenant", "count": 10}
        environment = {"quota": {"tenant_max_users": 200, "import_headroom_per_tenant": 100},
                       "tenants": [{"slot": "tenant-01", "target_key": "shared"}]}
        row = {"tenant_id": "fixture-1", "status": "enabled", "max_users": 200, "users": 1,
               "roles": 1, "max_roles": 10, "target_key": "shared", "placement_state": "active"}
        validate_quota(row, group, environment, "apply")
        for changed in ({"max_users": 201}, {"users": 91}, {"target_key": "other"},
                        {"placement_state": "maintenance"}, {"status": "disabled"}, {"roles": 10}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                validate_quota({**row, **changed}, group, environment, "apply")
        validate_quota({**row, "users": 100, "roles": 10}, group, environment, "verify")

    def test_targets_bind_to_actual_runtime_configuration_before_ownership(self):
        root = Path(__file__).resolve().parents[2]
        local = root / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        with WorkspaceDirectory(dir=local) as temporary:
            backend = Path(temporary)
            directory = backend / "config"
            directory.mkdir()
            (directory / "app.toml").write_text(
                '[[tenant_data.targets]]\nkey="shared"\nkind="mysql"\nmode="shared"\n'
                'host="127.0.0.1"\ndatabase="base_database"\nusername="fixture"\n'
                'password_env="APP_FIXTURE_PASSWORD"\n', encoding="utf-8")
            target = {"key": "shared", "mode": "shared", "connection": {
                "host": "127.0.0.1", "port": 3306, "database": "actual_database", "username": "fixture",
                "password_env": "APP_FIXTURE_PASSWORD", "tls_mode": "required"}}
            actual = {"key": target["key"], "mode": target["mode"], "kind": "mysql", **target["connection"]}
            environment = {"database": {"targets": [target]}}
            variables = {"APP_ENV": "test", "APP_FIXTURE_PASSWORD": "test-only-secret"}
            (directory / "app.test.toml").write_text(
                '[[tenant_data.targets]]\n' + '\n'.join(f'{key}={json.dumps(value)}' for key, value in actual.items()),
                encoding="utf-8")
            validate_target_bindings(backend, environment, variables)
            for changes in ({"database": "other_owned_database"}, {"mode": "dedicated"}, {"kind": "control"},
                            {"port": 3307}, {"host": "localhost"}, {"password_env": "OTHER_PASSWORD"},
                            {"tls_mode": "disabled"}, {"username": "other"}, {"key": "other"}):
                wrong = copy.deepcopy(environment)
                for key, value in changes.items():
                    if key in ("key", "mode", "kind"):
                        wrong["database"]["targets"][0][key] = value
                    else:
                        wrong["database"]["targets"][0]["connection"][key] = value
                # kind 属于真实配置；清单不允许凭空声明另一种产品目标。
                overrides = {**variables, "APP_TENANT_DATA_TARGETS": json.dumps([{**actual, **changes}])}
                with self.subTest(changes=changes), self.assertRaises(ValueError):
                    validate_target_bindings(backend, environment, overrides)
                if "kind" not in changes:
                    with self.subTest(manifest=changes), self.assertRaises(ValueError):
                        validate_target_bindings(backend, wrong, variables)
            for raw in ("{}", "null", "[1]", json.dumps([actual, actual])):
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    validate_target_bindings(backend, environment, {**variables, "APP_TENANT_DATA_TARGETS": raw})
            with self.assertRaises(ValueError):
                validate_target_bindings(backend, environment, {**variables, "APP_TENANT_DATA_TARGETS_FILE": "targets.json"})
            with self.assertRaises(ValueError):
                validate_target_bindings(backend, environment, {"APP_ENV": "test"})
            unbound = copy.deepcopy(environment)
            unbound["database"]["targets"][0]["connection"]["password_env"] = "UNBOUND_PASSWORD"
            with self.assertRaises(ValueError):
                validate_target_bindings(backend, unbound, {**variables, "UNBOUND_PASSWORD": "new-secret",
                    "APP_TENANT_DATA_TARGETS": json.dumps([{**actual, "password_env": "UNBOUND_PASSWORD"}])})
            self.assertNotIn("test-only-secret", json.dumps(environment))

    def test_target_mapping_follows_current_rust_defaults_and_override_spec(self):
        root = Path(__file__).resolve().parents[2]
        source = (root / "crates/ryframe-config/src/app_config/environment_overrides/spec.rs").read_text(encoding="utf-8")
        self.assertIn('EnvOverride::json_file("APP_TENANT_DATA_TARGETS", &["tenant_data", "targets"])', source)
        tls = (root / "crates/ryframe-config/src/db_config.rs").read_text(encoding="utf-8")
        self.assertRegex(tls, r"#\[default\]\s+Required")
        registry = (root / "crates/ryframe-tenant-db/src/registry/support.rs").read_text(encoding="utf-8")
        self.assertIn("port: target.port.unwrap_or(3306)", registry)
        self.assertIn("tls_mode: target.tls_mode.unwrap_or_default()", registry)


class IdentityControlTargetTests(unittest.TestCase):
    def setUp(self):
        backend = Path(__file__).resolve().parents[2]
        local = backend / '.local-tests/python-unit'
        local.mkdir(parents=True, exist_ok=True)
        self.temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(self.temporary.cleanup)
        self.backend = Path(self.temporary.name)
        (self.backend / 'config').mkdir()
        (self.backend / 'config/app.toml').write_text('[tenant_data]\ntargets=[]\n', encoding='utf-8')
        self.control = {'host': '127.0.0.1', 'port': 3306, 'database': 'fixture_control', 'username': 'fixture',
                        'password_env': 'APP_DATABASE_PASSWORD', 'tls_mode': 'required'}
        self.target = {'key': 'shared-control', 'mode': 'shared', 'connection': self.control}
        self.environment = {'database': {'control': self.control, 'targets': [self.target]}}
        self.variables = {'APP_ENV': 'test', 'APP_DATABASE_PASSWORD': 'fixture-only'}

    def check(self, targets, environment=None):
        validate_target_bindings(self.backend, environment or self.environment,
            {**self.variables, 'APP_TENANT_DATA_TARGETS': json.dumps(targets)})

    def test_implicit_and_explicit_control_preserve_exact_registered_primary_connection(self):
        self.check([])
        self.check([{'key': 'shared-control', 'kind': 'control', 'mode': 'shared'}])
        self.check([{'key': 'shared-control', 'kind': 'control', 'mode': 'shared', 'display_name': '共享控制库', 'host': None}])

    def test_control_does_not_accept_mysql_kind_connection_override_or_dedicated_mode(self):
        base = {'key': 'shared-control', 'kind': 'control', 'mode': 'shared'}
        for change in ({'kind': 'mysql'}, {'mode': 'dedicated'}, {'host': '127.0.0.1'},
                       {'database': 'another_owned_db'}, {'username': 'another'}, {'password_env': 'APP_OTHER'},
                       {'max_connections': 0}, {'tls_ca': ''}, {'tls_client_cert': ''}, {'tls_client_key': ''},
                       {'tls_mode': 'required'}, {'port': 3306}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.check([base | change])
        for change in ({'database': 'other'}, {'password_env': 'APP_OTHER'}, {'tls_mode': 'disabled'}, {'port': 3307}):
            with self.subTest(connection=change), self.assertRaises(ValueError):
                wrong = copy.deepcopy(self.environment)
                wrong['database']['targets'][0]['connection'] = self.control | change
                self.check([], wrong)
        wrong = copy.deepcopy(self.environment)
        wrong['database']['targets'][0]['mode'] = 'dedicated'
        with self.assertRaises(ValueError):
            self.check([], wrong)

    def test_unknown_target_and_duplicate_control_still_refused(self):
        wrong = copy.deepcopy(self.environment)
        wrong['database']['targets'][0]['key'] = 'unknown'
        with self.assertRaises(ValueError):
            self.check([], wrong)
        actual = {'key': 'shared-control', 'kind': 'control', 'mode': 'shared'}
        with self.assertRaises(ValueError):
            self.check([actual, actual])
        with self.assertRaises(ValueError):
            validate_target_bindings(self.backend, self.environment, {'APP_ENV': 'test'})

    def test_control_does_not_skip_other_target_connection_checks(self):
        connection = self.control | {'database': 'fixture_shared'}
        target = {'key': 'shared', 'mode': 'shared', 'connection': connection}
        configured = {'key': 'shared', 'kind': 'mysql', 'mode': 'shared', **connection}
        environment = copy.deepcopy(self.environment)
        environment['database']['targets'].append(target)
        for control in ([], [{'key': 'shared-control', 'kind': 'control', 'mode': 'shared'}]):
            self.check([*control, configured], environment)
            with self.assertRaisesRegex(ValueError, 'api_target_connection_mismatch'):
                self.check([*control, configured | {'database': 'unregistered'}], environment)

    def test_control_uses_merged_toml_and_current_json_override(self):
        validate_target_bindings(self.backend, self.environment, self.variables)
        (self.backend / 'config/app.test.toml').write_text(
            '[[tenant_data.targets]]\nkey="shared-control"\nkind="control"\nmode="shared"\n',
            encoding='utf-8')
        validate_target_bindings(self.backend, self.environment, self.variables)
        (self.backend / 'config/app.test.toml').write_text(
            '[[tenant_data.targets]]\nkey="shared-control"\nkind="control"\nmode="dedicated"\n',
            encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'api_control_target_configuration_mismatch'):
            validate_target_bindings(self.backend, self.environment, self.variables)
        self.check([])

    def test_current_rust_defaults_define_implicit_control_and_prohibit_connection_overrides(self):
        source = (Path(__file__).resolve().parents[2] / 'crates/ryframe-config/src/tenant_data_config.rs').read_text(encoding='utf-8')
        self.assertIn('targets.push(TenantDatabaseTargetConfig::shared_control());', source)
        self.assertIn('kind: TenantDatabaseTargetKind::Control', source)
        self.assertIn('mode: TenantDatabaseTargetMode::Shared', source)
        for name in ('host', 'port', 'database', 'username', 'password_env', 'max_connections', 'tls_mode', 'tls_ca',
                     'tls_client_cert', 'tls_client_key'):
            self.assertIn(f'self.{name}.is_some()', source)



if __name__ == "__main__":
    unittest.main()
