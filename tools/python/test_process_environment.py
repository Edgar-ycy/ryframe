"""受限进程环境构造与串行切换回归。"""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import process_environment as environment


class ConfiguredEnvironmentTests(unittest.TestCase):
    def test_only_inherits_system_locale_and_processor_values(self):
        inherited = {
            "Path": "tool-path",
            "LC_ALL": "C.UTF-8",
            "PROCESSOR_ARCHITECTURE": "AMD64",
            "temp": "ambient-temp",
            "app_env": "prod",
            "cargo_home": "unsafe",
            "https_proxy": "unsafe",
            "CUSTOM_PROXY": "unsafe",
            "safe": "kept",
            "HTTP_PROXY": "http://proxy.invalid",
            "APP_AUTH_JWT_SECRET": "secret",
        }
        self.assertEqual(
            environment.configured({"TEMP": "private-temp", "APP_ENV": "test"}, inherited),
            {
                "Path": "tool-path",
                "LC_ALL": "C.UTF-8",
                "PROCESSOR_ARCHITECTURE": "AMD64",
                "TEMP": "private-temp",
                "APP_ENV": "test",
            },
        )

    def test_invalid_or_case_duplicated_private_mapping_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "文本映射"):
            environment.configured({"BAD": 1}, {})
        with self.assertRaisesRegex(ValueError, "大小写重复"):
            environment.configured({"PATH": "one", "Path": "two"}, {})


class EnvironmentSwitchTests(unittest.TestCase):
    def test_nested_sides_restore_outer_then_caller_environment(self):
        inherited = {"PATH": os.environ["PATH"], "SYSTEMROOT": os.environ.get("SYSTEMROOT", "original-os-system-root"),
                     "TEMP": os.environ.get("TEMP", "."), "APP_ENV": "prod", "APP_AUTH_JWT_SECRET": "ambient-jwt",
                     "AWS_SECRET_ACCESS_KEY": "ambient-aws", "HTTPS_PROXY": "http://proxy.invalid",
                     "CARGO_HOME": "unselected-cargo-home"}
        with patch.dict(os.environ, inherited, clear=True):
            before = dict(os.environ)
            declarations = {"source": {"APP_ENV": "test"}, "target": {"APP_ENV": "dev"}}
            environments = environment.Environments(declarations["source"], declarations["target"])
            bindings = dict(environments.bindings)
            expected = {key: inherited[key] for key in ("PATH", "SYSTEMROOT", "TEMP")}
            with environments.use("source") as source:
                self.assertIs(source, environments.values["source"])
                self.assertEqual(source, {"APP_ENV": "test"})
                self.assertEqual(dict(os.environ), {**expected, "APP_ENV": "test"})
                subprocess.run(["git", "--version"], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               check=True, timeout=15)
                with environments.use("target") as target:
                    self.assertEqual(target, {"APP_ENV": "dev"})
                    self.assertEqual(dict(os.environ), {**expected, "APP_ENV": "dev"})
                self.assertEqual(dict(os.environ), {**expected, "APP_ENV": "test"})
            self.assertEqual(environments.values, declarations)
            self.assertEqual(environments.bindings, bindings)
            self.assertEqual(dict(os.environ), before)

    def test_body_exception_restores_environment_without_masking(self):
        before = dict(os.environ)
        environments = environment.Environments({"APP_SCOPE_ID": "source"}, {})
        with self.assertRaisesRegex(RuntimeError, "original"):
            with environments.use("source"):
                raise RuntimeError("original")
        self.assertEqual(dict(os.environ), before)

    def test_invalid_os_key_restores_partially_installed_environment(self):
        before = dict(os.environ)
        environments = environment.Environments(
            {"FIRST": "installed", "BAD=KEY": "invalid"}, {}
        )
        with self.assertRaises((ValueError, OSError)):
            with environments.use("source"):
                self.fail("无效环境不能进入业务")
        self.assertEqual(dict(os.environ), before)

    def test_environment_drift_is_rejected_after_restoring(self):
        before = dict(os.environ)
        environments = environment.Environments({"APP_ENV": "test"}, {})
        bindings = dict(environments.bindings)
        for field, value in (("APP_ENV", "dev"), ("PATH", "drifted-tool-path")):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "已恢复调用方环境"):
                with environments.use("source"):
                    os.environ[field] = value
            self.assertEqual(dict(os.environ), before)
            self.assertEqual(environments.values["source"], {"APP_ENV": "test"})
            self.assertEqual(environments.bindings, bindings)

    def test_returned_binding_mutation_is_rejected_and_restored(self):
        before = dict(os.environ)
        environments = environment.Environments({"APP_SCOPE_ID": "source"}, {})
        with self.assertRaises(ValueError):
            with environments.use("source") as value:
                value["APP_SCOPE_ID"] = "drift"
        self.assertEqual(dict(os.environ), before)
        with self.assertRaises(ValueError):
            with environments.use("source"):
                self.fail("变化的绑定不能再次使用")

    def test_caller_maps_are_copied_and_unknown_side_is_rejected(self):
        source = {"APP_SCOPE_ID": "source"}
        environments = environment.Environments(source, {})
        source["APP_SCOPE_ID"] = "later"
        with environments.use("source"):
            self.assertEqual(os.environ["APP_SCOPE_ID"], "source")
        with self.assertRaises(ValueError):
            with environments.use("unknown"):
                self.fail("未知侧不能进入")

    def test_same_instance_is_rejected_across_threads(self):
        environments = environment.Environments({}, {})
        errors = []

        def attempt():
            try:
                with environments.use("source"):
                    errors.append("entered")
            except ValueError:
                errors.append("rejected")

        thread = threading.Thread(target=attempt)
        thread.start()
        thread.join(timeout=5)
        self.assertFalse(thread.is_alive())
        self.assertEqual(errors, ["rejected"])

    def test_separate_instances_are_serialized(self):
        before = dict(os.environ)
        outer = environment.Environments({"SIDE": "first"}, {})
        ready, entered = threading.Event(), threading.Event()
        values = []

        def second():
            other = environment.Environments({"SIDE": "second"}, {})
            ready.set()
            with other.use("source"):
                values.append(os.environ["SIDE"])
                entered.set()

        with outer.use("source"):
            thread = threading.Thread(target=second)
            thread.start()
            self.assertTrue(ready.wait(timeout=5))
            self.assertFalse(entered.is_set())
            self.assertEqual(os.environ["SIDE"], "first")
        thread.join(timeout=5)
        self.assertFalse(thread.is_alive())
        self.assertEqual(values, ["second"])
        self.assertEqual(dict(os.environ), before)


if __name__ == "__main__":
    unittest.main()
