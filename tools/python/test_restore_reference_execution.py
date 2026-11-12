"""正式恢复必须先通过严格目标计划与原生产品运行记录；外部资源使用离线替身。"""

import copy
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_reference as reference
import restore_reference_execution as execution
import restore_reference_target as target
from restore_reference_target_fixture import setup
from restore_reference_execution_fixture import guards


class RestoreExecutionTests(unittest.TestCase):
    def setUp(self):
        setup(self)
        self.target = target.capture_target_plan(self.backend, self.plan, **self.paths)
        self.target_path = self.write("target-plan.json", self.target)
        self.root = Path(self.backup_value["backup_root"])
        manifest = reference.read_json(self.root / "manifest.json")
        self.record = {"plan": copy.deepcopy(self.product), "plan_hash": execution.product_plan_hash(self.product),
                       "status": "running", "started_at": reference.now(), "data_verified_at": None,
                       "completed_at": None, "recovered_at": manifest["captured_at"], "failure": None}
        self.record_path = self.write("record.json", self.record)
        self.registration_path = self.write("registration.json", {"target_plan": self.descriptor(self.target_path)})
        self.args = SimpleNamespace(command="restore", copy_id=None, target_plan=self.target_path,
                                    backup_root=self.root, record=self.record_path, runtime_registration=self.registration_path)
        guards(self)

    def write(self, name, value):
        path = self.work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        reference.write_json(path, value, new=False)
        return path

    def descriptor(self, path):
        return {"path": str(path), **reference.file_digest(path)}

    def write_binding(self, name, value):
        return self.descriptor(self.write(name, value))

    def inputs(self):
        return execution.restore_inputs(self.backend, self.plan, self.target_path, self.root,
                                        self.record_path, self.registration_path, read_only=True)


    def test_direct_legacy_arguments_are_rejected_before_reading(self):
        for extra in (["--target-plan", "one", "--target-plan=two"], ["--target-p", "unused"],
                      ["--copy-id", "other"], ["--artifact", "other"], ["--missing"]):
            argv = ["restore", "--plan", "unused", "--backend-dir", str(self.backend), *extra]
            with patch.object(reference, "read_json") as read, \
                    self.assertRaises(reference.ReferenceProtocolError):
                reference.private_protocol_request(argv=argv)
            read.assert_not_called()

    def test_record_plan_must_match_every_embedded_field_even_when_hash_is_updated(self):
        changes = [lambda value: value.update(id="other"), lambda value: value.update(frontend_sha="e" * 40),
                   lambda value: value.update(fault_at="2026-01-01T00:00:00Z"),
                   lambda value: value.update(api_ready_url="http://127.0.0.1:3100/readyz"),
                   lambda value: value.update(worker_ready_url="http://127.0.0.1:19201/readyz"),
                   lambda value: value["databases"].reverse()]
        for change in changes:
            record = copy.deepcopy(self.record)
            change(record["plan"])
            record["plan_hash"] = execution.product_plan_hash(record["plan"])
            self.write("record.json", record)
            tools = Mock()
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "不完全一致"):
                reference.execute(self.args, self.plan, self.backend, tools, self.work)
            self.assertEqual(tools.mock_calls, [])
            self.assertFalse((self.work / "restore-base.json").exists())


    def test_same_data_from_another_backup_directory_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "同一正式备份目录"):
            execution.restore_inputs(self.backend, self.plan, self.target_path, self.work / "other-backup",
                                     self.record_path, self.registration_path, read_only=True)

    def test_execution_rechecks_sources_and_publishes_side_and_input_bindings(self):
        tools = Mock()
        with patch.object(target, "_native", side_effect=lambda _operation, args, **_kw: args["value"]) as sources:
            result = reference.execute(self.args, self.plan, self.backend, tools, self.work)
        self.assertEqual([call.args[1]["read_only"] for call in sources.call_args_list], [True, False, True])
        self.assertEqual([call.kwargs["write"] for call in sources.call_args_list], [False, True, False])
        self.assertEqual(tools.restore_database.call_count, 4)
        receipt = reference.read_json(self.work / "restore-base.json")
        self.assertEqual(receipt["status"], "completed")
        self.assertEqual(receipt["target_side"], "base")
        self.assertEqual(receipt["target_plan"], self.descriptor(self.target_path))
        self.assertEqual(receipt["record"], self.descriptor(self.record_path))
        self.assertEqual(receipt["runtime_registration"], self.descriptor(self.registration_path))
        self.assertEqual(set(result["resource_images"]), {"before", "after"})
        self.assertEqual(self.events[-1], "unlock")
        self.assertEqual(result["status"], "external_copy_completed")
        self.assertFalse((self.work / "restore.json").exists())

    def test_output_collision_does_not_overwrite_or_start_an_external_operation(self):
        path = self.write("restore-base.json", {"original": True})
        original = path.read_bytes()
        tools = Mock()
        with self.assertRaises(ValueError):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        self.assertEqual(path.read_bytes(), original)
        self.assertEqual(tools.mock_calls, [])

    def test_changed_target_plan_after_preflight_is_rejected_before_full_execution(self):
        inputs = self.inputs()
        self.target_path.write_bytes(self.target_path.read_bytes() + b"\n")
        tools = Mock()
        with self.assertRaises(ValueError):
            reference.restore(self.plan, tools, self.backend, inputs)
        self.assertEqual(tools.mock_calls, [])

    def test_input_drift_during_owner_verification_prevents_database_or_object_writes(self):
        tools = Mock()
        self.before_hook = lambda: self.record_path.write_bytes(self.record_path.read_bytes() + b"\n")
        with self.assertRaises(ValueError):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        tools.restore_database.assert_not_called()
        tools.create_object_if_absent.assert_not_called()
        self.assertEqual(reference.read_json(self.work / "restore-base.json")["status"], "failed")

    def test_unknown_write_is_recorded_and_never_replayed(self):
        tools = Mock()
        tools.restore_database.side_effect = RuntimeError("unknown write")
        with self.assertRaisesRegex(RuntimeError, "unknown write"):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        self.assertEqual(tools.restore_database.call_count, 1)
        self.assertIn("failure-after", reference.read_json(self.work / "restore-base.json")["resource_images"])
        self.assertEqual(reference.read_json(self.work / "restore-base.json")["status"], "failed")
        with self.assertRaises(ValueError):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        self.assertEqual(tools.restore_database.call_count, 1)

    def test_complete_fresh_image_drift_prevents_every_database_and_object_write(self):
        self.image_error = "完整前像漂移"
        tools = Mock()
        with self.assertRaisesRegex(ValueError, "完整前像漂移"):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        tools.restore_database.assert_not_called()
        tools.create_object_if_absent.assert_not_called()
        receipt = reference.read_json(self.work / "restore-base.json")
        self.assertEqual(receipt["status"], "failed")
        self.assertEqual(set(receipt["resource_images"]), {"before"})

    def test_registration_drift_or_resource_image_output_collision_prevents_preflight(self):
        original = self.registration_path.read_bytes()
        self.registration_path.write_text('{"wrong":true}', encoding="utf-8")
        tools = Mock()
        with self.assertRaises(ValueError):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        self.registration_path.write_bytes(original)
        (self.work / "restore-base-before").mkdir()
        with self.assertRaises(ValueError):
            reference.execute(self.args, self.plan, self.backend, tools, self.work)
        tools.restore_database.assert_not_called()
        tools.create_object_if_absent.assert_not_called()
        self.assertFalse((self.work / "restore-base.json").exists())


if __name__ == "__main__":
    unittest.main()
