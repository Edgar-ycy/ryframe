"""successor arm 请求只能由绑定事实生成，并以完整文件一次性发布。"""

from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
from devex_clone_capture import write_json
from devex_clone_run_state import binding
import reference_fixture_successor as successor
import reference_fixture_successor_arm as arm


class SuccessorArmRequestTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name)
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.successor = self.file("successor.json", {"relationship": True})
        self.source = self.file("source-result.json", {"source": True})
        self.export_result = self.file("source-export-result.json", {"published": True})
        self.exported = self.file("export.json", {"export": True})
        export_check = patch.object(arm, "published_export", return_value={"export": binding(self.exported)})
        self.addCleanup(export_check.stop)
        self.export_check = export_check.start()
        self.bridge = self.file("build-bridge.json", {"bridge": True})
        self.environment = self.file("target-environment.json", {"environment": {}})
        self.workspace = self.local / "fresh target"
        (self.workspace / "target").mkdir(parents=True)
        self.registration = self.file_at(
            self.workspace / "registration.json", {"environment": binding(self.environment)}
        )
        self.initialized = self.file_at(
            self.workspace / "target/initialized.json", {"initialized": True}
        )
        self.initialized_files = self.file_at(
            self.workspace / "initialized-files.json", {"files": []}
        )
        self.published = {
            "source_generation": binding(self.file("source-generation.json", {"v2": True})),
            "review_successor": {"source_result": binding(self.source)},
            "manifest": {"build_bridges": [binding(self.bridge)]},
        }

    def file(self, name: str, value: dict) -> Path:
        return self.file_at(self.local / name, value)

    @staticmethod
    def file_at(path: Path, value: dict) -> Path:
        write_json(path, value)
        return path

    def test_build_derives_every_descriptor_and_runs_copy_validator(self):
        copy_directory = self.local / "copy base"
        with (
            patch(
                "reference_fixture_successor.published_source",
                return_value=self.published,
            ) as published,
            patch.object(
                arm, "validate_arm_request", return_value={"target_side": "base"}
            ) as validate,
        ):
            request = arm.build(
                self.backend,
                self.successor,
                self.workspace,
                "successor-base-r23",
                "base",
                copy_directory,
                source_export_result_path=self.export_result,
            )
        self.assertEqual(published.call_count, 2)
        self.assertEqual(request["source_registration"], binding(self.source))
        self.assertEqual(request["review_successor"], binding(self.successor))
        self.assertEqual(request["source_export"], binding(self.exported))
        self.assertEqual(request["source_export_result"], binding(self.export_result))
        self.export_check.assert_called_once_with(self.backend, binding(self.export_result), self.published)
        self.assertEqual(request["target_registration"], binding(self.registration))
        self.assertEqual(request["initialized"], binding(self.initialized))
        self.assertEqual(request["target_initialized_files"], binding(self.initialized_files))
        self.assertEqual(request["target_environment"], binding(self.environment))
        self.assertEqual(request["build_bridges"], [binding(self.bridge)])
        self.assertEqual(request["copy_directory"], str(copy_directory))
        validate.assert_called_once_with(
            self.backend,
            request,
            live_storage=False,
        )

    def test_build_rejects_workspace_side_mismatch(self):
        with (
            patch(
                "reference_fixture_successor.published_source",
                return_value=self.published,
            ),
            patch.object(
                arm, "validate_arm_request", return_value={"target_side": "candidate"}
            ),
            self.assertRaisesRegex(ValueError, "workspace 与请求 side"),
        ):
            arm.build(
                self.backend,
                self.successor,
                self.workspace,
                "successor-base-r23",
                "base",
                self.local / "copy-base",
                source_export_result_path=self.export_result,
            )

    def test_publish_rechecks_before_and_after_atomic_publication(self):
        request = {"kind": "request"}
        output = self.local / "arm-request.json"
        with patch.object(arm, "build", side_effect=[request, request, request]) as build:
            self.assertEqual(
                arm.publish(
                    self.backend,
                    self.successor,
                    self.workspace,
                    "successor-base-r23",
                    "base",
                    self.local / "copy-base",
                    output,
                    source_export_result_path=self.export_result,
                ),
                request,
            )
        self.assertEqual(build.call_count, 3)
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), request)
        self.assertFalse(any(path.name.endswith(".pending") for path in self.local.iterdir()))

    def test_publish_preserves_post_write_evidence_when_inputs_change(self):
        request = {"kind": "request"}
        changed = {"kind": "changed"}
        output = self.local / "arm-request.json"
        with (
            patch.object(arm, "build", side_effect=[request, request, changed]),
            self.assertRaisesRegex(ValueError, "保留文件"),
        ):
            arm.publish(
                self.backend,
                self.successor,
                self.workspace,
                "successor-base-r23",
                "base",
                self.local / "copy-base",
                output,
                source_export_result_path=self.export_result,
            )
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), request)

    def test_publish_rejects_pre_write_drift_without_output(self):
        request = {"kind": "request"}
        output = self.local / "arm-request.json"
        with (
            patch.object(arm, "build", side_effect=[request, {"kind": "changed"}]),
            self.assertRaisesRegex(ValueError, "发布前"),
        ):
            arm.publish(
                self.backend,
                self.successor,
                self.workspace,
                "successor-base-r23",
                "base",
                self.local / "copy-base",
                output,
                source_export_result_path=self.export_result,
            )
        self.assertFalse(output.exists())

    def test_atomic_failure_removes_pending_without_creating_target(self):
        output = self.local / "arm-request.json"
        with (
            patch.object(os, "link", side_effect=FileExistsError("collision")),
            self.assertRaises(FileExistsError),
        ):
            arm._publish_json(output, {"kind": "request"})
        self.assertFalse(output.exists())
        self.assertFalse(any(path.name.endswith(".pending") for path in self.local.iterdir()))

    def test_cli_defaults_to_read_only_and_rejects_unpaired_output(self):
        arguments = [
            "reference_fixture_successor.py",
            "arm-request",
            "--backend-dir",
            str(self.backend),
            "--successor",
            str(self.successor),
            "--source-export-result",
            str(self.export_result),
            "--workspace",
            str(self.workspace),
            "--id",
            "successor-base-r23",
            "--side",
            "base",
            "--copy-directory",
            str(self.local / "copy-base"),
        ]
        request = {"kind": "request"}
        with (
            patch.object(arm, "build", return_value=request) as build,
            patch.object(arm, "summary", return_value={"status": "planned"}),
            redirect_stdout(io.StringIO()) as output,
        ):
            successor.main(arguments[1:])
        build.assert_called_once()
        self.assertEqual(json.loads(output.getvalue()), {"status": "planned"})

        with (
            redirect_stderr(io.StringIO()),
            self.assertRaises(SystemExit) as raised,
        ):
            successor.main([*arguments[1:], "--output", str(self.local / "bad.json")])
        self.assertEqual(raised.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
