import copy
import hashlib
import json
from collections.abc import Mapping
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory
import restore_source_binding as binding

ROOT = Path(__file__).resolve().parents[2]


class SourceBindingTests(unittest.TestCase):
    def setUp(self):
        (ROOT / ".local-tests/python-unit").mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name)
        self.config = self.backend / "config"
        self.config.mkdir()
        self.client = self.backend / ".local-tests/client.cnf"
        self.client.parent.mkdir()
        self.client.write_text(
            "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture-user\n"
            "password=private-db-value\nssl-mode=REQUIRED\n", encoding="utf-8",
        )
        self.variables = {"APP_ENV": "test"}
        targets = [
            {"key": key, "kind": "mysql", "mode": mode, "host": "127.0.0.1", "port": 3306,
             "database": "source_" + key.replace("-", "_"), "username": "fixture-user",
             "password": "private-db-value", "tls_mode": "required"}
            for key, mode in (
                ("shared", "shared"), ("dedicated-a", "dedicated"), ("dedicated-b", "dedicated")
            )
        ]
        self.table = {
            "scope_id": "source-fixture",
            "database": {"primary": {
                "host": "127.0.0.1", "port": 3306, "database": "source_control",
                "username": "fixture-user", "password": "private-db-value", "tls_mode": "required",
            }},
            "object_storage": {
                "backend": "rustfs", "endpoint": "http://127.0.0.1:29000", "region": "us-east-1",
                "access_key": "private-access-value", "secret_key": "private-secret-value",
                "use_ssl": False,
            },
            "tenant_data": {"targets": targets},
        }
        self.plan = {"source": {
            "scope_id": "source-fixture",
            "s3": {"endpoint": "http://127.0.0.1:29000", "region": "us-east-1",
                   "access_key": "private-access-value", "secret_key": "private-secret-value"},
            "databases": [
                {"key": "shared-control", "kind": "combined", "mode": "shared",
                 "database": "source_control"},
                *[{"key": value["key"], "kind": "tenant", "mode": value["mode"],
                   "database": value["database"]} for value in targets],
            ],
        }}
        self.refresh_defaults_hash()
        loader = mock.patch.object(
            binding, "load_app_table", side_effect=lambda *args, **kwargs: copy.deepcopy(self.table)
        )
        self.loader = loader.start()
        self.addCleanup(loader.stop)

    def refresh_defaults_hash(self):
        for item in self.plan["source"]["databases"]:
            item.update(
                defaults_file=str(self.client.resolve()),
                defaults_sha256=hashlib.sha256(self.client.read_bytes()).hexdigest(),
                server_uuid="source-server-uuid",
            )

    def verify(self, *, config_dir=None):
        return binding.source_binding(
            self.backend, self.plan, self.variables, config_dir=config_dir
        )

    def test_execution_configuration_and_coordinator_credentials_have_explicit_roots(self):
        execution = self.backend / "execution"
        execution.mkdir()
        expected = self.verify(config_dir=self.config)
        with self.assertRaisesRegex(binding.BindingError, "凭据文件边界"):
            binding.source_binding(execution, self.plan, self.variables, config_dir=self.config)
        self.assertEqual(
            binding.source_binding(
                execution, self.plan, self.variables,
                config_dir=self.config, evidence_root=self.backend,
            ), expected,
        )
        self.loader.assert_called_with(execution, {"APP_ENV": "test"}, config_dir=self.config)
        foreign = self.backend / "foreign.cnf"
        foreign.write_bytes(self.client.read_bytes())
        self.plan["source"]["databases"][0]["defaults_file"] = str(foreign)
        with self.assertRaisesRegex(binding.BindingError, "凭据文件边界"):
            binding.source_binding(
                execution, self.plan, self.variables,
                config_dir=self.config, evidence_root=self.backend,
            )

    def test_complete_four_target_binding_omits_all_credentials_and_is_order_independent(self):
        before = self.verify()
        self.plan["source"]["databases"].reverse()
        self.assertEqual(self.verify(), before)
        self.assertEqual(len(before["databases"]), 4)
        self.assertTrue(all(item["server_uuid"] == "source-server-uuid" for item in before["databases"]))
        serialized = json.dumps(before)
        for value in (
            "private-db-value", "private-access-value", "private-secret-value",
            "fixture-user", "APP_SOURCE_PASSWORD", str(self.client),
        ):
            self.assertNotIn(value, serialized)
        self.assertEqual(before["s3"]["endpoint"], "http://127.0.0.1:29000")
        public = {key: value for key, value in before.items() if key != "sha256"}
        expected_hash = hashlib.sha256(
            json.dumps(public, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        self.assertEqual(before["sha256"], expected_hash)

    def test_selected_file_table_is_authoritative_over_legacy_overrides(self):
        self.table["database"]["primary"]["database"] = "wrong"
        self.table["object_storage"]["endpoint"] = "http://127.0.0.1:29001"
        with self.assertRaises(binding.BindingError):
            self.verify()
        self.variables.update(
            APP_DATABASE_NAME="source_control",
            APP_OBJECT_STORAGE_ENDPOINT="http://127.0.0.1:29000",
            APP_DATABASE_PASSWORD="private-db-value", APP_DATABASE_PORT="+3306",
            APP_OBJECT_STORAGE_USE_SSL="false",
            APP_TENANT_DATA_TARGETS=json.dumps(self.table["tenant_data"]["targets"]),
        )
        with self.assertRaises(binding.BindingError):
            self.verify()
        self.table["database"]["primary"]["database"] = "source_control"
        with self.assertRaisesRegex(binding.BindingError, "对象端点"):
            self.verify()
        self.table["object_storage"]["endpoint"] = "http://127.0.0.1:29000"
        self.assertEqual(len(self.verify()["databases"]), 4)

    def test_primary_mismatch_rejects_every_physical_and_authentication_component(self):
        primary = self.table["database"]["primary"]
        changes = {"host": "::1", "port": 3307, "database": "wrong", "username": "other-user",
                   "password": "wrong-secret", "tls_mode": "disabled"}
        for field, value in changes.items():
            with self.subTest(field=field):
                self.table["database"]["primary"] = {**primary, field: value}
                with self.assertRaisesRegex(binding.BindingError, "来源数据库"):
                    self.verify()
        for value in (True, 0, 65536, "3306"):
            with self.subTest(port=value):
                self.table["database"]["primary"] = {**primary, "port": value}
                with self.assertRaisesRegex(binding.BindingError, "端口"):
                    self.verify()

    def test_tenant_mismatch_rejects_same_scope_other_owned_database(self):
        original = copy.deepcopy(self.table["tenant_data"]["targets"])
        changes = {"host": "::1", "port": 3307, "database": "source_other_owned",
                   "username": "other-user", "tls_mode": "disabled",
                   "mode": "dedicated", "kind": "control"}
        for field, value in changes.items():
            with self.subTest(field=field):
                targets = copy.deepcopy(original)
                targets[0][field] = value
                self.table["tenant_data"]["targets"] = targets
                with self.assertRaises(binding.BindingError):
                    self.verify()

    def test_tenant_password_must_be_explicit_and_equal_to_external_tool(self):
        target = self.table["tenant_data"]["targets"][0]
        target["password"] = "changed-secret"
        with self.assertRaisesRegex(binding.BindingError, "认证"):
            self.verify()
        del target["password"]
        with self.assertRaises(binding.BindingError):
            self.verify()
        for variable in ("APP_SOURCE_PASSWORD", "REFERENCE_PASSWORD", "APP_SOURCE_PASSWORD_FILE"):
            for direct in (False, True):
                with self.subTest(variable=variable, direct=direct):
                    target["password_env"] = variable
                    self.variables[variable] = "private-db-value"
                    if direct:
                        target["password"] = "private-db-value"
                    else:
                        target.pop("password", None)
                    with self.assertRaisesRegex(binding.BindingError, "password 明文字段"):
                        self.verify()

    def test_target_set_missing_extra_duplicate_or_control_alias_fails_closed(self):
        original = self.table["tenant_data"]["targets"]
        extra = {**original[0], "key": "extra", "database": "source_extra"}
        for targets in (
            original[1:], [*original, extra], [*original, original[0]],
            [*original, {"key": "shared-control", "mode": "dedicated", "kind": "mysql"}],
        ):
            with self.subTest(count=len(targets)):
                self.table["tenant_data"]["targets"] = targets
                with self.assertRaises(binding.BindingError):
                    self.verify()

    def test_explicit_control_target_and_empty_read_replicas_are_supported(self):
        self.table["tenant_data"]["targets"].append(
            {"key": "shared-control", "kind": "control", "mode": "shared"}
        )
        self.table["database"].update(replicas=[], sources=[])
        self.assertEqual(len(self.verify()["databases"]), 4)

    def test_explicit_disabled_tls_and_https_object_scheme_are_compared_exactly(self):
        self.client.write_text(self.client.read_text().replace("REQUIRED", "DISABLED"))
        self.refresh_defaults_hash()
        self.table["database"]["primary"]["tls_mode"] = "disabled"
        for target in self.table["tenant_data"]["targets"]:
            target["tls_mode"] = "disabled"
        self.table["object_storage"].update(
            use_ssl=True, endpoint="https://127.0.0.1:29000/"
        )
        self.plan["source"]["s3"]["endpoint"] = "https://127.0.0.1:29000"
        self.assertTrue(all(item["tls_mode"] == "disabled" for item in self.verify()["databases"]))

    def test_no_implicit_tenant_port_or_tls_defaults_are_guessed(self):
        original = copy.deepcopy(self.table["tenant_data"]["targets"])
        for field in ("port", "tls_mode", "password"):
            with self.subTest(field=field):
                targets = copy.deepcopy(original)
                del targets[0][field]
                self.table["tenant_data"]["targets"] = targets
                with self.assertRaises(binding.BindingError):
                    self.verify()

    def test_file_table_replicas_and_sources_must_be_explicit_empty_arrays(self):
        for field in ("replicas", "sources"):
            for value in ([{"host": "127.0.0.1"}], "private-secret-malformed-json", {}, None):
                with self.subTest(field=field, value=value):
                    self.table["database"][field] = value
                    with self.assertRaises(binding.BindingError):
                        self.verify()
                    del self.table["database"][field]

    def test_tls_file_contents_cannot_be_proven_by_path_only(self):
        for connection in (
            self.table["database"]["primary"], self.table["tenant_data"]["targets"][0]
        ):
            for field in binding.TLS_FILES:
                with self.subTest(field=field, primary=connection is self.table["database"]["primary"]):
                    connection[field] = "unbound-certificate.pem"
                    with self.assertRaisesRegex(binding.BindingError, "TLS 文件"):
                        self.verify()
                    del connection[field]

    def test_object_endpoint_region_tls_backend_and_each_secret_must_match(self):
        original = self.table["object_storage"]
        changes = {
            "endpoint": "http://127.0.0.1:29001", "region": "us-west-2",
            "use_ssl": True, "backend": "local", "access_key": "changed-access",
            "secret_key": "changed-secret",
        }
        for field, value in changes.items():
            with self.subTest(field=field):
                self.table["object_storage"] = {**original, field: value}
                with self.assertRaises(binding.BindingError):
                    self.verify()
        self.table["object_storage"] = original
        self.plan["source"]["s3"]["secret_key"] = "external-secret-changed"
        with self.assertRaisesRegex(binding.BindingError, "对象认证"):
            self.verify()

    def test_portable_quoted_password_is_normalized_before_binding(self):
        self.client.write_text(self.client.read_text().replace("private-db-value", '"private-db-value"'))
        self.refresh_defaults_hash()
        self.assertEqual(len(self.verify()["databases"]), 4)

    def test_incomplete_ambiguous_defaults_or_changed_file_are_rejected(self):
        original = self.client.read_text()
        for content in (
            original + "ssl-ca=external.pem\n", original.replace("ssl-mode=REQUIRED\n", ""),
            original.replace("private-db-value", '"private db value"'),
            original.replace("private-db-value", '"private-db-value\\"'),
            original.replace("private-db-value", "private-db-value#comment"),
        ):
            with self.subTest():
                self.client.write_text(content)
                self.refresh_defaults_hash()
                with self.assertRaises(binding.BindingError):
                    self.verify()
        self.client.write_text(original)
        with self.assertRaisesRegex(binding.BindingError, "摘要已变化"):
            self.verify()

    def test_duplicate_physical_database_is_rejected_even_if_plan_matches(self):
        targets = self.table["tenant_data"]["targets"]
        targets[1]["database"] = targets[0]["database"]
        self.plan["source"]["databases"][2]["database"] = targets[0]["database"]
        with self.assertRaisesRegex(binding.BindingError, "同一物理数据库"):
            self.verify()

    def test_parse_failures_and_mismatches_never_echo_secret_values(self):
        self.table["tenant_data"]["targets"] = "private-secret-malformed-json"
        with self.assertRaises(binding.BindingError) as caught:
            self.verify()
        self.assertNotIn("private-secret", str(caught.exception))
        self.loader.side_effect = ValueError("password = private-secret-invalid-toml")
        with self.assertRaises(binding.BindingError) as caught:
            self.verify()
        self.assertEqual(str(caught.exception), "来源配置不完整或无法安全解析")
        self.assertNotIn("private-secret", str(caught.exception))
        self.assertTrue(caught.exception.__suppress_context__)

    def test_scope_file_and_environment_mismatch_fail_closed(self):
        for value in ("dev", "prod", " test", "test ", "TEST", None, 1, False):
            with self.subTest(app_env=value):
                self.variables["APP_ENV"] = value
                self.loader.reset_mock()
                with self.assertRaisesRegex(binding.BindingError, "来源环境"):
                    self.verify()
                self.loader.assert_not_called()
        self.variables["APP_ENV"] = "test"
        self.table["scope_id"] = "another-scope"
        self.variables["APP_SCOPE_ID"] = self.plan["source"]["scope_id"]
        with self.assertRaisesRegex(binding.BindingError, "scope"):
            self.verify()
        self.loader.assert_called_once_with(self.backend, {"APP_ENV": "test"}, config_dir=None)

    def test_binding_reads_shared_file_table_with_explicit_arguments(self):
        result = self.verify()
        self.loader.assert_called_once_with(self.backend, {"APP_ENV": "test"}, config_dir=None)
        self.assertEqual(result["scope_id"], self.table["scope_id"])
        self.assertEqual(
            {item["database"] for item in result["databases"]},
            {item["database"] for item in self.plan["source"]["databases"]},
        )
        self.assertEqual(result["s3"]["region"], self.table["object_storage"]["region"])

    def test_malicious_legacy_variables_do_not_change_result_or_loader_request(self):
        expected = self.verify()
        legacy = {
            "APP_CONFIG_DIR": object(), "APP_SCOPE_ID": "wrong-scope",
            "APP_DATABASE_HOST": "::1", "APP_DATABASE_PORT": "invalid-secret",
            "APP_DATABASE_NAME": "other", "APP_DATABASE_USERNAME": [], "APP_DATABASE_PASSWORD": {},
            "APP_DATABASE_REPLICAS": "not-json", "APP_DATABASE_SOURCES": 1,
            "APP_TENANT_DATA_TARGETS": b"private-secret",
            "APP_OBJECT_STORAGE_ENDPOINT": "private-secret", "APP_OBJECT_STORAGE_REGION": None,
            "APP_OBJECT_STORAGE_BACKEND": "local", "APP_OBJECT_STORAGE_USE_SSL": "private-secret",
            "APP_OBJECT_STORAGE_ACCESS_KEY": object(), "APP_OBJECT_STORAGE_SECRET_KEY": False,
            "EXTERNAL_ACCESS": "wrong-access", "EXTERNAL_SECRET": "wrong-secret",
            "APP_SOURCE_PASSWORD": "wrong-password", 42: object(),
        }
        legacy.update({key + "_FILE": object() for key in legacy if isinstance(key, str)})
        self.variables.update(legacy)
        self.loader.reset_mock()
        self.assertEqual(self.verify(), expected)
        self.loader.assert_called_once_with(self.backend, {"APP_ENV": "test"}, config_dir=None)
        for name in legacy:
            with self.subTest(variable=name):
                self.variables = {"APP_ENV": "test", name: legacy[name]}
                self.assertEqual(self.verify(), expected)

    def test_variables_and_default_os_environment_only_read_app_env(self):
        class AppEnvOnly(Mapping):
            def __getitem__(self, key):
                if key != "APP_ENV":
                    raise AssertionError("不得读取其他环境项")
                return "test"

            def __iter__(self):
                raise AssertionError("不得扫描环境映射")

            def __len__(self):
                raise AssertionError("不得扫描环境映射")

        expected = self.verify()
        self.assertEqual(binding.source_binding(self.backend, self.plan, AppEnvOnly()), expected)
        with mock.patch.object(binding.os, "environ", AppEnvOnly()):
            self.assertEqual(binding.source_binding(self.backend, self.plan), expected)
        self.loader.assert_called_with(self.backend, {"APP_ENV": "test"}, config_dir=None)

    def test_explicit_config_dir_is_forwarded_without_normalization(self):
        for directory in (None, "./配置 目录/../选定", self.config / "绝对 配置"):
            with self.subTest(config_dir=directory):
                self.loader.reset_mock()
                self.verify(config_dir=directory)
                self.loader.assert_called_once_with(
                    self.backend, {"APP_ENV": "test"}, config_dir=directory
                )

    def test_missing_direct_credentials_and_legacy_fields_are_rejected(self):
        primary = self.table["database"]["primary"]
        password = primary.pop("password")
        with self.assertRaises(binding.BindingError):
            self.verify()
        primary["password"] = password
        primary["password_env"] = "APP_SOURCE_PASSWORD"
        with self.assertRaisesRegex(binding.BindingError, "password 明文字段"):
            self.verify()
        del primary["password_env"]
        original = copy.deepcopy(self.plan["source"]["s3"])
        for field in ("access_key", "secret_key"):
            for direct in (False, True):
                with self.subTest(field=field, direct=direct):
                    expected = copy.deepcopy(original)
                    if not direct:
                        del expected[field]
                    self.plan["source"]["s3"] = expected
                    if not direct:
                        with self.assertRaises(binding.BindingError):
                            self.verify()
                    expected[field + "_env"] = "EXTERNAL_SECRET"
                    with self.assertRaises(binding.BindingError):
                        self.verify()
        self.plan["source"]["s3"] = original
        for field in ("access_key", "secret_key"):
            saved = self.table["object_storage"].pop(field)
            with self.subTest(config_field=field), self.assertRaises(binding.BindingError):
                self.verify()
            self.table["object_storage"][field] = saved

    def test_shared_control_cannot_declare_independent_connection_fields(self):
        targets = self.table["tenant_data"]["targets"]
        control = {"key": "shared-control", "kind": "control", "mode": "shared"}
        targets.append(control)
        for field in (*binding.DATABASE_FIELDS, "max_connections", "password_env"):
            with self.subTest(field=field):
                control[field] = 3306 if field in ("port", "max_connections") else "private-secret"
                with self.assertRaisesRegex(binding.BindingError, "复用明确的 primary"):
                    self.verify()
                del control[field]

    def test_scope_is_required_in_file_table_and_plan(self):
        for owner in (self.table, self.plan["source"]):
            original = owner["scope_id"]
            for value in (None, "", " source-fixture", 1):
                owner["scope_id"] = value
                with self.subTest(value=value), self.assertRaises(binding.BindingError):
                    self.verify()
            del owner["scope_id"]
            with self.assertRaises(binding.BindingError):
                self.verify()
            owner["scope_id"] = original

    def test_plan_database_target_set_is_exact(self):
        original = self.plan["source"]["databases"]
        for declared in (original[1:], [*original, original[0]],
                         [*original, {**original[0], "key": "unlisted"}]):
            self.plan["source"]["databases"] = declared
            with self.subTest(count=len(declared)), self.assertRaises(binding.BindingError):
                self.verify()


if __name__ == "__main__":
    unittest.main()
