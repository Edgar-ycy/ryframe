"""目标证据原生协议的失败关闭与单次调用边界。"""
from contextlib import nullcontext
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import devex_clone_target_evidence as client


class NativeEvidenceTransportTests(unittest.TestCase):
    def setUp(self):
        self.target = Path(__file__).resolve().parents[2] / ".local-tests/native-target-evidence"
        self.backend = self.target.parent.parent
        self.enterContext(patch.object(client, "tool_binding", return_value={"fixed": True}))
        self.enterContext(patch.object(client, "verified_tool", return_value=nullcontext("bound-xtask.exe")))
        self.run = self.enterContext(patch.object(client.subprocess, "run"))

    def response(self, value, status=0):
        self.run.return_value = subprocess.CompletedProcess([], status, json.dumps(value).encode(), b"")

    def test_bound_tool_receives_stdin_and_no_project_credentials(self):
        self.response({"ok": True, "result": {"status": "completed"}})
        result = client.evidence("history", self.backend, self.target)
        self.assertEqual(result, {"status": "completed"})
        args, options = self.run.call_args
        self.assertEqual(args[0], ["bound-xtask.exe", "__target-evidence"])
        payload = json.loads(options["input"])
        self.assertEqual(payload["backend"], str(self.target.parent.parent))
        self.assertEqual(payload["target"], str(self.target))
        self.assertEqual(payload["operation"], "history")
        self.assertNotIn("APP_ENV", options["env"])
        self.assertIs(options["stderr"], subprocess.DEVNULL)

    def test_unknown_or_inconsistent_results_are_never_retried(self):
        for status, response in [(1, {"ok": True, "result": {}}),
                                 (0, {"ok": True, "result": []}),
                                 (0, {"ok": True, "result": {}, "extra": True}),
                                 (0, {"ok": False, "error": {"message": "untrusted"}})]:
            with self.subTest(status=status, response=response):
                self.run.reset_mock()
                self.response(response, status)
                with self.assertRaises(ValueError):
                    client.evidence("history", self.backend, self.target)
                self.run.assert_called_once()

    def test_transport_failure_does_not_reveal_raw_output_or_retry(self):
        self.run.side_effect = subprocess.TimeoutExpired(["fake-secret"], 30,
                                                        output=b"fake-secret", stderr=b"fake-secret")
        with self.assertRaises(ValueError) as captured:
            client.evidence("history", self.backend, self.target)
        self.assertNotIn("fake-secret", str(captured.exception))
        self.run.assert_called_once()

    def test_duplicate_json_response_is_rejected(self):
        self.run.return_value = subprocess.CompletedProcess([], 0, b'{"ok":true,"result":{},"result":{}}', b"")
        with self.assertRaises(ValueError):
            client.evidence("history", self.backend, self.target)
        self.run.assert_called_once()

    def test_missing_tool_binding_never_invokes_cargo_or_a_subprocess(self):
        with patch.object(client, "tool_binding", return_value=None), self.assertRaisesRegex(
                ValueError, "缺少受管 Rust 工具"):
            client.evidence("history", self.backend, self.target)
        self.run.assert_not_called()

    def test_invalid_boundary_and_oversized_request_never_launch(self):
        for target, fields in [(self.target.parent, {}),
                               (self.target, {"manifest": "x" * (1024 * 1024)})]:
            with self.assertRaises(ValueError):
                client.evidence("history", self.backend, target, **fields)
        self.run.assert_not_called()


class NativeInventoryHistoryTests(unittest.TestCase):
    """通过正式已绑定工具核对原 factory/reference 调用；不替换 evidence 或 native。"""

    def setUp(self):
        from devex_clone_inventory import KEYS
        from restore_build import file_digest
        from restore_reference_plan import plan_hash
        from workspace_directory import WorkspaceDirectory

        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(self.backend / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.target = Path(temporary.name) / "target"
        self.directory = self.target / "inventory-initial"
        self.directory.mkdir(parents=True)
        binding = {"scope_id": "inventory-native", "来源": "真实绑定"}
        for name in ("binding-before.json", "binding-after.json"):
            (self.directory / name).write_text(json.dumps(binding), encoding="utf-8")
        databases, observations, images = [], {}, {}
        raw = {}
        for index, key in enumerate(KEYS):
            name = f"database_{index}"
            databases.append({"key": key, "database": name, "server_uuid": "physical-server"})
            image = {"resource": {"kind": "database", "scope_id": "inventory-native",
                                  "server_uuid": "physical-server", "database": name}, "row_count": 0}
            observations[key], images[name] = image, image
            for phase in ("before", "after"):
                filename = f"{phase}-target-{key}.json"
                (self.directory / filename).write_bytes(b"raw CLI output\n")
                raw[filename] = file_digest(self.directory / filename)
        self.receipt = {"format_version": 1, "status": "side_inventory_captured", "side": "target",
                        "scope_id": "inventory-native", "keys": list(KEYS), "observations": observations,
                        "inventories": raw, "binding_sha256": plan_hash(binding), "remote_writes": 0}
        self.request = {"target": {"scope_id": "inventory-native", "databases": databases}}
        self.initial = {"receipt": {}, "observations": images}
        self.save()

    def save(self):
        from restore_build import file_digest

        filename = self.directory / "inventory.json"
        filename.write_text(json.dumps(self.receipt), encoding="utf-8")
        self.initial["receipt"] = {"path": str(filename), **file_digest(filename)}

    def call(self, **options):
        from devex_clone_factory_context import inventory_history

        self.assertIsNone(inventory_history(self.backend, self.target, self.initial, self.request, **options))

    def test_real_bound_tool_accepts_complete_history_and_reference_explicit_directory(self):
        self.call()
        destination = self.target.parent / "reference-inventory"
        self.directory.rename(destination)
        self.directory = destination
        self.save()
        self.call(inventory_directory=self.directory)

    def test_real_bound_tool_requires_external_resume_qualification(self):
        destination = self.target / "inventory-resume-r001"
        self.directory.rename(destination)
        self.directory = destination
        self.save()
        (self.directory / "failure.json").write_text("resolved by outer controller", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.call()
        self.call(resumed=True)

    def test_real_bound_tool_rejects_missing_output_and_physical_substitution_without_leaking(self):
        self.request["target"]["databases"][0]["server_uuid"] = "PRIVATE-SENTINEL"
        with self.assertRaises(ValueError) as caught:
            self.call()
        self.assertNotIn("PRIVATE-SENTINEL", str(caught.exception))
        self.request["target"]["databases"][0]["server_uuid"] = "physical-server"
        (self.directory / "before-target-shared.json").unlink()
        with self.assertRaises(ValueError):
            self.call()


if __name__ == "__main__":
    unittest.main()
