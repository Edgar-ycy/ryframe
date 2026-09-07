import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_source_binding as binding

ROOT = Path(__file__).resolve().parents[2]


class SourceBindingTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name)
        self.config = self.backend / "config"
        self.config.mkdir()
        self.client = self.backend / ".local-tests/client.cnf"
        self.client.parent.mkdir()
        self.client.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=fixture-user\n"
                               "password=private-db-value\nssl-mode=REQUIRED\n", encoding="utf-8")
        self.variables = {"APP_ENV": "test", "APP_SCOPE_ID": "source-fixture",
                          "APP_SOURCE_PASSWORD": "private-db-value", "EXTERNAL_ACCESS": "private-access-value",
                          "EXTERNAL_SECRET": "private-secret-value"}
        self.targets = [{"key": key, "kind": "mysql", "mode": mode, "host": "127.0.0.1", "port": 3306,
                         "database": "source_" + key.replace("-", "_"), "username": "fixture-user",
                         "password_env": "APP_SOURCE_PASSWORD", "tls_mode": "required"}
                        for key, mode in (("shared", "shared"), ("dedicated-a", "dedicated"), ("dedicated-b", "dedicated"))]
        self.plan = {"source": {"scope_id": "source-fixture", "s3": {"endpoint": "http://127.0.0.1:29000",
                     "region": "us-east-1", "access_key_env": "EXTERNAL_ACCESS", "secret_key_env": "EXTERNAL_SECRET"},
                     "databases": [{"key": "shared-control", "kind": "combined", "mode": "shared", "database": "source_control"},
                                   *[{"key": value["key"], "kind": "tenant", "mode": value["mode"], "database": value["database"]}
                                     for value in self.targets]]}}
        self.refresh_defaults_hash()
        self.write_config()

    def refresh_defaults_hash(self):
        for item in self.plan["source"]["databases"]:
            item.update(defaults_file=str(self.client.resolve()), defaults_sha256=hashlib.sha256(self.client.read_bytes()).hexdigest(),
                        server_uuid="source-server-uuid")

    def write_config(self):
        base = ('[database.primary]\nhost="127.0.0.1"\nport=3306\ndatabase="source_control"\n'
                'username="fixture-user"\npassword="private-db-value"\ntls_mode="required"\n'
                '[object_storage]\nbackend="rustfs"\nendpoint="http://127.0.0.1:29000"\n'
                'region="us-east-1"\naccess_key="private-access-value"\nsecret_key="private-secret-value"\nuse_ssl=false\n')
        for target in self.targets:
            base += "[[tenant_data.targets]]\n" + "".join(f"{key}={json.dumps(value)}\n" for key, value in target.items())
        (self.config / "app.toml").write_text(base, encoding="utf-8")

    def verify(self):
        return binding.source_binding(self.backend, self.plan, self.variables)

    def test_complete_four_target_binding_omits_all_credentials_and_is_order_independent(self):
        before = self.verify()
        self.plan["source"]["databases"].reverse()
        self.assertEqual(self.verify(), before)
        self.assertEqual(len(before["databases"]), 4)
        serialized = json.dumps(before)
        for value in ("private-db-value", "private-access-value", "private-secret-value", "fixture-user", "APP_SOURCE_PASSWORD"):
            self.assertNotIn(value, serialized)
        self.assertEqual(before["s3"]["endpoint"], "http://127.0.0.1:29000")

    def test_environment_toml_and_direct_app_overrides_use_effective_values(self):
        (self.config / "app.test.toml").write_text('[database.primary]\ndatabase="wrong"\n'
                                                  '[object_storage]\nendpoint="http://127.0.0.1:29001"\n', encoding="utf-8")
        with self.assertRaises(binding.BindingError):
            self.verify()
        self.variables.update(APP_DATABASE_NAME="source_control", APP_OBJECT_STORAGE_ENDPOINT="http://127.0.0.1:29000",
                              APP_DATABASE_PASSWORD="private-db-value", APP_DATABASE_PORT="+3306",
                              APP_OBJECT_STORAGE_USE_SSL="false", APP_TENANT_DATA_TARGETS=json.dumps(self.targets))
        self.assertEqual(len(self.verify()["databases"]), 4)

    def test_primary_mismatch_rejects_every_physical_and_authentication_component(self):
        for suffix, value in {"HOST": "::1", "PORT": "3307", "NAME": "wrong", "USERNAME": "other-user",
                              "PASSWORD": "wrong-secret", "TLS_MODE": "disabled"}.items():
            with self.subTest(field=suffix):
                self.variables["APP_DATABASE_" + suffix] = value
                with self.assertRaisesRegex(binding.BindingError, "来源数据库"):
                    self.verify()
                del self.variables["APP_DATABASE_" + suffix]

    def test_tenant_mismatch_rejects_same_scope_other_owned_database(self):
        changes = {"host": "::1", "port": 3307, "database": "source_other_owned", "username": "other-user",
                   "tls_mode": "disabled", "mode": "dedicated", "kind": "control"}
        for field, value in changes.items():
            with self.subTest(field=field):
                targets = copy.deepcopy(self.targets)
                targets[0][field] = value
                self.variables["APP_TENANT_DATA_TARGETS"] = json.dumps(targets)
                with self.assertRaises(binding.BindingError):
                    self.verify()

    def test_tenant_password_must_be_bound_and_equal_to_external_tool(self):
        self.variables["APP_SOURCE_PASSWORD"] = "changed-secret"
        with self.assertRaisesRegex(binding.BindingError, "认证"):
            self.verify()
        self.variables["APP_SOURCE_PASSWORD"] = "private-db-value"
        for variable in ("REFERENCE_PASSWORD", "APP_SOURCE_PASSWORD_FILE"):
            self.variables[variable] = "private-db-value"
            targets = copy.deepcopy(self.targets)
            targets[0]["password_env"] = variable
            self.variables["APP_TENANT_DATA_TARGETS"] = json.dumps(targets)
            with self.assertRaisesRegex(binding.BindingError, "APP 环境变量"):
                self.verify()

    def test_target_set_missing_extra_duplicate_or_control_alias_fails_closed(self):
        extra = {**self.targets[0], "key": "extra", "database": "source_extra"}
        for targets in (self.targets[1:], [*self.targets, extra], [*self.targets, self.targets[0]],
                        [*self.targets, {"key": "shared-control", "mode": "dedicated", "kind": "mysql"}]):
            with self.subTest(count=len(targets)):
                self.variables["APP_TENANT_DATA_TARGETS"] = json.dumps(targets)
                with self.assertRaises(binding.BindingError):
                    self.verify()

    def test_explicit_control_target_and_empty_read_replicas_are_supported(self):
        targets = [*self.targets, {"key": "shared-control", "kind": "control", "mode": "shared"}]
        self.variables.update(APP_TENANT_DATA_TARGETS=json.dumps(targets), APP_DATABASE_REPLICAS="[]", APP_DATABASE_SOURCES="[]")
        self.assertEqual(len(self.verify()["databases"]), 4)

    def test_explicit_disabled_tls_and_https_object_scheme_are_compared_exactly(self):
        self.client.write_text(self.client.read_text().replace("REQUIRED", "DISABLED"))
        self.refresh_defaults_hash()
        targets = [{**target, "tls_mode": "disabled"} for target in self.targets]
        self.variables.update(APP_DATABASE_TLS_MODE="disabled", APP_TENANT_DATA_TARGETS=json.dumps(targets),
                              APP_OBJECT_STORAGE_USE_SSL="true", APP_OBJECT_STORAGE_ENDPOINT="https://127.0.0.1:29000/")
        self.plan["source"]["s3"]["endpoint"] = "https://127.0.0.1:29000"
        self.assertTrue(all(item["tls_mode"] == "disabled" for item in self.verify()["databases"]))

    def test_no_implicit_tenant_port_or_tls_defaults_are_guessed(self):
        for field in ("port", "tls_mode", "password_env"):
            with self.subTest(field=field):
                targets = copy.deepcopy(self.targets)
                del targets[0][field]
                self.variables["APP_TENANT_DATA_TARGETS"] = json.dumps(targets)
                with self.assertRaises(binding.BindingError):
                    self.verify()

    def test_external_files_and_unlisted_database_sources_are_rejected(self):
        for key in ("APP_DATABASE_PASSWORD_FILE", "APP_DATABASE_REPLICAS_FILE", "APP_DATABASE_SOURCES_FILE",
                    "APP_TENANT_DATA_TARGETS_FILE", "APP_OBJECT_STORAGE_ACCESS_KEY_FILE", "APP_OBJECT_STORAGE_SECRET_KEY_FILE"):
            with self.subTest(key=key):
                self.variables[key] = "never-read-secret-file"
                with self.assertRaisesRegex(binding.BindingError, "文件"):
                    self.verify()
                del self.variables[key]
        for key in ("APP_DATABASE_REPLICAS", "APP_DATABASE_SOURCES"):
            self.variables[key] = '[{"host":"127.0.0.1"}]'
            with self.assertRaisesRegex(binding.BindingError, "副本或命名数据源"):
                self.verify()
            del self.variables[key]

    def test_tls_file_contents_cannot_be_proven_by_path_only(self):
        for field in binding.TLS_FILES:
            self.variables["APP_DATABASE_" + field.upper()] = "unbound-certificate.pem"
            with self.assertRaisesRegex(binding.BindingError, "TLS 文件"):
                self.verify()
            del self.variables["APP_DATABASE_" + field.upper()]
            targets = copy.deepcopy(self.targets)
            targets[0][field] = "unbound-certificate.pem"
            self.variables["APP_TENANT_DATA_TARGETS"] = json.dumps(targets)
            with self.assertRaisesRegex(binding.BindingError, "TLS 文件"):
                self.verify()
            del self.variables["APP_TENANT_DATA_TARGETS"]

    def test_object_endpoint_region_tls_backend_and_each_secret_must_match(self):
        changes = {"ENDPOINT": "http://127.0.0.1:29001", "REGION": "us-west-2", "USE_SSL": "true",
                   "BACKEND": "local", "ACCESS_KEY": "changed-access", "SECRET_KEY": "changed-secret"}
        for suffix, value in changes.items():
            with self.subTest(field=suffix):
                self.variables["APP_OBJECT_STORAGE_" + suffix] = value
                with self.assertRaises(binding.BindingError):
                    self.verify()
                del self.variables["APP_OBJECT_STORAGE_" + suffix]
        self.variables["EXTERNAL_SECRET"] = "external-secret-changed"
        with self.assertRaisesRegex(binding.BindingError, "对象认证"):
            self.verify()

    def test_portable_quoted_password_is_normalized_before_binding(self):
        self.client.write_text(self.client.read_text().replace("private-db-value", '"private-db-value"'))
        self.refresh_defaults_hash()
        self.assertEqual(len(self.verify()["databases"]), 4)

    def test_incomplete_ambiguous_defaults_or_changed_file_are_rejected(self):
        original = self.client.read_text()
        for content in (original + "ssl-ca=external.pem\n", original.replace("ssl-mode=REQUIRED\n", ""),
                        original.replace("private-db-value", '"private db value"'),
                        original.replace("private-db-value", '"private-db-value\\"'),
                        original.replace("private-db-value", "private-db-value#comment")):
            with self.subTest():
                self.client.write_text(content)
                self.refresh_defaults_hash()
                with self.assertRaises(binding.BindingError):
                    self.verify()
        self.client.write_text(original)
        with self.assertRaisesRegex(binding.BindingError, "摘要已变化"):
            self.verify()

    def test_duplicate_physical_database_is_rejected_even_if_plan_matches(self):
        targets = copy.deepcopy(self.targets)
        targets[1]["database"] = targets[0]["database"]
        self.plan["source"]["databases"][2]["database"] = targets[0]["database"]
        self.variables["APP_TENANT_DATA_TARGETS"] = json.dumps(targets)
        with self.assertRaisesRegex(binding.BindingError, "同一物理数据库"):
            self.verify()

    def test_parse_failures_and_mismatches_never_echo_secret_values(self):
        self.variables["APP_TENANT_DATA_TARGETS"] = 'private-secret-malformed-json'
        with self.assertRaises(binding.BindingError) as caught:
            self.verify()
        self.assertNotIn("private-secret", str(caught.exception))
        del self.variables["APP_TENANT_DATA_TARGETS"]
        (self.config / "app.toml").write_text('password = private-secret-invalid-toml')
        with self.assertRaises(binding.BindingError) as caught:
            self.verify()
        self.assertNotIn("private-secret", str(caught.exception))

    def test_scope_or_environment_mismatch_fails_before_reading_configuration(self):
        for key, value in (("APP_SCOPE_ID", "another-scope"), ("APP_ENV", "prod")):
            self.variables[key] = value
            with self.assertRaisesRegex(binding.BindingError, "scope"):
                self.verify()

    def test_override_mapping_stays_bound_to_current_product_declarations(self):
        source = (ROOT / "crates/ryframe-config/src/app_config/environment_overrides/spec.rs").read_text()
        compact = "".join(source.split())
        for field, suffix in binding.DATABASE_FIELDS.items():
            self.assertIn(f'"APP_DATABASE_{suffix}",&["database","primary","{field}"]', compact)
        for field in binding.STORAGE_FIELDS:
            self.assertIn(f'"APP_OBJECT_STORAGE_{field.upper()}",&["object_storage","{field}"]', compact)


if __name__ == "__main__":
    unittest.main()
