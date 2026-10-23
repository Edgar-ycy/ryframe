"""固定绑定的拒绝路径与协议测试；真实成功路径单独使用主控生成的绑定。"""

from __future__ import annotations

from contextlib import nullcontext
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import traceback
import unittest
from unittest.mock import MagicMock, patch

import frontend_origin_tool as tool
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).absolute().parents[2]
SOURCES = ("Cargo.lock", "xtask/source-inventory.json", "xtask/src/frontend_origin.rs", "xtask/src/main.rs")
SECRET = "PRIVATE-SENTINEL-DO-NOT-PRINT"
INVALID = next(iter(tool.SAFE_ERRORS))


def encoded(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False).encode("utf-8")


class ExecutableSuffixTests(unittest.TestCase):
    def test_posix_suffix_semantics_are_independent_of_the_host_platform(self):
        for name in ("/opt/ryframe/xtask", "/opt/ryframe/tool.cmd", "/opt/ryframe/tool.exe"):
            self.assertTrue(tool._executable_suffix(name, platform="posix"))
        self.assertTrue(tool._executable_suffix("D:/tools/tool.EXE", platform="nt"))
        for name in ("D:/tools/xtask", "D:/tools/tool.cmd", "D:/tools/tool.exe.txt"):
            self.assertFalse(tool._executable_suffix(name, platform="nt"))


class MetadataProtocolTests(unittest.TestCase):
    def test_metadata_keeps_bounded_inventory_limit_independent_of_long_responses(self):
        value = {
            "format_version": 1,
            "kind": "frontend-origin-tool",
            "executable": {"path": "D:/fixture.exe", "sha256": "0" * 64},
            "sources": [{"path": name, "sha256": "0" * 64} for name in SOURCES],
        }
        raw = encoded(value)
        boundary = raw + b" " * (tool.METADATA_LIMIT - len(raw))
        self.assertEqual(tool._metadata(boundary, "受管绑定"), value)
        with self.assertRaises(ValueError):
            tool._metadata(boundary + b" ", "受管绑定")

    def test_response_budget_preserves_input_and_file_capacity(self):
        from restore_frontend_paths import MAX_BYTES

        self.assertEqual(tool.METADATA_LIMIT, 4 * 1024 * 1024)
        self.assertEqual(tool.STDERR_LIMIT, 64 * 1024)
        self.assertEqual(tool.INPUT_LIMIT, 64 * 1024 * 1024)
        self.assertGreaterEqual(tool.RESPONSE_LIMIT, tool.INPUT_LIMIT * 3 + 1024)
        self.assertEqual(MAX_BYTES, 16 * 1024 * 1024)


class ProviderRejectionTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(ROOT / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.executable = self.root / "只供反例.exe"
        self.executable.write_bytes(b"MZ fixture never executed")
        self.binding = self.root / tool.BINDING
        self.binding.parent.mkdir()
        sources = []
        for name in SOURCES:
            file = self.root / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(encoded({"format_version":1,"files":list(SOURCES),"trees":[],"excluded_directories":[]}) if name == tool.POLICY else name.encode())
            sources.append({"path": name, "sha256": hashlib.sha256(file.read_bytes()).hexdigest()})
        self.metadata = {
            "format_version": 1,
            "kind": "frontend-origin-tool",
            "executable": {
                "path": str(self.executable),
                "sha256": hashlib.sha256(self.executable.read_bytes()).hexdigest(),
            },
            "sources": sources,
        }
        self.save(self.metadata)
        root = patch.object(tool, "ROOT", self.root)
        root.start()
        self.addCleanup(root.stop)

    def save(self, value):
        self.binding.write_bytes(encoded(value))

    def failure(self, operation, expected=None):
        with self.assertRaises(ValueError) as caught:
            operation()
        self.assertNotIn(SECRET, str(caught.exception))
        self.assertNotIn(SECRET, "".join(traceback.format_exception(caught.exception)))
        self.assertIsNone(caught.exception.__cause__)
        self.assertIn("请", str(caught.exception))
        if expected:
            self.assertIn(expected, str(caught.exception))
        return caught.exception

    def reject(self, expected=None, *, effect=None, source_effect=None):
        # 仅模拟拒绝/漂移，不把伪程序或伪响应作为真实成功证据。
        token = {"format_version": 1, "kind": "frontend-origin-source-snapshot",
                 "source_count": len(SOURCES), "sha256": "0" * 64}
        with patch.object(tool, "protect_binary_invocation", return_value=nullcontext()) as guard, \
                patch.object(tool, "_source_snapshot", side_effect=source_effect, return_value=token):
            with patch.object(
                tool,
                "_exchange",
                side_effect=effect,
                return_value=(1, encoded({"format_version": 1, "valid": False, "error": INVALID})),
            ) as exchange:
                self.failure(lambda: tool.normalize_origin("https://" + SECRET, allow_empty=False), expected)
        return exchange, guard

    def test_missing_binding_is_configuration_error_without_fallback(self):
        self.binding.unlink()
        exchange, _ = self.reject(tool.BINDING.replace("/", os.sep))
        exchange.assert_not_called()

    def test_binding_utf8_duplicate_keys_constants_size_and_nonobjects_are_rejected(self):
        for raw in (
            b"\xff",
            b"[]",
            b"null",
            b'{"format_version":1,"format_version":1}',
            b'{"format_version":NaN}',
            b'{"format_version":1.0}',
            b" " * (tool.METADATA_LIMIT + 1),
            ('{"' + SECRET).encode(),
        ):
            self.binding.write_bytes(raw)
            exchange, _ = self.reject("绑定 UTF8/JSON")
            exchange.assert_not_called()

    def test_exact_metadata_schema_types_versions_and_source_order_are_required(self):
        mutations = [
            lambda value: value.update(format_version=True),
            lambda value: value.update(format_version=2),
            lambda value: value.update(kind=SECRET),
            lambda value: value.update(extra=SECRET),
            lambda value: value.pop("executable"),
            lambda value: value["executable"].update(bytes=1),
            lambda value: value["executable"].update(sha256="A" * 64),
            lambda value: value.update(sources={}),
            lambda value: value["sources"].reverse(),
            lambda value: value["sources"].pop(1),
            lambda value: value["sources"][0].update(path="../xtask/src/frontend_origin.rs"),
            lambda value: value["sources"][0].update(sha256=1),
            lambda value: value["sources"][0].update(extra=SECRET),
        ]
        for mutate in mutations:
            value = copy.deepcopy(self.metadata)
            mutate(value)
            self.save(value)
            exchange, _ = self.reject("绑定 UTF8/JSON")
            exchange.assert_not_called()



    def test_executable_relative_parentdir_wrong_type_and_non_native_paths_are_rejected(self):
        bad = [
            None,
            123,
            "tool.exe",
            str(self.root) + "/../tool.exe",
            str(self.root) + "/./tool.exe",
            "bad\x00.exe",
        ]
        if os.name == "nt":
            bad.extend(
                [
                    "D:tool.exe",
                    str(self.root / "tool.cmd"),
                    r"\\server\share\tool.exe",
                    r"\\?\UNC\server\share\tool.exe",
                    r"\\?\GLOBALROOT\Device\tool.exe",
                    r"\\.\D:\tool.exe",
                    r"\\?\D:\..\tool.exe",
                    str(self.executable) + ":alternate.exe",
                ]
            )
        for path in bad:
            value = copy.deepcopy(self.metadata)
            value["executable"]["path"] = path
            self.save(value)
            exchange, _ = self.reject("路径无效")
            exchange.assert_not_called()

    def device_binding(self):
        raw = "\\\\?\\" + str(self.executable)
        self.metadata["executable"]["path"] = raw
        self.save(self.metadata)
        return Path(raw), self.binding.read_bytes()

    def test_device_drive_alias_checks_real_samefile_hash_and_preserves_metadata_bytes(self):
        alias, before = self.device_binding()
        if os.name != "nt":
            self.reject("路径无效")
            return
        self.assertTrue(alias.samefile(self.executable))
        with patch.object(tool, "file_digest", wraps=tool.file_digest) as digest:
            exchange, guard = self.reject("地址为空")
        guard.assert_called_once_with({"path": str(self.executable), "sha256": self.metadata["executable"]["sha256"]})
        self.assertEqual(exchange.call_args.args[0], self.executable)
        self.assertGreaterEqual(sum(call.args == (alias,) for call in digest.call_args_list), 3)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertEqual(json.loads(before)["executable"]["path"], str(alias))

    def test_device_alias_samefile_mismatch_or_io_error_blocks_guard_and_invocation(self):
        self.device_binding()
        if os.name != "nt":
            self.reject("路径无效")
            return
        for effect in (False, OSError(SECRET)):
            options = {"side_effect": effect} if isinstance(effect, OSError) else {"return_value": effect}
            with patch.object(Path, "samefile", **options):
                exchange, guard = self.reject("同一文件" if effect is False else "samefile")
            exchange.assert_not_called()
            guard.assert_not_called()

    def test_device_alias_actual_digest_mismatch_is_rejected_before_guard(self):
        alias, _ = self.device_binding()
        if os.name != "nt":
            self.reject("路径无效")
            return
        original = tool.file_digest

        def digest(path):
            value = original(path)
            return {**value, "sha256": "0" * 64} if path == alias else value

        with patch.object(tool, "file_digest", side_effect=digest):
            exchange, guard = self.reject("device 原路径文件身份或实际 SHA-256 不符")
        exchange.assert_not_called()
        guard.assert_not_called()

    def test_device_alias_identity_is_rechecked_after_invocation(self):
        alias, before = self.device_binding()
        if os.name != "nt":
            self.reject("路径无效")
            return
        original = Path.samefile
        changed = False

        def samefile(path, other):
            return False if changed and path == alias else original(path, other)

        def change_alias(_path, _request):
            nonlocal changed
            changed = True
            return 1, encoded({"format_version": 1, "valid": False, "error": INVALID})

        with patch.object(Path, "samefile", samefile):
            self.reject("同一文件", effect=change_alias)
        self.assertEqual(self.binding.read_bytes(), before)


    def test_program_hash_drift_blocks_invocation(self):
        self.executable.write_bytes(b"different native program")
        exchange, _ = self.reject("程序身份或 SHA-256")
        exchange.assert_not_called()

    def reject_link(self, path):
        target = self.root / "link-target"
        target.write_bytes(path.read_bytes())
        path.unlink()
        path.symlink_to(target)
        self.addCleanup(path.unlink)
        exchange, _ = self.reject("链接")
        exchange.assert_not_called()

    def test_linked_binding_is_rejected_with_known_link_cleanup(self):
        self.reject_link(self.binding)

    def test_linked_program_is_rejected_with_known_link_cleanup(self):
        self.reject_link(self.executable)


    def test_unknown_reparse_attribute_is_rejected(self):
        original = os.lstat

        def reparse(path, *args, **kwargs):
            if Path(path).name == self.binding.name:
                from types import SimpleNamespace

                return SimpleNamespace(st_mode=stat.S_IFREG, st_file_attributes=0x400)
            return original(path, *args, **kwargs)

        with patch("restore_frontend_paths.os.lstat", side_effect=reparse):
            exchange, _ = self.reject("重解析点")
        exchange.assert_not_called()

    def test_fixed_binding_is_read_again_and_never_taken_from_environment(self):
        with patch.dict(os.environ, {"FRONTEND_ORIGIN_TOOL": SECRET, "APP_ENV": SECRET}):
            exchange, guard = self.reject("地址为空")
        exchange.assert_called_once()
        guard.assert_called_once_with(self.metadata["executable"])
        self.binding.write_bytes(b"{}")
        exchange, _ = self.reject("绑定 UTF8/JSON")
        exchange.assert_not_called()

    def test_binding_and_program_changes_during_call_fail_closed(self):
        for path in (self.binding, self.executable):
            before = path.read_bytes()

            def changed(_path, _request):
                path.write_bytes(before + b" ")
                return 1, encoded({"format_version": 1, "valid": False, "error": INVALID})

            self.reject("漂移" if path != self.executable else "SHA-256", effect=changed)
            path.write_bytes(before)

    def test_source_token_drift_is_checked_even_after_invocation_timeout(self):
        token = {"format_version": 1, "kind": "frontend-origin-source-snapshot",
                 "source_count": len(SOURCES), "sha256": "0" * 64}
        changed = {**token, "sha256": "1" * 64}
        self.reject("漂移", effect=subprocess.TimeoutExpired(SECRET, 10),
                    source_effect=[token, token, changed])

    def test_source_token_drift_before_yield_prevents_url_invocation(self):
        token = {"format_version": 1, "kind": "frontend-origin-source-snapshot",
                 "source_count": len(SOURCES), "sha256": "0" * 64}
        changed = {**token, "sha256": "1" * 64}
        exchange, _ = self.reject("漂移", source_effect=[token, changed, changed])
        exchange.assert_not_called()

    def test_all_three_source_phases_use_same_guarded_tool_and_compare_exact_token(self):
        phases = []
        protected = False

        from contextlib import contextmanager

        @contextmanager
        def guard(_binding):
            nonlocal protected
            protected = True
            try:
                yield
            finally:
                protected = False

        def snapshot(path, metadata):
            self.assertTrue(protected)
            self.assertEqual(path, self.executable)
            self.assertEqual(metadata, self.metadata)
            phases.append(path)
            return {"format_version": 1, "kind": "frontend-origin-source-snapshot",
                    "source_count": len(SOURCES), "sha256": "0" * 64}

        with patch.object(tool, "protect_binary_invocation", side_effect=guard), \
                patch.object(tool, "_source_snapshot", side_effect=snapshot):
            with tool._verified_tool() as path:
                self.assertEqual(path, self.executable)
                self.assertEqual(len(phases), 2)
        self.assertEqual(len(phases), 3)

    def test_cancelled_initial_probe_still_checks_binding_without_launching_another_probe(self):
        with patch.object(tool, "protect_binary_invocation", return_value=nullcontext()), \
                patch.object(tool, "_source_snapshot", side_effect=KeyboardInterrupt()) as probe, \
                patch.object(tool, "_snapshot", wraps=tool._snapshot) as snapshot:
            with self.assertRaises(KeyboardInterrupt):
                with tool._verified_tool():
                    self.fail("取消后不得发布工具")
        self.assertEqual(probe.call_count, 1)
        self.assertEqual([call.args for call in snapshot.call_args_list], [(self.binding,), (self.binding,)])

    def test_identical_bytes_with_changed_file_identity_still_fail_closed(self):
        original = tool._state
        for selected in (self.binding, self.executable):
            changed = False

            def state(path):
                value = original(path)
                return (value[0], value[1] + 1, *value[2:]) if changed and path == selected else value

            def replace_identity(_path, _request):
                nonlocal changed
                changed = True
                return 1, encoded({"format_version": 1, "valid": False, "error": INVALID})

            with patch.object(tool, "_state", side_effect=state):
                self.reject("漂移", effect=replace_identity)

    def test_request_uses_exact_stdin_json_and_program_digest_is_rechecked(self):
        with patch.object(tool, "file_digest", wraps=tool.file_digest) as digest:
            exchange, _ = self.reject("地址为空")
        path, request = exchange.call_args.args
        self.assertEqual(path, self.executable)
        self.assertEqual(json.loads(request), {"format_version": 1, "value": "https://" + SECRET, "allow_empty": False})
        self.assertGreaterEqual(digest.call_count, 3)

    def test_types_are_rejected_locally_without_loading_binding(self):
        self.binding.unlink()
        for value, allow in ((None, True), ([], True), (1, True), ("", 1)):
            self.failure(lambda: tool.normalize_origin(value, allow_empty=allow), "必须")

    def test_invocation_exceptions_and_protection_failures_are_redacted(self):
        for error in (
            OSError(SECRET),
            RuntimeError(SECRET),
            subprocess.TimeoutExpired(SECRET, 10, output=SECRET, stderr=SECRET),
        ):
            self.reject("失败", effect=error)
        with patch.object(tool, "protect_binary_invocation", side_effect=ValueError(SECRET)):
            self.failure(lambda: tool.normalize_origin(SECRET, allow_empty=True), "保护")


class SourceSnapshotProtocolTests(unittest.TestCase):
    def setUp(self):
        self.sources = [{"path": name, "sha256": "0" * 64} for name in SOURCES]
        self.metadata = {"sources": self.sources}
        self.token = {"format_version": 1, "kind": "frontend-origin-source-snapshot",
                      "source_count": len(SOURCES), "sha256": "a" * 64}
        self.path = ROOT / ".local-tests/unit-protocol.exe"

    def test_source_request_is_bounded_exact_and_never_contains_file_content(self):
        with patch.object(tool, "_exchange", return_value=(0, encoded(self.token))) as exchange:
            self.assertEqual(tool._source_snapshot(self.path, self.metadata), self.token)
        path, request = exchange.call_args.args
        self.assertEqual(path, self.path)
        self.assertEqual(json.loads(request), {"format_version": 1, "root": str(tool.ROOT), "sources": self.sources})
        self.assertEqual(exchange.call_args.kwargs, {"operation": "verify-sources", "response_limit": tool.STDERR_LIMIT, "timeout": None})
        with patch.object(tool, "METADATA_LIMIT", 10), patch.object(tool, "_exchange") as exchange:
            with self.assertRaisesRegex(ValueError, "4 MiB"):
                tool._source_snapshot(self.path, self.metadata)
        exchange.assert_not_called()

    def test_source_response_rejects_unknown_duplicate_fields_types_counts_and_unsafe_errors(self):
        values = [(0, {**self.token, "source_count": True}), (0, {**self.token, "sha256": SECRET}),
                  (0, {**self.token, "format_version": True}), (0, {**self.token, "source_count": 0}),
                  (0, {**self.token, "extra": SECRET}), (1, self.token), (False, self.token),
                  (0, b'{"format_version":1,"format_version":1}'),
                  (2, {"format_version": 1, "kind": "frontend-origin-source-error", "category": SECRET, "path": SECRET}),
                  (2, {"format_version": 1, "kind": "frontend-origin-source-error", "category": "digest", "path": SECRET})]
        for code, value in values:
            raw = value if isinstance(value, bytes) else encoded(value)
            with patch.object(tool, "_exchange", return_value=(code, raw)), self.assertRaises(ValueError) as caught:
                tool._source_snapshot(self.path, self.metadata)
            self.assertNotIn(SECRET, str(caught.exception))

    def test_source_failure_keeps_known_source_path_fixed_category_and_repair_advice(self):
        path = str(ROOT / "Cargo.lock")
        response = {"format_version": 1, "kind": "frontend-origin-source-error", "category": "identity", "path": path}
        with patch.object(tool, "_exchange", return_value=(2, encoded(response))), self.assertRaises(ValueError) as caught:
            tool._source_snapshot(self.path, self.metadata)
        self.assertIn(path, str(caught.exception))
        self.assertIn("identity", str(caught.exception))
        self.assertIn("编译来源", str(caught.exception))
        self.assertIn("重新编译", str(caught.exception))


class ResponseProtocolTests(unittest.TestCase):
    def failure(self, code, value, *, allow_empty=True):
        raw = value if isinstance(value, bytes) else encoded(value)
        with self.assertRaises(ValueError) as caught:
            tool._response(code, raw, allow_empty=allow_empty, label="受管程序.exe")
        self.assertNotIn(SECRET, str(caught.exception))
        self.assertIsNone(caught.exception.__cause__)

    def test_success_codec_exact_fields_types_and_empty_policy(self):
        for origin in ("https://xn--fsqu00a.xn--0zwm56d", "http://[::1]:8080", ""):
            value = {"format_version": 1, "valid": True, "origin": origin}
            self.assertEqual(tool._response(0, encoded(value), allow_empty=True, label="协议夹具"), origin)
        self.failure(0, {"format_version": 1, "valid": True, "origin": ""}, allow_empty=False)

    def test_exit_status_unknown_errors_and_malformed_responses_do_not_leak(self):
        success = {"format_version": 1, "valid": True, "origin": "http://example.invalid"}
        failure = {"format_version": 1, "valid": False, "error": INVALID}
        for code, value in (
            (False, success),
            (0.0, success),
            (1, success),
            (0, failure),
            (2, failure),
            (-9, failure),
            (3, failure),
            (0, {**success, "extra": SECRET}),
            (0, {**success, "format_version": True}),
            (0, {**success, "valid": 1}),
            (0, {**success, "origin": []}),
            (0, {**success, "origin": "http://"}),
            (0, {**success, "origin": "http://example.invalid/"}),
            (0, {**success, "origin": "http://user:" + SECRET + "@host"}),
            (1, {**failure, "error": SECRET}),
            (1, {**failure, "error": []}),
            (1, {**failure, "extra": SECRET}),
            (0, b"\xff"),
            (0, b"[]"),
            (0, b'{"valid":true,"valid":false}'),
            (0, b'{"format_version":NaN}'),
            (0, b" " * (tool.METADATA_LIMIT + 1)),
        ):
            self.failure(code, value)

    def test_valid_origin_response_larger_than_64_kib_is_accepted(self):
        origin = "https://" + "label." * 14000 + "invalid"
        raw = encoded({"format_version": 1, "valid": True, "origin": origin})
        self.assertGreater(len(raw), 64 * 1024)
        self.assertEqual(tool._response(0, raw, allow_empty=False, label="长响应协议夹具"), origin)

    def test_response_boundary_and_overflow_are_checked_before_json_decode(self):
        value = {"format_version": 1, "valid": True, "origin": "http://example.invalid"}
        raw = encoded(value)
        with patch.object(tool, "RESPONSE_LIMIT", len(raw)):
            self.assertEqual(tool._response(0, raw, allow_empty=False, label="响应边界夹具"), value["origin"])
            with patch.object(tool, "json_object", side_effect=AssertionError("超限不应解析 JSON")):
                self.failure(0, raw + b" ")

    def test_all_rust_safe_error_categories_are_accepted_only_with_exit_one(self):
        for error, category in tool.SAFE_ERRORS.items():
            with self.assertRaisesRegex(ValueError, category):
                tool._response(
                    1,
                    encoded({"format_version": 1, "valid": False, "error": error}),
                    allow_empty=True,
                    label="受管程序.exe",
                )


class PipeFailureTests(unittest.TestCase):
    def process(self, *, stdout=b"", stderr=b"", timeout=False, code=1):
        process = MagicMock()
        process.__enter__.return_value = process
        process.stdin, process.stdout, process.stderr = io.BytesIO(), io.BytesIO(stdout), io.BytesIO(stderr)
        process.poll.return_value = None if timeout else code
        process.wait.side_effect = [subprocess.TimeoutExpired(SECRET, 10), 0] if timeout else [code, code]
        return process

    def test_timeout_terminates_tool_and_redacts_outputs(self):
        process = self.process(stderr=SECRET.encode(), timeout=True)
        with patch.object(tool.subprocess, "Popen", return_value=process) as launch:
            with self.assertRaisesRegex(ValueError, "超时") as caught:
                tool._exchange(Path("D:/fixture.exe"), b"{}")
        self.assertNotIn(SECRET, str(caught.exception))
        self.assertIsNone(caught.exception.__cause__)
        process.kill.assert_called()
        self.assertEqual(launch.call_args.args[0], [str(Path("D:/fixture.exe")), "__frontend-origin", "check"])
        if os.name == "nt":
            self.assertEqual(launch.call_args.kwargs["creationflags"], subprocess.CREATE_NO_WINDOW)

    def test_source_probe_has_no_new_total_deadline_and_cancellation_reaps_without_retry(self):
        process = self.process()
        process.wait.side_effect = [KeyboardInterrupt(), 0]
        with patch.object(tool.subprocess, "Popen", return_value=process) as launch:
            with self.assertRaises(KeyboardInterrupt):
                tool._exchange(ROOT / "fixture.exe", b"{}", operation="verify-sources", timeout=None)
        self.assertEqual(process.wait.call_args_list[0].kwargs, {"timeout": None})
        self.assertEqual(launch.call_count, 1)
        process.kill.assert_called_once()

    def test_stdout_and_stderr_limits_terminate_without_exposing_contents(self):
        for stream in ("stdout", "stderr"):
            limit = 128 * 1024 if stream == "stdout" else tool.STDERR_LIMIT
            process = self.process(**{stream: SECRET.encode() + b"x" * limit})
            with (
                patch.object(tool, "RESPONSE_LIMIT", 128 * 1024),
                patch.object(tool.subprocess, "Popen", return_value=process),
            ):
                with self.assertRaisesRegex(ValueError, "64 KiB") as caught:
                    tool._exchange(Path("D:/fixture.exe"), b"{}")
            self.assertNotIn(SECRET, str(caught.exception))
            process.kill.assert_called()

    def test_long_response_and_stderr_at_their_limits_are_drained_without_kill(self):
        origin = "https://" + "label." * 14000 + "invalid"
        raw = encoded({"format_version": 1, "valid": True, "origin": origin})
        limit = 128 * 1024
        raw += b" " * (limit - len(raw))
        process = self.process(stdout=raw, stderr=b"x" * tool.STDERR_LIMIT, code=0)
        with patch.object(tool, "RESPONSE_LIMIT", limit), patch.object(tool.subprocess, "Popen", return_value=process):
            code, received = tool._exchange(Path("D:/fixture.exe"), b"{}")
            self.assertEqual(tool._response(code, received, allow_empty=False, label="管道协议夹具"), origin)
        self.assertEqual(len(received), limit)
        process.kill.assert_not_called()


class NativeBindingTests(unittest.TestCase):
    def test_real_managed_binding_normalizes_origin_without_skip_or_mock(self):
        self.assertEqual(
            tool.normalize_origin("\ufeffHTTPS://EXAMPLE.INVALID:443/?#\ufeff", allow_empty=False),
            "https://example.invalid",
        )
        self.assertEqual(tool.normalize_origin("\ufeff", allow_empty=True), "")


if __name__ == "__main__":
    unittest.main()
