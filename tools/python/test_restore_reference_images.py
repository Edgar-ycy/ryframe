"""完整资源前后像的精确比较；库存、存储与对象读取使用现有边界的离线替身。"""

from contextlib import nullcontext
import copy
import hashlib
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_reference as reference
import restore_reference_images as images
from devex_clone_inventory import KEYS, expected_owners
from devex_clone_transfer import DatabaseObservation
from restore_reference_fixture import environment, stored_backup
from restore_runtime_evidence import read_json_document


class RestoreImagesControlTests(unittest.TestCase):
    def test_control_uses_coordinator_bridges_with_the_original_target_storage(self):
        backend = Path(__file__).resolve().parents[2]
        work = backend / ".local-tests/reference-control-fixture"
        bridges = [{"path": str(work / "source-bridge.json"), "sha256": "a" * 64}]
        manifest = {"target_storage_run": {"path": str(work / "storage")}}
        subject = object.__new__(images.RestoreImages)
        subject.backend, subject.plan = backend, {"work_dir": str(work)}
        execution = work / "product-backend"
        target = {"execution_backend": {"path": str(execution)}}
        subject.arm = {"request": {"build_bridges": bridges}, "manifest": manifest, "target": target}
        with (
            patch.object(images, "execution_backend", return_value=(execution, {})) as selected,
            patch.object(images, "artifact_sources", return_value=nullcontext()) as audited,
            patch.object(images, "target_storage_control", return_value=nullcontext()) as controlled,
            subject.control() as active,
        ):
            self.assertIs(active, subject)
            selected.assert_called_once_with(backend, target)
            audited.assert_called_once_with(backend, bridges)
            controlled.assert_called_once_with(backend, work, manifest)


class RestoreImagesTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        for side in ("source", "target"):
            original = self.plan[side]["databases"]
            self.plan[side]["databases"] = [{**original[0 if index == 0 else 1], "key": key,
                "database": f"{side}_{index}", "mode": "shared" if index < 2 else "dedicated"} for index, key in enumerate(KEYS)]
        self.work = reference.work_directory(self.plan)
        root, self.manifest = stored_backup(self.plan, self.work)
        self.manifest.update(control_schema_fingerprint="c" * 16, tenant_schema_fingerprint="d" * 64)
        databases = {}
        for index, target in enumerate(self.plan["target"]["databases"]):
            migrations = {"seaql_tenant_data_migrations": {"rows": 1, "sha256": "c" * 64}}
            kept = {"ryframe_resource_ownership": {"rows": 2 if index == 0 else 1, "sha256": "d" * 64}}
            if index == 0:
                migrations["seaql_migrations"] = {"rows": 1, "sha256": "c" * 64}
                kept.update({table: {"rows": 0, "sha256": "e" * 64} for table in ("sys_backup_set", "sys_backup_resource", "sys_restore_run")})
            self.manifest["databases"][index]["tables"] += [{"table": table, **value} for table, value in migrations.items()]
            kept.update(migrations)
            schema = {"control_schema_fingerprint": "c" * 16 if index == 0 else None, "tenant_schema_fingerprint": "d" * 64}
            databases[target["database"]] = {"resource": {"kind": "database", "scope_id": "target",
                "server_uuid": "uuid-1", "database": target["database"]}, "schema_sha256": reference.plan_hash(schema),
                "tables": {"sys_post": {"rows": 0, "sha256": "a" * 64}}, "preserved": kept,
                "all_tables": sorted(["sys_post", *kept]), "ownership": list(expected_owners("target", index == 0))}
        objects = {}
        for bucket in images.BUCKETS:
            owner = f"ryframe-owner:v1:target:object-storage:{bucket}".encode()
            objects[bucket] = {"keys": ["target/.ryframe-owner"], "owner": {"bytes": len(owner), "sha256": hashlib.sha256(owner).hexdigest()}}
        self.initialized = {"inventory": {"observations": databases}, "objects": objects,
                            "redis": {"keys": ["target-owner"], "owner": "target", "sentinel": "fresh"}}
        self.before, self.after = images.expected_images(self.plan, self.manifest, self.initialized)
        self.request = {"target": self.plan["target"], "tools": self.plan["tools"],
                        "maintenance_build": self.document("maintenance.json", {"built": True})}
        self.arm = {"target": self.request, "initialized": self.initialized, "target_storage_run": {"path": str(self.work / "storage")}}
        self.inputs = SimpleNamespace(target=read_json_document(self.write("target.json", {
            "fresh_target": {"initialized": self.document("initialized.json", self.initialized)}})),
            manifest=SimpleNamespace(value=self.manifest), assert_unchanged=Mock())
        self.subject = object.__new__(images.RestoreImages)
        self.subject.backend, self.subject.plan, self.subject.inputs = self.backend, self.plan, self.inputs
        self.subject.arm, self.subject.before, self.subject.after = self.arm, self.before, self.after
        self.subject.bindings, self.subject.tools = {}, Mock()
        self.subject.environment_document = read_json_document(self.write("environment.json", {"environment": {}}))
        self.subject.environment = images.Environments({}, images.configured({}))
        self.resources = Mock(selected=self.plan["target"], execution_backend=self.backend,
                              object_configuration=self.plan["target"]["s3"]["configuration"])
        self.current = copy.deepcopy(self.before)
        self.resources.objects.side_effect = lambda **_kw: copy.deepcopy(self.current["objects"])
        self.resources.redis_state.side_effect = lambda **_kw: copy.deepcopy(self.current["redis"])
        for replacement in (patch.object(images, "target_history", return_value=(self.initialized, self.request, self.resources)),
                            patch.object(images, "capture_side_inventory", side_effect=self.capture),
                            patch.object(images, "inventory_history")):
            replacement.start()
            self.addCleanup(replacement.stop)

    def write(self, name, value):
        path = self.work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        reference.write_json(path, value)
        return path

    def document(self, name, value):
        return reference.document_binding(read_json_document(self.write(name, value)))

    def capture(self, _backend, side, _tools, _maintenance, output, **kwargs):
        self.assertEqual(side, "target")
        self.assertEqual(kwargs["evidence_root"], self.backend)
        output.mkdir()
        observations = [DatabaseObservation(**copy.deepcopy(value)) for value in self.current["databases"].values()]
        path = output / "inventory.json"
        reference.write_json(path, {"observations": self.current["databases"]})
        return SimpleNamespace(observations=observations, receipt_file=reference.document_binding(read_json_document(path)))

    def test_complete_before_image_is_bound_without_remote_writes(self):
        descriptor = self.subject.capture("before", after=False)
        value = reference.read_json(Path(descriptor["path"]))
        self.assertEqual(value["image"], self.before)
        self.assertTrue(value["matches_expected"])
        self.assertEqual(value["target_plan"], reference.document_binding(self.inputs.target))
        self.subject.tools.restore_database.assert_not_called()
        self.subject.tools.create_object_if_absent.assert_not_called()

    def test_any_business_preserved_schema_or_owner_change_rejects_complete_before(self):
        changes = [lambda value: value["databases"]["target_0"]["tables"]["sys_post"].update(rows=1),
                   lambda value: value["databases"]["target_0"]["preserved"]["sys_restore_run"].update(rows=1),
                   lambda value: value["databases"]["target_0"]["preserved"]["sys_backup_set"].update(sha256="f" * 64),
                   lambda value: value["databases"]["target_0"]["ownership"].clear(),
                   lambda value: value["databases"]["target_2"].update(schema_sha256="f" * 64),
                   lambda value: value["databases"]["target_3"]["all_tables"].append("unknown"),
                   lambda value: value["objects"]["uploads"]["keys"].append("target/external"),
                   lambda value: value["objects"]["uploads"]["owner"].update(sha256="f" * 64),
                   lambda value: value["redis"].update(owner="other")]
        for index, change in enumerate(changes):
            self.current = copy.deepcopy(self.before)
            change(self.current)
            with self.subTest(change=index), self.assertRaisesRegex(ValueError, "完整资源像"):
                self.subject.capture(f"before-{index}", after=False)
            self.assertFalse(reference.read_json(Path(self.subject.bindings[f"before-{index}"]["path"]))["matches_expected"])
        self.subject.tools.restore_database.assert_not_called()
        self.subject.tools.create_object_if_absent.assert_not_called()

    def test_after_image_checks_full_business_and_preserved_tables(self):
        self.current = copy.deepcopy(self.after)
        with patch.object(self.subject, "_objects", return_value=(self.current["objects"], [])):
            value = self.subject.capture("after", after=True)
        self.assertTrue(reference.read_json(Path(value["path"]))["matches_expected"])
        self.current["databases"]["target_0"]["preserved"]["sys_restore_run"]["rows"] = 1
        with patch.object(self.subject, "_objects", return_value=(self.current["objects"], [])), self.assertRaises(ValueError):
            self.subject.capture("after-drift", after=True)

    def test_inventory_receipt_drift_during_object_observation_is_rejected(self):
        def objects(**_kwargs):
            path = self.work / "restore-base-before/inventory/inventory.json"
            path.write_bytes(path.read_bytes() + b"\n")
            return self.current["objects"]
        self.resources.objects.side_effect = objects
        with self.assertRaises(ValueError):
            self.subject.capture("before", after=False)
        self.assertNotIn("before", self.subject.bindings)
        self.assertIn("before-failure", self.subject.bindings)


if __name__ == "__main__":
    unittest.main()
