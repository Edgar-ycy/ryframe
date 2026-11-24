"""通过原生只读协议核对库存配置目录和非敏感环境摘要。"""

from collections.abc import Mapping
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_inventory as inventory
from restore_reference_plan import plan_hash
from workspace_directory import WorkspaceDirectory


ROOT = Path(__file__).resolve().parents[2]


class ConfigurationTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.backend = self.root / "execution"
        self.coordinator = self.root / "coordinator"
        self.backend.mkdir()
        self.coordinator.mkdir()
        self.directory = self.make_config(self.backend / "config")
        self.environment = {"APP_ENV": "test"}

    def make_config(self, directory, *, app_env="test", common=False):
        directory.mkdir(parents=True)
        (directory / f"app.{app_env}.toml").write_text("# 离线环境配置\n", encoding="utf-8")
        if common:
            (directory / "app.toml").write_text("# 可选公共配置\n", encoding="utf-8")
        return directory

    def call(self, **kwargs):
        return inventory.configuration(
            self.backend, self.environment,
            evidence_root=self.coordinator, **kwargs,
        )

    def test_default_directory_requires_selected_file_but_not_common_file(self):
        result = self.call()
        self.assertEqual(result["directory"], str(self.directory.resolve()))
        self.assertEqual(set(result["files"]), {"app.test.toml"})
        self.assertEqual(result["environment_sha256"], plan_hash({"APP_ENV": "test"}))
        self.assertEqual(self.call(config_dir=None), result)
        self.assertEqual(inventory.configuration(self.backend, self.environment), result)

    def test_explicit_relative_unicode_directory_and_absolute_execution_directory(self):
        directory = self.make_config(self.backend / "中文 配置", common=True)
        relative = self.call(config_dir="中文 配置")
        absolute = self.call(config_dir=directory)
        self.assertEqual(relative, absolute)
        self.assertEqual(relative["directory"], str(directory.resolve()))
        self.assertEqual(set(relative["files"]), {"app.toml", "app.test.toml"})

    def test_explicit_coordinator_directory_is_allowed(self):
        directory = self.make_config(self.coordinator / "选定 配置")
        self.assertEqual(self.call(config_dir=directory)["directory"], str(directory.resolve()))

    def test_legacy_environment_directory_and_credentials_have_no_effect(self):
        original = self.call()
        self.environment.update({
            "APP_CONFIG_DIR": str(self.root / "outside-private-value"),
            "APP_CONFIG_DIR_FILE": "fixture-private-value",
            "APP_ENV_FILE": "fixture-private-value",
            "APP_SCOPE_ID": "fixture-private-value",
            "APP_DATABASE_PASSWORD": "fixture-private-value",
            "APP_OBJECT_STORAGE_SECRET_KEY": "fixture-private-value",
            "AUTH_PRIVATE": "fixture-private-value",
            "MYSQL_PWD": "fixture-private-value",
        })
        self.assertEqual(self.call(), original)
        explicit = self.make_config(self.coordinator / "explicit")
        self.assertEqual(self.call(config_dir=explicit)["directory"], str(explicit.resolve()))

    def test_only_app_env_and_explicit_paths_are_sent_to_the_native_protocol(self):
        class AppEnvOnly(Mapping):
            def __getitem__(self, key):
                if key != "APP_ENV":
                    raise AssertionError("不得查询其他环境键")
                return "test"

            def __iter__(self):
                raise AssertionError("不得遍历环境")

            def __len__(self):
                raise AssertionError("不得遍历环境")

        self.environment = AppEnvOnly()
        with mock.patch.object(inventory, "_native", wraps=inventory._native) as native:
            result = self.call()
        native.assert_called_once_with("copy-inventory-configuration", {
            "backend": str(self.backend),
            "evidence_root": str(self.coordinator),
            "config_dir": None,
            "app_env": "test",
        })
        self.assertEqual(result["environment_sha256"], plan_hash({"APP_ENV": "test"}))

    def test_environment_digest_is_exact_and_file_selection_follows_app_env(self):
        for app_env in ("dev", "test", "prod"):
            directory = self.make_config(self.backend / app_env, app_env=app_env)
            self.environment = {"APP_ENV": app_env, "APP_PRIVATE": "fixture-private-value"}
            result = self.call(config_dir=directory)
            self.assertEqual(set(result["files"]), {f"app.{app_env}.toml"})
            self.assertEqual(result["environment_sha256"], plan_hash({"APP_ENV": app_env}))

    def test_missing_or_invalid_app_env_cannot_select_an_arbitrary_file(self):
        for environment in ({}, *({"APP_ENV": value} for value in (
            "", "TEST", " test", "test\n", "../private-value", None, 1, False, object(),
        ))):
            with self.subTest(environment=environment), self.assertRaisesRegex(ValueError, "APP_ENV"):
                self.environment = environment
                self.call()

    def test_common_or_unrelated_toml_cannot_replace_the_missing_selected_file(self):
        (self.directory / "app.test.toml").unlink()
        (self.directory / "app.toml").write_text("# 公共配置\n", encoding="utf-8")
        (self.directory / "app.dev.toml").write_text("# 其他环境\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "已选环境文件"):
            self.call()

    def test_missing_directory_and_directory_used_as_selected_file_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "目录必须存在"):
            self.call(config_dir=self.backend / "missing")
        (self.directory / "app.test.toml").unlink()
        (self.directory / "app.test.toml").mkdir()
        with self.assertRaisesRegex(ValueError, "已选环境文件"):
            self.call()

    def test_explicit_path_must_remain_inside_execution_or_coordinator(self):
        outside = self.make_config(self.root / "outside")
        with self.assertRaisesRegex(ValueError, "明确后端内"):
            self.call(config_dir=outside)
        similar_prefix = self.make_config(self.root / "execution-other")
        with self.assertRaisesRegex(ValueError, "明确后端内"):
            self.call(config_dir=similar_prefix)

    def test_invalid_explicit_path_has_fixed_diagnostics(self):
        class InvalidPath:
            def __fspath__(self):
                raise ValueError("fixture-private-value")

        for value in ("", " ", " config", "config ", b"fixture-private-value", 1, object(), InvalidPath()):
            with self.subTest(kind=type(value).__name__), self.assertRaises(ValueError) as raised:
                self.call(config_dir=value)
            self.assertEqual(str(raised.exception), "库存配置目录必须为明确文本路径")

        # 保留上述空值、类型和固定诊断断言，再覆盖控制字符及父目录跳转。
        controls = [chr(code) for code in (*range(32), *range(0x7F, 0xA0))]
        invalid = [f"fixture-private{char}value" for char in controls]
        invalid.extend(("..", "../fixture-private", "nested/../fixture-private",
                        self.backend / "nested" / ".." / "config"))
        for value in invalid:
            with self.subTest(kind=type(value).__name__), self.assertRaises(ValueError) as raised:
                self.call(config_dir=value)
            self.assertEqual(str(raised.exception), "库存配置目录不能包含控制字符或父目录跳转")
            self.assertNotIn("fixture-private", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
