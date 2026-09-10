import hashlib
import json
import os
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_artifacts as artifacts


class ArtifactEvidenceTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.scope = "artifact-test"
        self.base = self.root / "objects/exports" / self.scope
        self.base.mkdir(parents=True)
        (self.base / ".ryframe-owner").write_text(
            f"ryframe-owner:v1:{self.scope}:object-storage:exports"
        )
        self.data = b"actual export data"
        (self.base / "result.xlsx").write_bytes(self.data)
        self.record = {
            "job_id": "123",
            "file_id": "456",
            "bucket": "exports",
            "key": "result.xlsx",
            "bytes": len(self.data),
            "sha256": hashlib.sha256(self.data).hexdigest(),
        }
        environment = mock.patch.dict(
            os.environ,
            {
                "APP_OBJECT_STORAGE_BACKEND": "local",
                "APP_OBJECT_STORAGE_LOCAL_BASE_DIR": str(self.root / "objects"),
                "APP_DATABASE_NAME": "owned_test",
            },
        )
        environment.start()
        self.addCleanup(environment.stop)
        for name, value in (
            ("verify_runtime", {"scope_id": self.scope}),
            ("database_identity", "server-uuid"),
            ("export_record", self.record),
        ):
            patch = mock.patch.object(artifacts, name, return_value=value)
            patch.start()
            self.addCleanup(patch.stop)
        (self.root / ".local-tests").mkdir()
        self.receipt = self.root / ".local-tests/receipt.json"

    def inspect(self, operation):
        return artifacts.inspect(operation, self.root, self.root, "123", self.receipt)

    def test_object_and_both_metadata_records_must_disappear(self):
        self.assertEqual(self.inspect("snapshot")["state"], "present")
        with mock.patch.object(artifacts, "mysql", return_value="0"):
            self.assertEqual(self.inspect("verify-deleted")["state"], "pending")
            (self.base / "result.xlsx").unlink()
            self.assertEqual(self.inspect("verify-deleted")["state"], "deleted")
        with mock.patch.object(artifacts, "mysql", return_value="1"):
            self.assertEqual(self.inspect("verify-deleted")["state"], "pending")

    def test_wrong_scope_or_file_digest_cannot_be_reported_as_success(self):
        (self.base / "result.xlsx").write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "摘要"):
            self.inspect("snapshot")
        (self.base / "result.xlsx").write_bytes(self.data)
        self.inspect("snapshot")
        receipt = json.loads(self.receipt.read_text())
        receipt["scope_id"] = "other-scope"
        self.receipt.write_text(json.dumps(receipt))
        with self.assertRaisesRegex(ValueError, "不匹配"):
            self.inspect("verify-deleted")

    def test_invalid_owner_path_and_id_fail_closed(self):
        for key in ("../outside", "/absolute", "x/../y", "x\\y", "x:stream", "x//y"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                artifacts.object_bytes(self.scope, key)
        for identifier in ("0", "1 OR 1=1", "-1", str(2**63)):
            with self.subTest(identifier=identifier), self.assertRaises(ValueError):
                artifacts.identifier(identifier)
        (self.base / ".ryframe-owner").write_text("other-owner")
        with self.assertRaisesRegex(ValueError, "ownership"):
            self.inspect("snapshot")

    def test_snapshot_is_create_only_and_s3_must_stay_on_loopback(self):
        self.inspect("snapshot")
        with self.assertRaises(FileExistsError):
            self.inspect("snapshot")
        with mock.patch.dict(os.environ, {"APP_OBJECT_STORAGE_ENDPOINT": "https://example.test"}):
            with self.assertRaisesRegex(ValueError, "本机"):
                artifacts.signed_request("GET", "exports", "scope/key")

    def test_receipt_must_stay_in_the_current_backend_evidence_root(self):
        with self.assertRaisesRegex(ValueError, r"\.local-tests"):
            artifacts.inspect(
                "snapshot", self.root, self.root, "123", self.root / "outside.json"
            )
        with self.assertRaisesRegex(ValueError, "未知"):
            artifacts.inspect("other", self.root, self.root, "123", self.receipt)

    def test_mysql_uses_explicit_endpoint_and_tls_without_client_defaults(self):
        executable = self.root / "mysql.exe"
        executable.write_bytes(b"fixture")
        values = {
            "APP_DATABASE_HOST": "127.0.0.1",
            "APP_DATABASE_PORT": "13306",
            "APP_DATABASE_USERNAME": "fixture",
            "APP_DATABASE_PASSWORD": "fixture-password",
            "APP_DATABASE_TLS_MODE": "required",
            "RYFRAME_E2E_MYSQL_CLIENT": str(executable),
            "RYFRAME_CI_MYSQL_CONTAINER_ID": "",
        }
        with mock.patch.dict(os.environ, values), mock.patch.object(artifacts.subprocess, "run") as run:
            run.return_value = mock.Mock(returncode=0, stdout="verified\n")
            self.assertEqual(artifacts.mysql("SELECT 1;"), "verified")
            arguments = run.call_args.args[0]
            self.assertEqual(arguments[:2], [str(executable), "--no-defaults"])
            self.assertIn("--ssl-mode=REQUIRED", arguments)
            self.assertIn("--port=13306", arguments)
            self.assertNotIn("fixture-password", " ".join(arguments))
            os.environ["APP_DATABASE_TLS_MODE"] = "verify_identity"
            with self.assertRaisesRegex(ValueError, "TLS"):
                artifacts.mysql("SELECT 1;")


if __name__ == "__main__":
    unittest.main()
