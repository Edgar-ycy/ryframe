"""前端 v3 文件来源的反向契约场景；测试存在不代表已执行通过。"""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import unittest
from unittest.mock import patch

from frontend_origin_tool import normalize_origin
from workspace_directory import WorkspaceDirectory
from frontend_receipt_fixture import APPLICATION, frontend_context, prepare_frontend, save_receipt, write_json
from restore_frontend_build import build_registered_frontend, validate_frontend_build, validate_registered_frontend
from restore_frontend_configuration import verify_frontend_inputs
from restore_frontend_context import IDENTITY, validate_frontend_build_context
from restore_frontend_paths import ordinary
from restore_runtime_evidence import read_json_document, validate_frontend_receipt
from source_inventory import build_source_domains, capture_inventory

ROOT = Path(__file__).resolve().parents[2]
SECRET = "PRIVATE-SENTINEL-DO-NOT-PRINT"


class FrontendConfigurationTests(unittest.TestCase):
    def setUp(self):
        # 真实文件夹具通过正式原生适配完成配置核验。
        temporary = WorkspaceDirectory(ROOT / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "中文 空格"
        self.head = prepare_frontend(self.root)
        self.build = frontend_context(self.root)

    def safe_failure(self, call, text=None):
        with self.assertRaises(ValueError) as caught:
            call()
        message = str(caught.exception)
        self.assertNotIn(SECRET, message)
        self.assertIsNone(caught.exception.__cause__)
        if text:
            self.assertIn(text, message)
        return caught.exception

    def test_three_environments_keep_saved_selection_independent_of_shell_and_mode(self):
        for relative in (".env", ".env.local", ".env.production", ".env.production.local"):
            (self.root / relative).write_text(f"VITE_APP_API_ORIGIN={SECRET}\n", encoding="utf-8")
        for environment in ("dev", "test", "prod"):
            with self.subTest(environment=environment):
                build = frontend_context(self.root, environment)
                with patch.dict(os.environ, {"APP_ENV": "INVALID", "VITE_APP_API_ORIGIN": SECRET}):
                    inputs = verify_frontend_inputs(self.root, build)
                self.assertEqual(inputs.environment, environment)
                self.assertEqual(build["mode"], "production")
                self.assertEqual(
                    [item["path"] for item in inputs.configuration["files"]],
                    ["config/app.toml", f"config/app.{environment}.toml"],
                )

    def test_optional_common_and_required_selected_complete_toml(self):
        (self.root / "config/app.toml").unlink()
        selected = self.root / "config/app.dev.toml"
        selected.write_text(APPLICATION, encoding="utf-8")
        build = frontend_context(self.root)
        self.assertEqual(len(build["configuration"]["files"]), 1)
        verify_frontend_inputs(self.root, build)
        selected.unlink()
        self.safe_failure(lambda: verify_frontend_inputs(self.root, build), "必需文件")

    def test_optional_managed_json_can_have_missing_parent_directories(self):
        (self.root / IDENTITY).unlink()
        (self.root / ".local-tests/build").rmdir()
        (self.root / ".local-tests").rmdir()
        write_json(self.root / "dist/build-identity.json", {"frontend_commit": "development"})
        save_receipt(self.root)
        validate_frontend_build(self.root)
        build = frontend_context(self.root)
        self.assertIsNone(build["identity"]["source_file"])
        self.assertEqual(
            build["configuration"]["tools"],
            [item for item in self.build["configuration"]["tools"] if item["path"] == "config/tools.json"],
        )

    def test_missing_ancestor_is_not_optional_common(self):
        (self.root / ".local-tests/frontend-tools.json").write_text(
            '{"application_config_dir":"missing/child"}', encoding="utf-8"
        )
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "父目录")

    def test_local_tool_priority_unicode_directory_and_absolute_source(self):
        directory = self.root.parent / "外部 公开目录"
        directory.mkdir()
        (directory / "app.test.toml").write_text(APPLICATION, encoding="utf-8")
        write_json(self.root / "config/tools.json", {"application_config_dir": "unused-public"})
        write_json(self.root / ".local-tests/frontend-tools.json", {"application_config_dir": str(directory)})
        build = frontend_context(self.root, "test")
        self.assertEqual(build["configuration"]["directory"], directory.as_posix())
        self.assertEqual(
            [item["path"] for item in build["configuration"]["tools"]],
            ["config/tools.json", ".local-tests/frontend-tools.json"],
        )
        verify_frontend_inputs(self.root, build)

    def test_each_input_hash_and_optional_file_set_drift_is_rejected(self):
        save_receipt(self.root)
        for relative in (
            "config/app.toml",
            "config/app.dev.toml",
            "config/tools.json",
            IDENTITY,
            "dist/build-identity.json",
        ):
            file = self.root / relative
            original = file.read_bytes()
            file.write_bytes(original + b"\n")
            with self.subTest(relative=relative):
                self.safe_failure(lambda: validate_frontend_build(self.root), "不同")
            file.write_bytes(original)
        write_json(self.root / ".local-tests/frontend-tools.json", {})
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "不同")

    def test_before_after_detects_byte_change_even_without_semantic_change(self):
        inputs = verify_frontend_inputs(self.root, self.build)
        with (self.root / "config/app.dev.toml").open("a", encoding="utf-8") as file:
            file.write("# 漂移\n")
        self.safe_failure(inputs.assert_unchanged, "漂移")

    def test_toml_duplicate_unknown_private_and_utf8_errors_do_not_leak(self):
        selected = self.root / "config/app.dev.toml"
        malformed = [
            f'password="{SECRET}"\n',
            f'api_origin=""\napi_origin="{SECRET}"\n',
            f'[server]\n[server]\npassword="{SECRET}"\n',
            f'api_origin="{SECRET}',
            f'__proto__="{SECRET}"\n',
        ]
        for text in malformed:
            selected.write_text(text, encoding="utf-8")
            with self.subTest(kind=text.split("=")[0]):
                self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build))
        selected.write_bytes(SECRET.encode() + b"\xff")
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "UTF8")

    def test_types_missing_fields_special_numbers_and_invalid_origin_are_rejected(self):
        selected = self.root / "config/app.dev.toml"
        for value in (
            "api_origin=[]",
            "server=[]",
            "[server]\nport=true",
            "[server]\nport=0",
            "[server]\nport=65536",
            "[server]\nport=nan",
            "[server]\nport=inf",
            "[server]\nport=99999999999999999999999999999",
            "[server]\nport=2026-10-22",
            '[server]\nhost=""',
            '[server]\nhost="\\u0001"',
            '[server]\nproxy_target="ftp://invalid"',
            'api_origin="http://example.invalid/path"',
            f'api_origin="http://user:{SECRET}@example.invalid"',
        ):
            selected.write_text(value, encoding="utf-8")
            with self.subTest(field=value.split("=")[0]):
                self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build))
        (self.root / "config/app.toml").unlink()
        selected.write_text('api_origin=""', encoding="utf-8")
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "缺少 api_origin 或 server.host/port/proxy_target")

    def test_integer_valued_toml_float_port_matches_frontend_number_is_integer(self):
        (self.root / "config/app.dev.toml").write_text("[server]\nport=5173.0\n", encoding="utf-8")
        verify_frontend_inputs(self.root, frontend_context(self.root))

    def test_real_origin_cannot_be_hidden_by_shell_override(self):
        (self.root / "config/app.dev.toml").write_text('api_origin="https://example.invalid"', encoding="utf-8")
        with patch.dict(os.environ, {"VITE_APP_API_ORIGIN": ""}):
            self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "api_origin 为空")

    def test_tools_and_identity_json_are_strict_safe_and_not_toml(self):
        tool = self.root / "config/tools.json"
        write_json(tool, {"api_contract": {"private": SECRET}, "real_browser": SECRET,
                          "browser_smoke": [SECRET]})
        inputs = verify_frontend_inputs(self.root, frontend_context(self.root))
        self.assertNotIn(SECRET, json.dumps(inputs.observed))
        self.assertNotIn(SECRET, json.dumps(inputs.configuration))
        inputs.assert_unchanged()
        write_json(tool, {})
        for relative in (".local-tests/frontend-tools.json", IDENTITY):
            file = self.root / relative
            original = file.read_bytes() if file.exists() else None
            for raw in (
                b"\xff",
                f'{{"password":"{SECRET}"}}'.encode(),
                f'{{"password":"{SECRET}","password":1}}'.encode(),
                b"[]",
                b'{"application_config_dir":NaN}',
            ):
                file.write_bytes(raw)
                with self.subTest(relative=relative, kind=raw[:1]):
                    self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build))
            if original is None:
                file.unlink()
            else:
                file.write_bytes(original)

    def test_dist_identity_and_registered_source_sha_must_match(self):
        save_receipt(self.root)
        validate_registered_frontend(self.root, self.head)
        self.safe_failure(lambda: validate_registered_frontend(self.root, "0" * 40), "干净")
        write_json(self.root / "dist/build-identity.json", {"frontend_commit": "0" * 40})
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "身份")

    def test_source_identity_accepts_json_number_one_point_zero_with_real_fixture(self):
        source = self.root / IDENTITY
        write_json(source, {"format_version": 1.0, "frontend_commit": self.head})
        self.assertIn(b'"format_version": 1.0', source.read_bytes())
        build = frontend_context(self.root)
        inputs = verify_frontend_inputs(self.root, build, expected_head=self.head)
        self.assertEqual(inputs.identity["frontend_commit"], self.head)
        self.assertEqual(inputs.identity["source_file"], build["identity"]["source_file"])
        save_receipt(self.root)
        validate_registered_frontend(self.root, self.head)

    def test_source_identity_rejects_bool_nonintegral_nonfinite_and_other_versions(self):
        for version in (True, False, 1.5, float("nan"), float("inf"), float("-inf"), 0, 2, "1"):
            write_json(self.root / IDENTITY, {"format_version": version, "frontend_commit": self.head})
            with self.subTest(kind=type(version).__name__):
                self.safe_failure(lambda: frontend_context(self.root))

    def test_external_build_rejects_wrong_sha_and_legacy_layout_without_invocation(self):
        forbidden = unittest.mock.Mock(side_effect=AssertionError("禁止执行外部构建"))
        self.safe_failure(lambda: build_registered_frontend(self.root, self.root, "0" * 40, forbidden), "干净")
        file = self.root / "scripts/application-config.mjs"
        file.unlink()
        subprocess.check_call(["git", "-C", str(self.root), "add", "-u"], stdout=subprocess.DEVNULL)
        subprocess.check_call(
            [
                "git",
                "-C",
                str(self.root),
                "-c",
                "user.name=RyFrame fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "legacy layout",
            ]
        )
        head = subprocess.check_output(["git", "-C", str(self.root), "rev-parse", "HEAD"]).decode().strip()
        self.safe_failure(lambda: build_registered_frontend(self.root, self.root, head, forbidden), "历史 B0")
        forbidden.assert_not_called()

    def test_generic_development_is_allowed_but_not_registered_formal(self):
        write_json(self.root / IDENTITY, {"format_version": 1, "frontend_commit": "development"})
        write_json(self.root / "dist/build-identity.json", {"frontend_commit": "development"})
        save_receipt(self.root)
        validate_frontend_build(self.root)
        self.safe_failure(lambda: validate_registered_frontend(self.root, self.head), "身份")
        (self.root / IDENTITY).unlink()
        save_receipt(self.root)
        validate_frontend_build(self.root)

    def test_old_v2_unknown_fields_and_wrong_source_order_fail_closed(self):
        file = save_receipt(self.root)
        receipt = read_json_document(file).value
        old = copy.deepcopy(receipt)
        old["format_version"] = 2
        self.safe_failure(lambda: validate_frontend_receipt(old), "v3")
        for mutate in (
            lambda value: value.update(environment_files=[]),
            lambda value: value["configuration"]["files"].reverse(),
            lambda value: value["configuration"].update(environment="production"),
            lambda value: value["identity"].update(frontend_commit="A" * 40),
            lambda value: value["toolchain"]["pnpm"].update(observed="other"),
        ):
            build = copy.deepcopy(self.build)
            mutate(build)
            self.safe_failure(lambda: validate_frontend_build_context(build))

    def test_v3_receipt_float_version_and_integer_byte_numbers_use_real_artifacts(self):
        artifact = self.root / "dist/assets/one-byte.bin"
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_bytes(b"x")
        file = save_receipt(self.root)
        receipt = read_json_document(file).value
        receipt["format_version"] = 3.0
        for item in receipt["files"]:
            item["bytes"] = float(item["bytes"])
        write_json(file, receipt)
        self.assertIn(b'"format_version": 3.0', file.read_bytes())
        self.assertIn(b'"bytes": 1.0', file.read_bytes())
        validate_frontend_build(self.root)
        validate_registered_frontend(self.root, self.head)

    def test_v3_receipt_invalid_numeric_types_ranges_and_nonfinite_values_are_rejected(self):
        file = save_receipt(self.root)
        original = read_json_document(file).value
        for field, values in (
            ("format_version", (True, False, 3.5, float("nan"), float("inf"), 2, 4, "3")),
            (
                "bytes",
                (
                    True,
                    False,
                    -1,
                    1.5,
                    9007199254740992,
                    float(9007199254740992),
                    float("nan"),
                    float("inf"),
                    float("-inf"),
                    "1",
                ),
            ),
        ):
            for number in values:
                value = copy.deepcopy(original)
                if field == "bytes":
                    value["files"][0][field] = number
                else:
                    value[field] = number
                write_json(file, value)
                with self.subTest(field=field, kind=type(number).__name__):
                    self.safe_failure(lambda: validate_frontend_receipt(read_json_document(file).value))

    def test_v3_receipt_safe_integer_schema_accepts_zero_and_inclusive_upper_boundary(self):
        file = save_receipt(self.root)
        receipt = read_json_document(file).value
        for number in (0, -0.0, 9007199254740991, float(9007199254740991)):
            receipt["files"][0]["bytes"] = number
            write_json(file, receipt)
            value = validate_frontend_receipt(read_json_document(file).value)
            self.assertEqual(value["files"][0]["bytes"], number)

    def test_readonly_validation_preserves_all_file_bytes(self):
        save_receipt(self.root)
        files = {
            path: path.read_bytes() for path in self.root.rglob("*") if path.is_file() and ".git" not in path.parts
        }
        validate_frontend_build(self.root)
        self.assertEqual(files, {path: path.read_bytes() for path in files})

    def test_link_dotdot_dangling_directory_and_reparse_are_rejected(self):
        file = self.root / "config/app.dev.toml"
        file.unlink()
        file.mkdir()
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "错误类型")
        file.rmdir()
        # 未知 reparse tag 不要求管理员构造；附加属性协议以窄 lstat 适配器检查。
        original_lstat = os.lstat

        def reparse(path, *args, **kwargs):
            result = original_lstat(path, *args, **kwargs)
            if Path(path).name == "config":
                from types import SimpleNamespace

                return SimpleNamespace(st_mode=stat.S_IFDIR, st_file_attributes=0x400)
            return result

        with patch("restore_frontend_paths.os.lstat", side_effect=reparse):
            self.safe_failure(lambda: ordinary(str(self.root / "config/../config"), directory=True), "重解析")
        # 不跳过权限错误或悬空链接；平台不允许创建链接时测试直接失败，由主控处理环境。
        file.symlink_to(self.root / "missing-common-target.toml")
        self.addCleanup(file.unlink)
        self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build), "链接")
        link = self.root / "linked"
        link.symlink_to(self.root / "config", target_is_directory=True)
        self.addCleanup(link.unlink)
        self.safe_failure(lambda: ordinary(str(link) + "/../config", directory=True), "链接")

    def test_source_domains_classify_tools_json_and_toml_like_frontend(self):
        domains = build_source_domains(capture_inventory(self.root), "frontend")
        self.assertIn("config/tools.json", domains["tools"]["files"])
        self.assertNotIn("config/tools.json", domains["product"]["frontend"]["files"])
        self.assertIn("config/app.toml", domains["product"]["frontend"]["files"])

    def test_full_dist_unicode_sort_links_and_artifact_changes_remain_bound(self):
        for relative in ("assets/z.txt", "assets/中文.txt", "a.txt", "assets/😀.txt"):
            file = self.root / "dist" / relative
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text(relative, encoding="utf-8")
        receipt_file = save_receipt(self.root)
        receipt = read_json_document(receipt_file).value
        names = [item["path"] for item in receipt["files"]]
        self.assertEqual(names, sorted(names, key=lambda name: name.encode("utf-8")))
        file = self.root / "dist/assets/中文.txt"
        file.write_bytes(b"changed")
        self.safe_failure(lambda: validate_frontend_build(self.root), "不同")
        file.unlink()
        file.symlink_to(self.root / "config/app.toml")
        self.addCleanup(file.unlink)
        self.safe_failure(lambda: validate_frontend_build(self.root))

    def test_dist_required_files_and_added_file_during_verification_are_rejected(self):
        import restore_frontend_build

        save_receipt(self.root)
        original = restore_frontend_build.artifact_snapshot
        added = self.root / "dist/new.txt"

        def read_then_add(path):
            result = original(path)
            added.write_text("new", encoding="utf-8")
            return result

        with patch.object(restore_frontend_build, "artifact_snapshot", side_effect=read_then_add):
            self.safe_failure(lambda: restore_frontend_build.frontend_snapshots(self.root), "文件集合")
        added.unlink()
        (self.root / "dist/.vite/manifest.json").unlink()
        self.safe_failure(lambda: validate_frontend_build(self.root), "manifest.json")

    def test_io_errors_and_changes_between_two_reads_fail_closed(self):
        from restore_frontend_paths import read_bytes

        with patch("restore_frontend_paths.os.lstat", side_effect=PermissionError(13, SECRET)):
            self.safe_failure(lambda: read_bytes(str(self.root / "config/app.toml"), optional=True), "IO code 13")
        write_json(self.root / ".local-tests/frontend-tools.json", {})
        observed = verify_frontend_inputs(self.root, frontend_context(self.root))
        (self.root / ".local-tests/frontend-tools.json").unlink()
        self.safe_failure(observed.assert_unchanged, "漂移")

    def test_contract_entry_executes_validator_and_redacts_private_toml(self):
        save_receipt(self.root)
        inventory = capture_inventory(self.root)
        request = json.dumps({"frontend_inventories": [inventory], "backend_inventories": []})
        from rate_limit_tool import tool_binding, verified_tool

        binding = tool_binding()
        command = [binding["path"], "__frontend-build-contract", "--frontend-dir", str(self.root)]
        options = {
            "input": request,
            "text": True,
            "encoding": "utf-8",
            "capture_output": True,
            "env": {**os.environ, "APP_ENV": "prod", "VITE_APP_API_ORIGIN": SECRET},
        }
        with verified_tool(binding):
            result = subprocess.run(command, **options)
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value["frontend_build"]["configuration"], self.build["configuration"])
        self.assertNotIn("environment_files", value)
        (self.root / "config/app.dev.toml").write_text(f'password="{SECRET}"', encoding="utf-8")
        with verified_tool(binding):
            failure = subprocess.run(command, **options)
        self.assertNotEqual(failure.returncode, 0)
        self.assertEqual(failure.stdout, "")
        self.assertNotIn(SECRET, failure.stderr)
        self.assertIn("FrontendBuildRejected", failure.stderr)
        self.assertIn("重新构建", failure.stderr)

    def test_rust_origin_normalization_preserves_frontend_url_semantics(self):
        for value, expected in (
            ("http://127.1", "http://127.0.0.1"),
            ("http://0x7f000001", "http://127.0.0.1"),
            ("http://example.invalid/?#", "http://example.invalid"),
            ("http://:@example.invalid/", "http://example.invalid"),
            ("https://例子.测试", "https://xn--fsqu00a.xn--0zwm56d"),
            ("\ufeff", ""),
        ):
            with self.subTest(expected=expected):
                self.assertEqual(normalize_origin(value, allow_empty=True), expected)
        for value in ("http://999.999.999.999", "http://256.0.0.1", "\u0085"):
            self.safe_failure(lambda: normalize_origin(value, allow_empty=True))

    def test_ecmascript_host_and_same_origin_whitespace_remain_distinct(self):
        selected = self.root / "config/app.dev.toml"
        selected.write_text('api_origin="\\ufeff"\n[server]\nhost="\\ufefflocalhost\\ufeff"\n', encoding="utf-8")
        verify_frontend_inputs(self.root, frontend_context(self.root))
        for value in ('api_origin="\\u0085"', '[server]\nhost="\\ufeff"', '[server]\nhost="\\u0085"'):
            selected.write_text(value, encoding="utf-8")
            with self.subTest(field=value.split("=")[0]):
                self.safe_failure(lambda: verify_frontend_inputs(self.root, self.build))


if __name__ == "__main__":
    unittest.main()
