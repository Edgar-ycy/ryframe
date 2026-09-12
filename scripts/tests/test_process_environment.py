"""受限进程环境构造与串行切换回归。"""

from __future__ import annotations

import os
from pathlib import Path
import sys
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import process_environment as environment


class ConfiguredEnvironmentTests(unittest.TestCase):
    def test_only_inherits_system_locale_and_processor_values(self):
        inherited = {
            "PATH": "tools",
            "LC_ALL": "C.UTF-8",
            "PROCESSOR_ARCHITECTURE": "AMD64",
            "HTTP_PROXY": "http://proxy.invalid",
            "APP_AUTH_JWT_SECRET": "secret",
        }
        self.assertEqual(
            environment.configured({"APP_SCOPE_ID": "scope"}, inherited),
            {
                "PATH": "tools",
                "LC_ALL": "C.UTF-8",
                "PROCESSOR_ARCHITECTURE": "AMD64",
                "APP_SCOPE_ID": "scope",
            },
        )

    def test_private_values_override_inherited_names_case_insensitively(self):
        self.assertEqual(
            environment.configured({"Path": "private"}, {"PATH": "inherited", "TEMP": "temp"}),
            {"TEMP": "temp", "Path": "private"},
        )

    def test_invalid_or_case_duplicated_private_mapping_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "文本映射"):
            environment.configured({"BAD": 1}, {})
        with self.assertRaisesRegex(ValueError, "大小写重复"):
            environment.configured({"PATH": "one", "Path": "two"}, {})


class EnvironmentSwitchTests(unittest.TestCase):
    def test_nested_sides_restore_outer_then_caller_environment(self):
        before = dict(os.environ)
        environments = environment.Environments(
            {"APP_SCOPE_ID": "source"}, {"APP_SCOPE_ID": "target"}
        )
        with environments.use("source"):
            self.assertEqual(dict(os.environ), {"APP_SCOPE_ID": "source"})
            with environments.use("target"):
                self.assertEqual(dict(os.environ), {"APP_SCOPE_ID": "target"})
            self.assertEqual(dict(os.environ), {"APP_SCOPE_ID": "source"})
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
        environments = environment.Environments({"APP_SCOPE_ID": "source"}, {})
        with self.assertRaises(ValueError):
            with environments.use("source"):
                os.environ["APP_SCOPE_ID"] = "drift"
        self.assertEqual(dict(os.environ), before)

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
                values.append(dict(os.environ))
                entered.set()

        with outer.use("source"):
            thread = threading.Thread(target=second)
            thread.start()
            self.assertTrue(ready.wait(timeout=5))
            self.assertFalse(entered.is_set())
            self.assertEqual(dict(os.environ), {"SIDE": "first"})
        thread.join(timeout=5)
        self.assertFalse(thread.is_alive())
        self.assertEqual(values, [{"SIDE": "second"}])
        self.assertEqual(dict(os.environ), before)


if __name__ == "__main__":
    unittest.main()
