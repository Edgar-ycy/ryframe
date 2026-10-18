"""显式工具绑定的严格解析、文件保护与逐次 stdin 调用回归。"""

import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import artifact_digests
import rate_limit_client as client
import rate_limit_tool as tool
from workspace_directory import WorkspaceDirectory


ROOT = Path(__file__).resolve().parents[2]


class DescriptorTests(unittest.TestCase):
    def setUp(self):
        self.value = {
            "format_version": 1,
            "kind": "ryframe-native-rate-limit-tool",
            "path": str(ROOT / "xtask.exe"),
            "bytes": 132,
            "sha256": "a" * 64,
        }

    def parse(self, raw):
        with mock.patch.dict(os.environ, {tool.TOOL_ENVIRONMENT: raw}, clear=True):
            return tool.tool_binding()

    def test_absent_binding_alone_selects_the_unbound_entry(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertIsNone(tool.tool_binding())
        self.assertEqual(self.parse(json.dumps(self.value)), self.value)

    def test_duplicate_extra_and_missing_fields_are_rejected(self):
        encoded = json.dumps(self.value)
        for field, item in self.value.items():
            duplicate = encoded[:-1] + "," + json.dumps(field) + ":" + json.dumps(item) + "}"
            with self.subTest(duplicate=field), self.assertRaises(ValueError):
                self.parse(duplicate)
            missing = {key: item for key, item in self.value.items() if key != field}
            with self.subTest(missing=field), self.assertRaises(ValueError):
                self.parse(json.dumps(missing))
        with self.assertRaises(ValueError):
            self.parse(json.dumps({**self.value, "extra": "private-value"}))

    def test_strict_types_versions_sizes_absolute_paths_and_hashes(self):
        invalid = {
            "format_version": [True, False, 1.0, "1", 0, 2, None],
            "kind": [None, 1, [], "", "other"],
            "bytes": [True, False, 0, -1, 132.0, "132", None, []],
            "path": [None, 1, [], "", "xtask.exe", "./xtask.exe", "../xtask.exe",
                     "D:xtask.exe", "\\xtask.exe", str(ROOT / "bad\0.exe")],
            "sha256": [None, 1, [], "", "a" * 63, "a" * 65, "A" * 64,
                       "g" * 64, "a" * 63 + "\n", " " + "a" * 64],
        }
        for field, items in invalid.items():
            for item in items:
                with self.subTest(field=field, item=item), self.assertRaises(ValueError):
                    self.parse(json.dumps({**self.value, field: item}))

    def test_empty_malformed_non_object_and_oversized_descriptors(self):
        for raw in ("", " ", "{", "[]", "null", "true", "1", '"text"',
                    " " * (tool.MAX_BINDING_BYTES + 1),
                    "中" * (tool.MAX_BINDING_BYTES // 3 + 1)):
            with self.subTest(raw=raw[:20]), self.assertRaises(ValueError):
                self.parse(raw)
        encoded = json.dumps(self.value)
        boundary = encoded + " " * (tool.MAX_BINDING_BYTES - len(encoded.encode("utf-8")))
        self.assertEqual(self.parse(boundary), self.value)
        with self.assertRaises(ValueError):
            self.parse(boundary + " ")


class ToolFixture(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.executable = self.directory / "限流 工具.exe"
        data = bytearray(132)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 0x3C, 128)
        data[128:] = b"PE\0\0"
        self.content = bytes(data)
        self.executable.write_bytes(self.content)
        self.value = {
            "format_version": 1,
            "kind": "ryframe-native-rate-limit-tool",
            "path": str(self.executable),
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
        self.environment = {
            tool.TOOL_ENVIRONMENT: json.dumps(self.value),
            "CARGO": "must-never-start-cargo",
            "APP_DATABASE_PASSWORD": "private-value",
            "APP_ENV": "ambient-value",
            "PYTHONUTF8": "1",
            "PYTHONIOENCODING": "cp936",
        }

    def assert_released(self):
        self.assertIsNone(artifact_digests._ACTIVE)
        self.executable.write_bytes(self.content)

    def invoke(self, operation):
        if operation == "login-budget":
            return client.login_budget_environment(
                self.directory, {"APP_ENV": "explicit"}, self.directory / "budget.json"
            )
        function = {
            "authority": client.rate_limit_authority,
            "load-table": client.load_app_table,
            "settings": client.rate_limit_settings,
        }[operation]
        return function(self.directory, {"APP_ENV": "explicit"})


class BoundClientTests(ToolFixture):
    def test_all_operations_use_exact_bound_argv_and_private_stdin(self):
        for operation in sorted(client.OPERATIONS):
            result = {} if operation != "login-budget" else {
                key: "fixture" for key in client.LOGIN_BUDGET_FIELDS
            }
            completed = subprocess.CompletedProcess(
                [], 0, json.dumps({"ok": True, "result": result}).encode("utf-8")
            )
            with (
                self.subTest(operation=operation),
                mock.patch.dict(os.environ, self.environment, clear=True),
                mock.patch.object(client.subprocess, "run", return_value=completed) as run,
            ):
                self.assertEqual(self.invoke(operation), result)
            run.assert_called_once()
            call = run.call_args
            self.assertEqual(call.args, ([str(self.executable), "__rate-limit", operation],))
            self.assertNotIn("shell", call.kwargs)
            self.assertEqual(call.kwargs["cwd"], client.ROOT)
            self.assertEqual(call.kwargs["timeout"], 30)
            self.assertEqual(call.kwargs["stdout"], subprocess.PIPE)
            self.assertEqual(call.kwargs["stderr"], subprocess.DEVNULL)
            if os.name == "nt":
                self.assertEqual(call.kwargs["creationflags"], subprocess.CREATE_NO_WINDOW)
            child = call.kwargs["env"]
            self.assertFalse(any(key.upper().startswith("APP_") for key in child))
            self.assertNotIn(tool.TOOL_ENVIRONMENT, child)
            self.assertNotIn("PYTHONUTF8", child)
            self.assertNotIn("PYTHONIOENCODING", child)
            request = json.loads(call.kwargs["input"].decode("utf-8"))
            expected = {"backend": str(self.directory), "environment": {"APP_ENV": "explicit"}}
            if operation == "login-budget":
                expected["state_path"] = str(self.directory / "budget.json")
            self.assertEqual(request, expected)
            self.assert_released()

    def test_each_call_sends_fresh_configuration_and_receives_fresh_results(self):
        table = self.directory / "app.toml"
        requests = []

        def respond(argv, **kwargs):
            requests.append(json.loads(kwargs["input"].decode("utf-8")))
            result = {"contents": table.read_text(encoding="utf-8")}
            return subprocess.CompletedProcess(
                argv, 0, json.dumps({"ok": True, "result": result}).encode("utf-8")
            )

        environment = {"APP_ENV": "first"}
        with (
            mock.patch.dict(os.environ, self.environment, clear=True),
            mock.patch.object(client.subprocess, "run", side_effect=respond) as run,
        ):
            table.write_text("first", encoding="utf-8")
            self.assertEqual(client.load_app_table(self.directory, environment), {"contents": "first"})
            environment["APP_ENV"] = "second"
            table.write_text("second", encoding="utf-8")
            self.assertEqual(client.load_app_table(self.directory, environment), {"contents": "second"})
        self.assertEqual(run.call_count, 2)
        self.assertEqual([item["environment"] for item in requests],
                         [{"APP_ENV": "first"}, {"APP_ENV": "second"}])
        self.assert_released()

    def test_invalid_bindings_never_spawn_or_fall_back(self):
        for change in ({"sha256": "0" * 64}, {"bytes": self.value["bytes"] + 1},
                       {"path": str(self.directory / "missing.exe")},
                       {"path": str(self.directory)}, {"format_version": True}):
            environment = {**self.environment, tool.TOOL_ENVIRONMENT: json.dumps({**self.value, **change})}
            with (
                self.subTest(change=change),
                mock.patch.dict(os.environ, environment, clear=True),
                mock.patch.object(client.subprocess, "run") as run,
                self.assertRaises((ValueError, RuntimeError)),
            ):
                self.invoke("settings")
            run.assert_not_called()
            self.assert_released()
        for raw in ("", "{}", "not-json"):
            with (
                self.subTest(raw=raw),
                mock.patch.dict(os.environ, {tool.TOOL_ENVIRONMENT: raw}, clear=True),
                mock.patch.object(client.subprocess, "run") as run,
                self.assertRaises(ValueError),
            ):
                self.invoke("load-table")
            run.assert_not_called()

    def test_bound_spawn_failure_attempts_only_the_bound_tool(self):
        for error in (OSError("fixture"), subprocess.TimeoutExpired("fixture", 30)):
            with (
                self.subTest(error=type(error).__name__),
                mock.patch.dict(os.environ, self.environment, clear=True),
                mock.patch.object(client.subprocess, "run", side_effect=error) as run,
                self.assertRaisesRegex(RuntimeError, "无法启动"),
            ):
                self.invoke("authority")
            run.assert_called_once()
            self.assertEqual(run.call_args.args[0], [str(self.executable), "__rate-limit", "authority"])
            self.assert_released()

    def test_binding_is_read_again_for_every_call(self):
        completed = subprocess.CompletedProcess([], 0, b'{"ok":true,"result":{}}')
        with (
            mock.patch.dict(os.environ, self.environment, clear=True),
            mock.patch.object(client.subprocess, "run", return_value=completed) as run,
        ):
            self.assertEqual(self.invoke("settings"), {})
            os.environ[tool.TOOL_ENVIRONMENT] = ""
            with self.assertRaises(ValueError):
                self.invoke("settings")
        run.assert_called_once()
        self.assert_released()


class ProtectionTests(ToolFixture):
    def test_real_protection_releases_after_success_and_original_body_errors(self):
        for failure in (None, RuntimeError("fixture"), KeyboardInterrupt("fixture")):
            with self.subTest(failure=type(failure).__name__):
                with mock.patch.object(tool, "file_digest", wraps=tool.file_digest) as digest:
                    try:
                        with tool.verified_tool(self.value) as executable:
                            self.assertEqual(executable, str(self.executable))
                            if os.name == "nt":
                                self.assertIsNotNone(artifact_digests._ACTIVE)
                                with self.assertRaises(OSError):
                                    self.executable.write_bytes(b"changed")
                            if failure is not None:
                                raise failure
                    except BaseException as error:
                        self.assertIs(error, failure)
                self.assertEqual(digest.call_count, 2)
                self.assert_released()

    def test_body_failure_still_checks_identity_and_digest_and_keeps_both_errors(self):
        original_stat = Path.stat
        expected = {key: self.value[key] for key in ("bytes", "sha256")}
        for change in ("identity", "digest"):
            changed = False
            body_error = RuntimeError("original-operation-error")

            def stat(path, *args, **kwargs):
                observed = original_stat(path, *args, **kwargs)
                if changed and path == self.executable and change == "identity":
                    fields = {key: getattr(observed, key) for key in dir(observed) if key.startswith("st_")}
                    fields["st_ino"] += 1
                    return SimpleNamespace(**fields)
                return observed

            observed_digest = expected if change == "identity" else {**expected, "sha256": "0" * 64}
            with (
                self.subTest(change=change),
                mock.patch.object(Path, "stat", stat),
                mock.patch.object(tool, "file_digest", side_effect=[expected, observed_digest]) as digest,
                self.assertRaises(ExceptionGroup) as raised,
            ):
                with tool.verified_tool(self.value):
                    changed = True
                    raise body_error
            self.assertEqual(digest.call_count, 2)
            self.assertIs(raised.exception.exceptions[0], body_error)
            self.assertIsInstance(raised.exception.exceptions[1], ValueError)
            changed = False
            self.assert_released()

    def test_post_execution_digest_failure_is_rejected_without_a_body_error(self):
        expected = {key: self.value[key] for key in ("bytes", "sha256")}
        with (
            mock.patch.object(tool, "file_digest", side_effect=[expected, OSError("fixture")]),
            self.assertRaisesRegex(OSError, "fixture"),
        ):
            with tool.verified_tool(self.value):
                pass
        self.assert_released()

    def test_interrupt_and_integrity_errors_are_both_preserved(self):
        expected = {key: self.value[key] for key in ("bytes", "sha256")}
        primary = KeyboardInterrupt("fixture")
        integrity = OSError("digest-fixture")
        with (
            mock.patch.object(tool, "file_digest", side_effect=[expected, integrity]),
            self.assertRaises(BaseExceptionGroup) as raised,
        ):
            with tool.verified_tool(self.value):
                raise primary
        self.assertEqual(raised.exception.exceptions, (primary, integrity))
        self.assert_released()

    @unittest.skipUnless(os.name == "nt", "需要 Windows 保护句柄")
    def test_release_failure_attempts_close_and_preserves_the_original_error(self):
        original_close = artifact_digests._Protected.close
        for primary in (None, RuntimeError("original-operation-error")):
            closed = []

            def close(protected):
                original_close(protected)
                closed.append(protected.handle)
                raise OSError("release-fixture")

            with (
                self.subTest(primary=type(primary).__name__),
                mock.patch.object(artifact_digests._Protected, "close", close),
                self.assertRaises(OSError if primary is None else RuntimeError) as raised,
            ):
                with tool.verified_tool(self.value):
                    if primary is not None:
                        raise primary
            self.assertEqual(closed, [None])
            if primary is not None:
                self.assertIs(raised.exception, primary)
                self.assertTrue(any("OSError" in note for note in primary.__notes__))
            self.assert_released()

    def test_directory_and_special_files_are_rejected_before_reading(self):
        directory_binding = {**self.value, "path": str(self.directory)}
        with mock.patch.object(tool, "file_digest") as digest:
            with self.assertRaisesRegex(ValueError, "普通文件"):
                with tool.verified_tool(directory_binding):
                    self.fail("目录不能作为限流工具")
            digest.assert_not_called()
        original_lstat = Path.lstat

        def special_file(path, *args, **kwargs):
            if path == self.executable:
                return SimpleNamespace(st_mode=0o010600, st_file_attributes=0)
            return original_lstat(path, *args, **kwargs)

        with (
            mock.patch.object(Path, "is_symlink", return_value=False),
            mock.patch.object(Path, "lstat", special_file),
            mock.patch.object(tool, "file_digest") as digest,
            self.assertRaisesRegex(ValueError, "普通文件"),
        ):
            with tool.verified_tool(self.value):
                self.fail("特殊文件不能作为限流工具")
        digest.assert_not_called()

    def test_links_and_parent_reparse_points_are_rejected_before_spawning(self):
        original_lstat = Path.lstat
        for linked in (self.executable, self.directory):
            for mode in ("symlink", "reparse"):
                def lstat(path, *args, **kwargs):
                    if path == linked and mode == "reparse":
                        return mock.Mock(st_file_attributes=0x400)
                    return original_lstat(path, *args, **kwargs)

                with (
                    self.subTest(linked=linked.name, mode=mode),
                    mock.patch.dict(os.environ, self.environment, clear=True),
                    mock.patch.object(Path, "is_symlink", lambda path: path == linked and mode == "symlink"),
                    mock.patch.object(Path, "lstat", lstat),
                    mock.patch.object(client.subprocess, "run") as run,
                    self.assertRaisesRegex(ValueError, "链接或重解析点"),
                ):
                    self.invoke("load-table")
                run.assert_not_called()
                self.assert_released()

    @unittest.skipUnless(os.name == "nt", "需要 Windows PE 和保护句柄")
    def test_non_pe_executable_and_protection_acquisition_failure_are_rejected(self):
        self.executable.write_bytes(b"not-a-PE")
        value = {**self.value, **artifact_digests.file_digest(self.executable)}
        with self.assertRaisesRegex(ValueError, "Windows 二进制"):
            with tool.verified_tool(value):
                self.fail("无效 PE 不得进入执行阶段")
        self.assert_released()
        with self.executable.open("r+b"):
            with self.assertRaises(OSError):
                with tool.verified_tool(self.value):
                    self.fail("已有写句柄时不得进入执行阶段")
        self.assert_released()


if __name__ == "__main__":
    unittest.main()
