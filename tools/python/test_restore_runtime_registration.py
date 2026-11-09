import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_runtime_registration as registration
import restore_runtime
from restore_runtime_evidence import artifact_snapshot
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


class RestoreRuntimeRegistrationTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.runtime = self.root / "target-runtime"
        self.reference_path = self.root / "reference.json"
        self.target_path = self.root / "target.json"
        self.environment_path = self.root / "environment.json"
        (self.root / "receipts").mkdir()
        self.output = self.root / "receipts/runtime-registration.json"
        self.reference = {
            "format_version": 1,
            "target_side": "base",
            "target": {
                "scope_id": "formal-restore-b0",
                "runtime_dir": str(self.runtime),
                "frontend_url": "http://127.0.0.1:14174",
            },
        }
        self.target = {
            "format_version": 1,
            "kind": "restore-reference-target-plan",
            "target_side": "base",
            "product_plan": {
                "id": "restore-one",
                "backup_id": "backup-one",
                "scope_id": "formal-restore-b0",
                "api_ready_url": "http://127.0.0.1:18080/readyz",
                "worker_ready_url": "http://127.0.0.1:19091/readyz",
                "frontend_sha": "a" * 40,
            },
            "fresh_target": {
                "environment": {},
            },
            "maintenance_execution": {
                "root": str(ROOT),
                "binding": {},
                "build": {},
            },
            "product_execution": {
                "roots": {},
                "backend_product_sha": "b" * 40,
                "backend_execution_sha": "b" * 40,
                "frontend_sha": "a" * 40,
                "builds": {},
                "adapter": None,
            },
        }
        self.write_json(
            self.environment_path,
            {
                "environment": {
                    "APP_ENV": "test",
                    "APP_SCOPE_ID": "formal-restore-b0",
                    "APP_JOBS_MODE": "external",
                    "APP_APP_HOST": "127.0.0.1",
                    "APP_APP_PORT": "18080",
                    "APP_JOBS_HEALTH_HOST": "127.0.0.1",
                    "APP_JOBS_HEALTH_PORT": "19091",
                }
            },
        )
        self.target["fresh_target"]["environment"] = artifact_snapshot(
            self.environment_path
        ).descriptor()
        self.write_json(self.reference_path, self.reference)
        self.write_json(self.target_path, self.target)

    @staticmethod
    def write_json(path: Path, value: dict) -> None:
        path.write_text(json.dumps(value), encoding="utf-8")

    def verifier(self, _backend: Path, reference: dict, path: Path) -> dict:
        self.assertEqual(reference, self.reference)
        self.assertEqual(path, self.target_path)
        return self.target

    def prepare_registration(self) -> None:
        anchor = self.root.stat()
        self.write_json(self.output, {
            "format_version": 1,
            "kind": "restore-runtime-registration",
            "reference_plan": artifact_snapshot(self.reference_path).descriptor(),
            "target_plan": self.target_descriptor(),
            "observation": {
                "runtime": {"exists": False, "entries": [], "anchor": {
                    "path": str(self.root), "device": anchor.st_dev,
                    "inode": anchor.st_ino, "created_ns": anchor.st_ctime_ns,
                }},
                "ports_idle": ["api", "worker", "frontend"],
            },
            "remote_writes": 0,
        })

    def target_descriptor(self) -> dict:
        return artifact_snapshot(self.target_path).descriptor()

    def test_context_holds_lock_and_supports_live_checks(self):
        self.prepare_registration()
        with patch.object(registration, "validate_plan"), patch.object(
            registration, "require_closed_port"
        ):
            with registration.registered_stopped_runtime(
                ROOT,
                self.output,
                self.target_descriptor(),
                target_verifier=self.verifier,
            ) as checkpoint:
                lock = ROOT / ".local-tests" / registration.REGISTRATION_LOCK.lock_name
                self.assertTrue((lock / "owner.json").is_file())
                checked = checkpoint()
                self.assertEqual(checked["target_plan"], self.target_descriptor())
            self.assertFalse(lock.exists())
            with self.assertRaisesRegex(ValueError, "已离开"):
                checkpoint()

    def test_context_rechecks_after_restore_failure_and_preserves_original(self):
        self.prepare_registration()
        with patch.object(registration, "validate_plan"), patch.object(
            registration, "require_closed_port"
        ):
            with self.assertRaisesRegex(RuntimeError, "restore failed") as caught:
                with registration.registered_stopped_runtime(
                    ROOT,
                    self.output,
                    self.target_descriptor(),
                    target_verifier=self.verifier,
                ):
                    self.runtime.mkdir()
                    (self.runtime / "api.json").write_text("unknown", encoding="utf-8")
                    raise RuntimeError("restore failed")
        self.assertTrue(any("零进程状态复核也失败" in note for note in caught.exception.__notes__))

    def test_context_rejects_runtime_file_created_during_successful_restore(self):
        self.prepare_registration()
        with patch.object(registration, "validate_plan"), patch.object(
            registration, "require_closed_port"
        ), self.assertRaisesRegex(ValueError, "运行目录已出现"):
            with registration.registered_stopped_runtime(
                ROOT,
                self.output,
                self.target_descriptor(),
                target_verifier=self.verifier,
            ):
                self.runtime.mkdir()
                (self.runtime / "launch.json").write_text("unknown", encoding="utf-8")

    def test_descriptor_mismatch_and_busy_port_fail_closed(self):
        self.prepare_registration()
        wrong = {**self.target_descriptor(), "sha256": "b" * 64}
        with patch.object(registration, "validate_plan"), self.assertRaisesRegex(
            ValueError, "调用方绑定"
        ):
            registration.verify_registration(
                ROOT, self.output, wrong, target_verifier=self.verifier
            )
        with patch.object(registration, "validate_plan"), patch.object(
            registration, "require_closed_port", side_effect=ValueError("port busy")
        ), self.assertRaisesRegex(ValueError, "port busy"):
            registration.verify_registration(
                ROOT,
                self.output,
                self.target_descriptor(),
                target_verifier=self.verifier,
            )

    def test_retired_registration_protocol_rejects_both_write_values(self):
        protocol = {
            "backend_dir": str(ROOT),
            "format_version": 1,
            "kind": restore_runtime.PROTOCOL_KIND,
            "operation": "register",
            "plan": str(self.reference_path),
            "target_plan": str(self.target_path),
            "output": str(self.output),
        }
        for write in (False, True):
            with self.subTest(write=write), self.assertRaises(restore_runtime.RuntimeProtocolError):
                restore_runtime.decode_request(json.dumps({**protocol, "write": write}))
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
