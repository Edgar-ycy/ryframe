"""来源验收工具 staging 的 Git 来源、create-only 与完整集合回归。"""

import os
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import restore_source_runtime_producer as producer
import restore_source_runtime_staging as staging
import source_fingerprints
from restore_reference_plan import configuration_binding, validate_configuration_binding
from test_source_fingerprints import inventory
from workspace_directory import WorkspaceDirectory


class SourceRuntimeStagingTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.head = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.backend, text=True
        ).strip()
        value = inventory()
        value["source"]["snapshot"].update(clean=True, head=self.head)
        self.source = source_fingerprints.execution_source(value)
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/python-unit", prefix="source-staging-"
        )
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name) / "verification"
        self.directory.mkdir()
        self.tool = {"path": str(Path(sys.executable).resolve()),
                     "sha256": producer.artifact_snapshot(Path(sys.executable).resolve()).sha256}

    def test_git_objects_define_exact_runtime_closure_and_create_only_manifest(self):
        descriptor = staging.create_tool_staging(
            self.backend, self.backend, self.directory, self.source, self.head, self.tool
        )
        verified = staging.verify_tool_staging(
            self.backend, self.backend, self.directory, descriptor, self.source, self.head, self.tool
        )
        self.assertEqual(len(verified["manifest"]["runtime_modules"]), 1)
        self.assertEqual(verified["manifest"]["runtime_modules"][0]["path"], self.tool["path"])
        self.assertEqual(
            [(row["origin"], row["path"]) for row in verified["manifest"]["files"]],
            [("execution", str((verified["root"] / staging.CONTRACT).resolve()))],
        )

    def test_mutation_unknown_file_and_second_creation_fail_closed(self):
        descriptor = staging.create_tool_staging(
            self.backend, self.backend, self.directory, self.source, self.head, self.tool
        )
        root = self.directory / staging.DIRECTORY
        entry = root / staging.CONTRACT
        original = entry.read_bytes()
        entry.write_bytes(b"throw new Error('B')\n")
        with self.assertRaisesRegex(ValueError, "Git blob"):
            staging.verify_tool_staging(
                self.backend, self.backend, self.directory, descriptor, self.source, self.head, self.tool
            )
        entry.write_bytes(original)
        staging.verify_tool_staging(
            self.backend, self.backend, self.directory, descriptor, self.source, self.head, self.tool
        )
        unknown = root / "unknown.mjs"
        unknown.write_text("export default 'unknown'\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "缺失或未知"):
            staging.verify_tool_staging(
                self.backend, self.backend, self.directory, descriptor, self.source, self.head, self.tool
            )
        unknown.unlink()
        with self.assertRaisesRegex(ValueError, "create-only"):
            staging.create_tool_staging(
                self.backend, self.backend, self.directory, self.source, self.head, self.tool
            )

    def test_native_environment_contains_no_application_secret(self):
        value = producer._native_environment(
            {
                "APP_ENV": "test",
                "RYFRAME_PASSWORD": "secret",
            }
        )
        self.assertEqual(value["APP_ENV"], "test")
        self.assertNotIn("RYFRAME_PASSWORD", value)

    def test_configuration_binding_contains_only_path_and_digest(self):
        directory = self.directory.parent / "private-config"
        directory.mkdir()
        secrets = (
            "private-admin-password",
            "private-access-key",
            "private-secret-key",
            "private-session-token",
        )
        (directory / "app.toml").write_text("[application]\nname='unit'\n", encoding="utf-8")
        (directory / "app.test.toml").write_text(
            "[bootstrap]\nadmin_password='private-admin-password'\n"
            "[object_storage]\naccess_key='private-access-key'\n"
            "secret_key='private-secret-key'\nsession_token='private-session-token'\n",
            encoding="utf-8",
        )
        value = configuration_binding(self.backend, directory, "test")
        encoded = json.dumps(value, ensure_ascii=False, sort_keys=True)
        self.assertEqual(set(value), {"app_env", "directory", "files", "sha256"})
        self.assertEqual(set(value["files"]), {"app.toml", "app.test.toml"})
        for secret in secrets:
            self.assertNotIn(secret, encoded)
        invalid = {**value, "secret_key": "private-secret-key"}
        with self.assertRaisesRegex(ValueError, "无凭据文件绑定") as failure:
            validate_configuration_binding(invalid, self.backend)
        self.assertNotIn("private-secret-key", str(failure.exception))

    def test_git_blob_reads_ignore_inherited_repository_and_replace_overrides(self):
        with patch.dict(
            os.environ,
            {
                "GIT_DIR": "outside",
                "GIT_OBJECT_DIRECTORY": "outside-objects",
                "GIT_REPLACE_REF_BASE": "refs/replace/hostile",
            },
        ), patch.object(staging.subprocess, "check_output", return_value=b"result") as checked:
            self.assertEqual(staging._git(self.backend, "cat-file", "blob", "a" * 40), b"result")
        environment = checked.call_args.kwargs["env"]
        self.assertEqual(environment["GIT_OPTIONAL_LOCKS"], "0")
        self.assertEqual(environment["GIT_NO_REPLACE_OBJECTS"], "1")
        permitted = {"GIT_OPTIONAL_LOCKS", "GIT_NO_REPLACE_OBJECTS"}
        self.assertFalse(any(key.upper().startswith("GIT_") for key in environment if key not in permitted))

if __name__ == "__main__":
    unittest.main()
