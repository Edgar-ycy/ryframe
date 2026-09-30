"""正式恢复输入只能从已发布来源和 fresh ownership 只读推导。"""

import copy
from datetime import datetime, timezone
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_input_plan as inputs
import restore_reference as reference
import restore_reference_target as target
from restore_reference_target_fixture import setup


class RestoreInputPlanTests(unittest.TestCase):
    def setUp(self):
        setup(self)
        source_generation = self.write_binding(
            "source-generation-current.json", {"status": "source_generation_stopped"}
        )
        backend_build = self.write_binding("source-backend-build.json", {"role": "api-worker"})
        runtime = self.write_binding("source-runtime.json", {"status": "stopped"})
        processes = {
            role: self.write_binding(f"source-{role}.json", {"role": role})
            for role in ("api", "worker")
        }
        producers = self.write_binding("source-producers.json", {"processes": []})
        self.source_request = {
            "format_version": 1,
            "kind": "devex-clone-source-export",
            "id": "source-export-current",
            "source": copy.deepcopy(self.plan["source"]),
            "tools": copy.deepcopy(self.plan["tools"]),
            "backend_build": backend_build,
            "maintenance_build": copy.deepcopy(self.request["maintenance_build"]),
            "worktree_fingerprint": "sha256:" + "a" * 64,
            "runtime": runtime,
            "processes": processes,
            "producers_registry": producers,
            "max_object_bytes": 1024,
        }
        self.arm["source"] = {
            "request": copy.deepcopy(self.source_request),
            "source_generation": source_generation,
        }
        self.arm["result"]["source_generation"] = source_generation
        self.arm["manifest"] = {"copy_directory": str(self.work / "arm-copy")}
        arm_path = self.write("arm-input.json", self.arm["result"])
        self.arm["binding"] = self.descriptor(arm_path)
        patches = [
            patch.object(
                inputs,
                "verify_published_arm_input",
                side_effect=lambda *_: copy.deepcopy(self.arm),
            ),
            patch.object(
                inputs,
                "request_binding",
                side_effect=lambda *_: ({}, copy.deepcopy(self.selected)),
            ),
            patch.object(
                inputs,
                "execution_backend",
                side_effect=lambda *_: (
                    self.backend,
                    copy.deepcopy(self.maintenance_binding),
                ),
            ),
            patch.object(
                inputs,
                "verify_comparison_sources",
                side_effect=lambda _root, value: copy.deepcopy(value),
            ),
        ]
        for item in patches:
            item.start()
            self.addCleanup(item.stop)

    def write(self, name, value):
        path = self.work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        reference.write_json(path, value, new=False)
        return path

    def descriptor(self, path):
        return {"path": str(path), **reference.file_digest(path)}

    def write_binding(self, name, value):
        return self.descriptor(self.write(name, value))

    def build_reference(self, work_dir=None):
        return inputs.build_reference(
            self.backend,
            self.paths["arm_input"],
            self.paths["fresh_target_verify"],
            "base",
            self.plan["id"],
            work_dir or self.work / "base-restore",
        )

    def fault_time(self):
        return datetime.now(timezone.utc).isoformat(timespec="microseconds").replace(
            "+00:00", "Z"
        )

    def test_reference_is_derived_from_source_and_exact_fresh_target(self):
        value = self.build_reference()
        self.assertEqual(value["source"], self.source_request["source"])
        self.assertEqual(value["tools"], self.source_request["tools"])
        self.assertEqual(value["target_side"], "base")
        self.assertEqual(value["target"]["runtime_dir"], self.selected["runtime_dir"])
        self.assertEqual(value["target"]["worker_ready_url"], self.selected["worker_ready_url"])
        self.assertNotIn("frontend_url", value["source"])
        self.assertFalse(Path(value["work_dir"]).exists())

    def test_reference_rejects_side_generation_and_overlapping_work_dir(self):
        original = copy.deepcopy(self.arm)
        changes = [
            lambda value: value.update(target_side="candidate"),
            lambda value: value["result"].update(source_generation={"changed": True}),
        ]
        for change in changes:
            self.arm = copy.deepcopy(original)
            change(self.arm)
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.build_reference()
        self.arm = original
        runtime = Path(self.selected["runtime_dir"])
        runtime.mkdir(parents=True, exist_ok=True)
        with self.assertRaisesRegex(ValueError, "完全分离"):
            self.build_reference(runtime / "restore")

    def test_product_and_bindings_are_derived_from_the_same_chain(self):
        reference_plan = self.build_reference()
        reference_path = self.write("base-reference.json", reference_plan)
        product = inputs.build_product(
            self.backend,
            reference_path,
            self.paths["backup_receipt"],
            self.paths["comparison_sources"],
            self.paths["arm_input"],
            self.paths["fresh_target_verify"],
            "base",
            "restore-run-base",
            self.fault_time(),
        )
        self.assertEqual(product["backup_id"], reference_plan["id"])
        self.assertEqual(product["frontend_sha"], "b" * 40)
        self.assertEqual(product["worker_ready_url"], self.selected["worker_ready_url"])
        product_path = self.write("generated-product.json", product)
        target_plan = target.capture_target_plan(
            self.backend,
            reference_plan,
            backup_receipt=self.paths["backup_receipt"],
            comparison_sources=self.paths["comparison_sources"],
            arm_input=self.paths["arm_input"],
            fresh_target_verify=self.paths["fresh_target_verify"],
            product_plan=product_path,
        )
        target_path = self.write("generated-target.json", target_plan)
        now = reference.now()
        record = {
            "plan": product,
            "plan_hash": reference.plan_hash(product),
            "status": "data_verified",
            "started_at": now,
            "data_verified_at": now,
            "completed_at": None,
            "recovered_at": now,
            "failure": None,
        }
        record_path = self.write("data-verified.json", record)
        bindings = inputs.build_bindings(
            self.backend,
            reference_path,
            target_path,
            self.paths["backup_receipt"],
            record_path,
        )
        self.assertEqual(bindings["record"], record)
        self.assertEqual(bindings["manifest"]["id"], reference_plan["id"])

    def test_publish_is_create_new_and_preserves_post_write_drift(self):
        value = self.build_reference()
        output = self.work / "published-reference.json"
        self.assertEqual(inputs._publish(self.backend, output, lambda: value, ()), value)
        original = output.read_bytes()
        self.assertEqual(original, inputs._canonical_json_bytes(value))
        inputs._verify_published(output, value, original)
        self.assertEqual(list(self.work.glob(f".{output.name}.*.pending")), [])
        with self.assertRaises(ValueError):
            inputs._publish(self.backend, output, lambda: value, ())
        self.assertEqual(output.read_bytes(), original)
        drift_output = self.work / "drift-reference.json"
        build = Mock(side_effect=[value, value, {**value, "id": "changed"}])
        with self.assertRaisesRegex(ValueError, "发布后"):
            inputs._publish(self.backend, drift_output, build, ())
        self.assertEqual(json.loads(drift_output.read_text(encoding="utf-8")), value)

    def test_publish_rejects_truncated_reread_and_preserves_racing_target(self):
        value = self.build_reference()
        content = inputs._canonical_json_bytes(value)
        truncated = self.work / "truncated-reference.json"
        truncated.write_bytes(content[:-1])
        with self.assertRaisesRegex(ValueError, "写后字节"):
            inputs._verify_published(truncated, value, content)

        output = self.work / "racing-reference.json"
        original_link = inputs.os.link

        def racing_link(source, destination):
            Path(destination).write_bytes(b"unrelated partial writer")
            return original_link(source, destination)

        with patch.object(inputs.os, "link", side_effect=racing_link), \
                self.assertRaisesRegex(ValueError, "并发创建"):
            inputs._publish(self.backend, output, lambda: value, ())
        self.assertEqual(output.read_bytes(), b"unrelated partial writer")
        self.assertEqual(list(self.work.glob(f".{output.name}.*.pending")), [])

    def test_private_protocol_requires_explicit_write_pair_and_rejects_duplicates(self):
        request = {
            "backend_dir": str(self.backend),
            "format_version": 1,
            "operation": "reference",
            "arm_input": str(self.paths["arm_input"]),
            "fresh_target_verify": str(self.paths["fresh_target_verify"]),
            "side": "base",
            "id": self.plan["id"],
            "work_dir": str(self.work / "preview"),
            "write": False,
        }
        protocol = {"format_version": 1, "kind": "ryframe-xtask-recovery-inputs",
                    "request": request}
        parsed = inputs._request_from_protocol(protocol)
        self.assertEqual(parsed.backend_dir, self.backend)
        invalid = copy.deepcopy(protocol)
        invalid["request"]["write"] = True
        with self.assertRaises(inputs.RestoreInputsProtocolError):
            inputs._request_from_protocol(invalid)
        parent_jump = copy.deepcopy(protocol)
        parent_jump["request"]["work_dir"] = str(self.backend / "evidence" / ".." / "work")
        with self.assertRaises(inputs.RestoreInputsProtocolError):
            inputs._request_from_protocol(parent_jump)
        raw = json.dumps(protocol).replace('"side": "base"',
                                           '"side": "base", "side": "candidate"')
        environment = {inputs.PROTOCOL_KEY: raw}
        with self.assertRaises(inputs.RestoreInputsProtocolError):
            inputs.private_protocol_request(argv=[], environment=environment)
        self.assertNotIn(inputs.PROTOCOL_KEY, environment)
        with self.assertRaises(inputs.RestoreInputsProtocolError):
            inputs.private_protocol_request(argv=["reference"],
                                            environment={inputs.PROTOCOL_KEY: json.dumps(protocol)})


if __name__ == "__main__":
    unittest.main()
