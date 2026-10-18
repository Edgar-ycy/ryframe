"""监控预检复用正式来源与存活代次核验，私有桥接只接受固定只读输入。"""
from __future__ import annotations
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch
sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_monitoring_authority as authority_module

class FakeDocument:
    def __init__(self, path: Path, value: dict, sha: str):
        self.path = path
        self.value = value
        self.raw = b"{}"
        self.sha256 = sha
        self.unchanged = 0

    def assert_unchanged(self):
        self.unchanged += 1


class MonitoringAuthorityTests(unittest.TestCase):
    def test_private_protocol_rejects_unknown_fields_relative_paths_and_wrong_types(self):
        value = {
            "format_version": 1, "kind": "monitoring-preflight",
            "backend": str(Path(__file__).resolve().parents[2]),
            "runtime": str(Path(__file__).resolve()),
            "target": str(Path(__file__).resolve()),
        }
        with patch.object(authority_module, "monitoring_preflight", return_value={"ok": True}) as run:
            self.assertEqual(authority_module.preflight_protocol(value), {"ok": True})
            self.assertEqual(run.call_count, 1)
        for key, invalid in (("format_version", True), ("kind", "other"), ("runtime", "relative"), ("backend", 1), ("write", True)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                authority_module.preflight_protocol({**value, key: invalid})

    def test_preflight_uses_static_authority_and_live_generation(self):
        backend = Path("C:/coordinator")
        runtime_path = Path("C:/evidence/runtime.json")
        target_path = Path("C:/evidence/target.json")
        launch_path = Path("C:/evidence/runtime-launch.json")
        runtime = {
            "paths": {
                "launch": str(launch_path),
                "bindings": "C:/evidence/bindings.json",
            },
            "digests": {"launch": "c" * 64},
            "restore": {
                "id": "restore-one",
                "backup_id": "backup-one",
                "plan_hash": "d" * 64,
                "scope_id": "restore-scope",
                "data_verified_at": "2026-09-12T08:00:00Z",
            },
            "source": {
                "backup_source_sha": "1" * 40,
                "backend_product_sha": "2" * 40,
                "backend_execution_sha": "3" * 40,
                "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
                "frontend_sha": "4" * 40,
            },
            "endpoints": {
                "api": "http://127.0.0.1:18080/readyz",
                "worker": "http://127.0.0.1:19091/readyz",
                "frontend": "http://127.0.0.1:14174",
            },
            "processes": {
                role: {"identity": {"pid": index + 10, "started": str(index + 20), "executable": f"C:/{role}.exe"}}
                for index, role in enumerate(("api", "worker", "frontend"))
            },
        }
        launch_request = {
            name: {}
            for name in (
                "authority",
                "registration",
                "roots",
                "paths",
                "digests",
                "artifacts",
                "tools",
            )
        }
        launch_request.update(
            {
                "format_version": 1,
                "kind": "restore-runtime-launch-request",
                "environment": {
                    "document": {"path": "C:/evidence/environment.json", "bytes": 2, "sha256": "e" * 64},
                    "variables": [],
                    "sha256": "f" * 64,
                },
            }
        )
        runtime_document = FakeDocument(runtime_path, runtime, "a" * 64)
        launch_document = FakeDocument(launch_path, {"request": launch_request}, "c" * 64)
        target_document = FakeDocument(target_path, {}, "b" * 64)
        target = {
            "maintenance_execution": {
                "root": "C:/maintenance",
                "binding": {"kind": "current-backend", "path": "C:/maintenance"},
                "build": {"path": "C:/evidence/build.json", "bytes": 2, "sha256": "8" * 64},
            },
            "product_execution": {
                "roots": {
                    "source_backend": "C:/product",
                    "execution_backend": "C:/execution",
                    "frontend": "C:/frontend",
                }
            },
        }
        runtime_authority = {"kind": "restore-runtime-authority"}
        dataset = {
            "source_generation": {"path": "C:/evidence/stop.json", "bytes": 2, "sha256": "9" * 64},
            "dataset_lineage": {"path": "C:/evidence/lineage.json", "bytes": 2, "sha256": "0" * 64},
        }
        descriptors = {
            runtime_path: {"path": str(runtime_path), "bytes": 2, "sha256": "a" * 64},
            target_path: {"path": str(target_path), "bytes": 2, "sha256": "b" * 64},
            launch_path: {"path": str(launch_path), "bytes": 2, "sha256": "c" * 64},
        }
        live = Mock(return_value={"runtime_receipt_sha256": "a" * 64})
        with (
            patch.object(authority_module, "repository", return_value=backend),
            patch.object(authority_module, "read_json_document", side_effect=[runtime_document, launch_document]),
            patch.object(authority_module, "validate_runtime_receipt", return_value=runtime),
            patch.object(
                authority_module,
                "_verified_target",
                return_value=(target_document, target, {"target": {}}, {}, dataset),
            ),
            patch.object(authority_module, "verify_static_runtime", return_value=runtime_authority),
            patch.object(authority_module, "verify_live_generation", live),
            patch.object(authority_module, "_descriptor", side_effect=lambda document: descriptors[document.path]),
        ):
            result = authority_module.monitoring_preflight(backend, runtime_path, target_path)
        self.assertEqual(result["runtime"], descriptors[runtime_path])
        self.assertEqual(result["target_plan"], descriptors[target_path])
        self.assertEqual(result["launch"], descriptors[launch_path])
        self.assertEqual(result["source_generation"], dataset["source_generation"])
        self.assertEqual(result["endpoints"]["api_metrics"], "http://127.0.0.1:18080/api/v1/monitor/metrics")
        self.assertEqual(result["endpoints"]["worker_metrics"], "http://127.0.0.1:19091/metrics")
        self.assertEqual(live.call_args.args[1:4], (backend, Path("C:/execution"), Path("C:/frontend")))
        self.assertEqual(live.call_args.kwargs["product_backend"], Path("C:/product"))
        self.assertGreaterEqual(runtime_document.unchanged, 1)
        self.assertGreaterEqual(target_document.unchanged, 1)
        self.assertGreaterEqual(launch_document.unchanged, 1)
