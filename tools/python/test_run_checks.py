"""检查入口的参数边界、显式安装顺序和 unittest 退出传播。"""

import io
import json
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rate_limit_tool as tool
import run_checks


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        coordinator = mock.patch.object(tool, "_COORDINATOR_BINDING", None)
        coordinator.start()
        self.addCleanup(coordinator.stop)
        argv = mock.patch.object(sys, "argv", ["original-check-entry", "original-argument"])
        argv.start()
        self.addCleanup(argv.stop)
        self.value = {
            "format_version": 1,
            "kind": "ryframe-native-rate-limit-tool",
            "path": str(Path(__file__).resolve().parents[2] / "xtask.exe"),
            "bytes": 132,
            "sha256": "a" * 64,
        }
        self.raw = json.dumps(self.value)

    def assert_rejected(self, arguments):
        before = sys.argv.copy()
        stderr = io.StringIO()
        runner = mock.Mock()
        with mock.patch.object(sys, "stderr", stderr), self.assertRaises(SystemExit) as raised:
            run_checks.execute(arguments, runner=runner)
        self.assertEqual(raised.exception.code, 2)
        self.assertEqual(stderr.getvalue(), "检查入口参数或限流工具绑定无效\n")
        self.assertNotIn("private-value", stderr.getvalue())
        self.assertNotIn(self.raw, stderr.getvalue())
        self.assertEqual(sys.argv, before)
        runner.assert_not_called()

    def test_unbound_arguments_preserve_all_unittest_arguments(self):
        arguments = ["--", "-m", "unittest", "discover", "-s", "tools/python", "-v"]
        original = arguments.copy()
        self.assertEqual(
            run_checks.parse_arguments(arguments),
            (None, ["unittest", "discover", "-s", "tools/python", "-v"]),
        )
        self.assertEqual(arguments, original)

    def test_bound_arguments_accept_only_the_front_binding_and_separator(self):
        self.assertEqual(
            run_checks.parse_arguments([
                "--rate-limit-tool-binding", self.raw, "--", "-m", "unittest", "test_module",
            ]),
            (self.raw, ["unittest", "test_module"]),
        )

    def test_unbound_execution_does_not_install_and_passes_exact_runpy_options(self):
        runner = mock.Mock()
        with mock.patch.object(run_checks, "install_binding") as install:
            run_checks.execute(["--", "-m", "unittest", "-q", "test_module"], runner=runner)
        install.assert_not_called()
        self.assertIsNone(tool.tool_binding())
        self.assertEqual(sys.argv, ["unittest", "-q", "test_module"])
        runner.assert_called_once_with("unittest", run_name="__main__", alter_sys=True)

    def test_binding_is_installed_before_the_runner_observes_argv(self):
        def runner(module, **options):
            self.assertEqual(tool.tool_binding(), self.value)
            self.assertEqual(sys.argv, ["unittest", "test_module", "-v"])
            self.assertEqual(module, "unittest")
            self.assertEqual(options, {"run_name": "__main__", "alter_sys": True})

        with mock.patch.object(run_checks, "install_binding", wraps=tool.install_binding) as install:
            run_checks.execute([
                "--rate-limit-tool-binding", self.raw, "--", "-m", "unittest", "test_module", "-v",
            ], runner=runner)
        install.assert_called_once_with(self.raw)
        self.assertEqual(tool._COORDINATOR_BINDING, self.raw)

    def test_missing_binding_value_separator_or_unittest_arguments_is_rejected(self):
        for arguments in (
            [], ["--rate-limit-tool-binding"], ["--rate-limit-tool-binding", self.raw],
            ["--rate-limit-tool-binding", "--", "-m", "unittest"],
            ["--rate-limit-tool-binding", self.raw, "-m", "unittest"],
            ["--rate-limit-tool-binding", self.raw, "--"],
            ["--"], ["--", "-m"], ["-m", "unittest"],
        ):
            with self.subTest(arguments=arguments):
                self.assert_rejected(arguments)
                self.assertIsNone(tool._COORDINATOR_BINDING)

    def test_repeated_binding_parameters_are_rejected_even_when_equal(self):
        for second in (self.raw, "private-value"):
            with self.subTest(second=second):
                self.assert_rejected([
                    "--rate-limit-tool-binding", self.raw,
                    "--rate-limit-tool-binding", second, "--", "-m", "unittest",
                ])
        self.assertIsNone(tool._COORDINATOR_BINDING)

    def test_unknown_bootstrap_parameters_are_rejected(self):
        for arguments in (
            ["--unknown=private-value", "--", "-m", "unittest"],
            ["--rate-limit-tool-binding=" + self.raw, "--", "-m", "unittest"],
            ["private-value", "--", "-m", "unittest"],
            ["--rate-limit-tool-binding", self.raw, "--unknown", "--", "-m", "unittest"],
        ):
            with self.subTest(arguments=arguments):
                self.assert_rejected(arguments)

    def test_wrong_module_or_invocation_form_is_rejected(self):
        for remaining in (
            ["-m", "pytest"], ["-m", "unittest.main"], ["unittest"],
            ["-c", "private-value"], ["-m", "unittest "] , ["--", "-m", "unittest"],
        ):
            with self.subTest(remaining=remaining):
                self.assert_rejected(["--", *remaining])

    def test_invalid_binding_json_or_schema_has_fixed_diagnostics_and_never_installs(self):
        duplicate = self.raw[:-1] + ',"bytes":132}'
        for raw in ("", "private-value", "[]", "null", "{}", duplicate,
                    json.dumps({**self.value, "extra": "private-value"}),
                    json.dumps({**self.value, "bytes": True})):
            with self.subTest(raw=raw):
                self.assert_rejected([
                    "--rate-limit-tool-binding", raw, "--", "-m", "unittest",
                ])
                self.assertIsNone(tool._COORDINATOR_BINDING)

    def test_repeated_execution_cannot_replace_the_installed_host_binding(self):
        runner = mock.Mock()
        arguments = ["--rate-limit-tool-binding", self.raw, "--", "-m", "unittest"]
        run_checks.execute(arguments, runner=runner)
        for raw in (self.raw, json.dumps({**self.value, "bytes": 333})):
            with self.subTest(raw=raw):
                self.assert_rejected([
                    "--rate-limit-tool-binding", raw, "--", "-m", "unittest",
                ])
                self.assertEqual(tool.tool_binding(), self.value)
        runner.assert_called_once()

    def test_unbound_execution_preserves_an_existing_host_binding(self):
        tool.install_binding(self.raw)
        run_checks.execute(["--", "-m", "unittest"], runner=mock.Mock())
        self.assertEqual(tool._COORDINATOR_BINDING, self.raw)

    def test_test_success_and_failure_system_exit_are_propagated_unchanged(self):
        for status in (0, 1, 2, "original-unittest-failure"):
            error = SystemExit(status)
            runner = mock.Mock(side_effect=error)
            with self.subTest(status=status), self.assertRaises(SystemExit) as raised:
                run_checks.execute(["--", "-m", "unittest", "test_module"], runner=runner)
            self.assertIs(raised.exception, error)
            self.assertEqual(sys.argv, ["unittest", "test_module"])

    def test_runner_exceptions_and_interrupts_are_not_converted_to_argument_failures(self):
        for error in (ValueError("original-runner-error"), KeyboardInterrupt("original-interrupt")):
            stderr = io.StringIO()
            with (
                self.subTest(error=type(error).__name__),
                mock.patch.object(sys, "stderr", stderr),
                self.assertRaises(type(error)) as raised,
            ):
                run_checks.execute(["--", "-m", "unittest"], runner=mock.Mock(side_effect=error))
            self.assertIs(raised.exception, error)
            self.assertEqual(stderr.getvalue(), "")

    def test_unittest_owns_its_remaining_options_and_validation(self):
        runner = mock.Mock(side_effect=SystemExit(2))
        with self.assertRaises(SystemExit) as raised:
            run_checks.execute(["--", "-m", "unittest", "--unknown-unittest-option"], runner=runner)
        self.assertEqual(raised.exception.code, 2)
        self.assertEqual(sys.argv, ["unittest", "--unknown-unittest-option"])
        runner.assert_called_once_with("unittest", run_name="__main__", alter_sys=True)


if __name__ == "__main__":
    unittest.main()
