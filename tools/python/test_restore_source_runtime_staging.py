"""来源验收工具 staging 的 Git 来源、create-only 与完整集合回归。"""

import os
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
from test_source_fingerprints import inventory
from workspace_directory import WorkspaceDirectory


MODULES = [
    "tools/js/config.mjs",
    "tools/js/failure.mjs",
    "tools/js/pacing-model.mjs",
    "tools/js/request.mjs",
    "tools/js/restore_reference_existing.mjs",
    "tools/js/restore_reference_pacing.mjs",
    "tools/js/restore_source_existing.mjs",
]


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

    def test_git_objects_define_exact_runtime_closure_and_create_only_manifest(self):
        descriptor = staging.create_tool_staging(
            self.backend, self.backend, self.directory, self.source, self.head
        )
        verified = staging.verify_tool_staging(
            self.backend, self.backend, self.directory, descriptor, self.source, self.head
        )
        self.assertEqual(verified["manifest"]["runtime_modules"], MODULES)
        self.assertEqual(
            [(row["origin"], row["path"]) for row in verified["manifest"]["files"]],
            [("execution", staging.CONTRACT), *[("coordinator", path) for path in MODULES]],
        )

    def test_mutation_unknown_file_and_second_creation_fail_closed(self):
        descriptor = staging.create_tool_staging(
            self.backend, self.backend, self.directory, self.source, self.head
        )
        root = self.directory / staging.DIRECTORY
        entry = root / staging.ENTRY
        original = entry.read_bytes()
        entry.write_bytes(b"throw new Error('B')\n")
        with self.assertRaisesRegex(ValueError, "Git blob"):
            staging.verify_tool_staging(
                self.backend, self.backend, self.directory, descriptor, self.source, self.head
            )
        entry.write_bytes(original)
        staging.verify_tool_staging(
            self.backend, self.backend, self.directory, descriptor, self.source, self.head
        )
        unknown = root / "unknown.mjs"
        unknown.write_text("export default 'unknown'\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "缺失或未知"):
            staging.verify_tool_staging(
                self.backend, self.backend, self.directory, descriptor, self.source, self.head
            )
        unknown.unlink()
        with self.assertRaisesRegex(ValueError, "create-only"):
            staging.create_tool_staging(
                self.backend, self.backend, self.directory, self.source, self.head
            )

    def test_node_environment_cannot_load_or_write_code_outside_staging(self):
        value = producer._node_environment(
            {
                "PATH": "fixed",
                "NODE_OPTIONS": "--require=outside.js",
                "node_path": "outside",
                "Node_Compile_Cache": "outside",
                "NODE_V8_COVERAGE": "outside",
                "RYFRAME_PASSWORD": "secret",
            }
        )
        self.assertEqual(value, {"PATH": "fixed", "RYFRAME_PASSWORD": "secret"})

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

    def test_runtime_closure_rejects_dynamic_and_external_code_loading(self):
        for content in (
            b"await import('./later.mjs')\n",
            b"import dependency from 'external-package'\n",
            b"import { value }\nfrom './later.mjs'\n",
        ):
            with self.subTest(content=content), self.assertRaisesRegex(ValueError, "动态|外部|import"):
                staging._runtime_modules({staging.ENTRY: content})


if __name__ == "__main__":
    unittest.main()
