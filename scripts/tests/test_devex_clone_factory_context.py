"""复制目标初始化历史的离线回归；不访问服务或写业务资源。"""
import copy
from dataclasses import asdict, replace
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_factory_context as context
import devex_clone_target as target
from devex_clone_target_fixture import Fixture
from devex_clone_target_state import generation_lock
from restore_build import file_digest
from restore_reference_plan import plan_hash


class TargetHistoryTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(Path(__file__).resolve().parents[2] / ".local-tests/t", prefix="")
        self.addCleanup(temporary.cleanup)
        self.f = Fixture(Path(temporary.name), self)
        self.f.mocks[-1].side_effect = self.inventory_fixture
        target.prepare_target(self.f.root, self.f.path, self.f.output, self.f.run)
        self.initialized = target.initialize_target(self.f.root, self.f.output, self.f.run)
        self.binding = self.bind(self.f.output / "initialized.json")
        self.counter = 0

    def inventory_fixture(self, backend, side, tools, receipt, output, *, environment):
        """补齐离线捕获形状；原始内容均为替身，不作为实际库存证明。"""
        value = self.f.inventory(backend, side, tools, receipt, output, environment=environment)
        observations = []
        for db, original in zip(self.f.request["target"]["databases"], value.observations, strict=True):
            observations.append(replace(original, resource={"kind": "database", "scope_id": self.f.scope,
                                "server_uuid": db["server_uuid"], "database": db["database"]}))
        raw_files = {}
        for key in context.KEYS:
            for phase in ("before", "after"):
                name = f"{phase}-target-{key}.json"
                self.write(output / name, {"offline_test_fixture": key})
                raw_files[name] = file_digest(output / name)
        for phase in ("before", "after"):
            self.write(output / f"binding-{phase}.json", {"offline_test_fixture": True})
        mapping = {db["key"]: asdict(image) for db, image in zip(self.f.request["target"]["databases"], observations, strict=True)}
        receipt_value = {"format_version": 1, "status": "side_inventory_captured", "side": "target",
                         "scope_id": self.f.scope, "keys": list(context.KEYS), "observations": mapping,
                         "inventories": raw_files, "binding_sha256": plan_hash({"offline_test_fixture": True})}
        self.write(output / "inventory.json", receipt_value)
        value.observations = tuple(observations)
        value.receipt_file = self.bind(output / "inventory.json")
        return value

    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value), encoding="utf-8")

    @staticmethod
    def bind(path):
        return {"path": str(path), **file_digest(path)}

    def check(self, *, owned_lock_identity=None, storage_run=None):
        output = self.f.local / f"observation-{self.counter}"
        self.counter += 1; output.mkdir()
        return context.target_history(self.f.root, self.binding, output, self.f.run,
                                      owned_lock_identity=owned_lock_identity, storage_run=storage_run)

    def republish_receipt(self, mutate):
        path = self.f.output / "inventory-initial/inventory.json"
        value = json.loads(path.read_text(encoding="utf-8")); mutate(value)
        self.write(path, value)
        result = copy.deepcopy(self.initialized)
        result["inventory"]["receipt"] = self.bind(path)
        for name in ("initialized.json", "initialized-candidate.json"):
            self.write(self.f.output / name, result)
        self.binding = self.bind(self.f.output / "initialized.json")

    def test_history_is_valid_after_copy_without_rechecking_initial_database_image(self):
        self.f.extra_objects["uploads"] = [self.f.scope + "/copied-business-object"]
        calls = len(self.f.calls)
        result, request, resources = self.check()
        self.assertEqual(result, self.initialized)
        self.assertEqual(request, self.f.request)
        self.assertEqual(resources.request, request)
        self.assertFalse(any(Path(command[0]).stem in {"mysql", "aws", "reset", "migrate", "tenant-data"}
                             for command in self.f.calls[calls:]))

    def test_external_initialized_binding_cannot_be_replaced(self):
        path = self.f.output / "initialized.json"
        value = json.loads(path.read_text(encoding="utf-8")); value["completed_at"] = "changed"
        self.write(path, value)
        with self.assertRaises(ValueError): self.check()

    def test_cache_runtime_is_checked_while_original_history_is_returned_unchanged(self):
        f = self.f
        old = copy.deepcopy(f.request["storage"]["redis"])
        proof = {"redis": {**old, "run_id": "4" * 40},
                 "request": f.bound(f.local / "cache-request.json", {"previous": old})}
        (f.local / "cache-target").mkdir()
        cache_request = {"initialized": self.binding}
        before = file_digest(f.output / "initialized.json")
        f.storage_restarted = True
        with patch("devex_clone_storage.registered_storage_binding", return_value=None), \
                patch("devex_clone_cache.registration", return_value=(cache_request, object())), \
                patch("devex_clone_cache.registered_cache_binding", return_value=proof) as cache, \
                generation_lock(f.output) as lock_identity:
            result, request, resources = self.check(storage_run=f.local, owned_lock_identity=lock_identity)
            cache.assert_called_once_with(f.root, f.local, owned_lock_identity=lock_identity)
        self.assertEqual(result, self.initialized)
        self.assertEqual(request, f.request)
        self.assertEqual(resources.cache_runtime_binding, proof)
        self.assertEqual(result["generation"]["storage"]["redis"], old)
        self.assertEqual(file_digest(f.output / "initialized.json"), before)
        with patch("devex_clone_storage.registered_storage_binding", return_value=None), \
                patch("devex_clone_cache.registered_cache_binding", side_effect=ValueError("latest failed")):
            with self.assertRaisesRegex(ValueError, "latest failed"):
                self.check(storage_run=f.local)

    def test_cache_from_other_initialization_does_not_receive_current_target_lock(self):
        f = self.f
        other = f.local / "other-initialization"
        other.mkdir()
        different = copy.deepcopy(f.request)
        different["id"] = "different-fresh-target"
        source = f.bound(other / "source-request.json", different)
        self.write(other / "prepare.json", {"request": source})
        initialized = f.bound(other / "initialized.json", {"fixture": "other"})
        (f.local / "cache-target").mkdir()
        cache_request = {"initialized": initialized}
        with patch("devex_clone_storage.registered_storage_binding", return_value=None), \
                patch("devex_clone_cache.registration", return_value=(cache_request, object())), \
                patch("devex_clone_cache.registered_cache_binding", return_value=None) as cache, \
                generation_lock(f.output) as lock_identity:
            result, request, resources = self.check(storage_run=f.local, owned_lock_identity=lock_identity)
            cache.assert_called_once_with(f.root, f.local, owned_lock_identity=None)
        self.assertEqual(result, self.initialized)
        self.assertEqual(request, f.request)
        self.assertIsNone(resources.cache_runtime_binding)

    def test_missing_creation_confirmation_is_rejected(self):
        (self.f.output / "create-shared.confirmed.json").unlink()
        with self.assertRaises(ValueError): self.check()

    def test_candidate_or_saved_request_change_rejected(self):
        for name in ("initialized-candidate.json", "request.json"):
            path = self.f.output / name; original = path.read_bytes()
            self.write(path, {})
            with self.assertRaises(ValueError): self.check()
            path.write_bytes(original)

    def test_original_inventory_raw_change_or_missing_rejected(self):
        path = self.f.output / "inventory-initial/before-target-shared.json"
        original = path.read_bytes(); self.write(path, {"different": True})
        with self.assertRaises(ValueError): self.check()
        path.write_bytes(original); path.unlink()
        with self.assertRaises(ValueError): self.check()

    def test_inventory_failed_publication_rejected(self):
        self.write(self.f.output / "inventory-initial/failure.json", {})
        with self.assertRaises(ValueError): self.check()

    def test_root_failure_and_active_lock_rejected(self):
        path = self.f.output / "failure.json"; self.write(path, {})
        with self.assertRaises(ValueError): self.check()
        path.unlink()
        (self.f.output / "initialize.lock").mkdir()
        with self.assertRaises(ValueError): self.check()

    def test_owned_live_lock_allows_history_and_remains_owned(self):
        lock = self.f.output / "initialize.lock"
        with generation_lock(self.f.output):
            identity = lock.stat().st_ino
            result, _, _ = self.check(owned_lock_identity=identity)
            self.assertEqual(result, self.initialized)
            self.assertEqual(lock.stat().st_ino, identity)
        self.assertFalse(lock.exists())

    def test_missing_or_wrong_owned_lock_rejected(self):
        with self.assertRaises(ValueError): self.check(owned_lock_identity=100)
        with generation_lock(self.f.output):
            identity = (self.f.output / "initialize.lock").stat().st_ino
            for wrong in (identity + 1, True, -1):
                with self.assertRaises(ValueError): self.check(owned_lock_identity=wrong)

    def test_owned_lock_disappearing_during_context_is_rejected(self):
        lock = self.f.output / "initialize.lock"; lock.mkdir()
        identity = lock.stat().st_ino
        def observe(*args, storage_run=None, owned_lock_identity=None):
            self.assertIsNone(storage_run)
            lock.rmdir()
            return copy.deepcopy(self.initialized["generation"]), SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None)
        with patch.object(context, "context", side_effect=observe):
            with self.assertRaises(ValueError): self.check(owned_lock_identity=identity)

    def test_incomplete_raw_inventory_set_rejected_even_with_bound_receipt(self):
        self.republish_receipt(lambda value: value["inventories"].pop("before-target-shared" + ".json"))
        with self.assertRaises(ValueError): self.check()

    def test_inventory_binding_before_or_after_change_rejected(self):
        path = self.f.output / "inventory-initial/binding-after.json"
        self.write(path, {"offline_test_fixture": "changed"})
        with self.assertRaises(ValueError): self.check()

    def test_receipt_and_published_images_must_match(self):
        self.republish_receipt(lambda value: value["observations"]["shared"]["tables"]["biz_tenant_fence"].update(rows=1))
        with self.assertRaises(ValueError): self.check()

    def test_reset_release_report_mutation_rejected(self):
        path = next((self.f.output / "reset-state").glob("*.report.json"))
        value = json.loads(path.read_text(encoding="utf-8")); value["status"] = "reused"
        self.write(path, value)
        with self.assertRaises(ValueError): self.check()

    def test_generation_change_rejected(self):
        original = copy.deepcopy(self.initialized["generation"]); original["configuration_sha256"] = "1" * 64
        with patch.object(context, "context", return_value=(original, SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None))):
            with self.assertRaises(ValueError): self.check()

    def test_failure_appearing_during_live_context_is_rejected(self):
        def observe(*args, storage_run=None, owned_lock_identity=None):
            self.assertIsNone(storage_run)
            self.write(self.f.output / "inventory-initial/failure.json", {})
            return copy.deepcopy(self.initialized["generation"]), SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None)
        with patch.object(context, "context", side_effect=observe):
            with self.assertRaises(ValueError): self.check()

    def test_original_file_changing_during_live_context_is_rejected(self):
        def observe(*args, storage_run=None, owned_lock_identity=None):
            self.assertIsNone(storage_run)
            self.write(self.f.output / "inventory-initial/after-target-shared.json", {"changed": True})
            return copy.deepcopy(self.initialized["generation"]), SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None)
        with patch.object(context, "context", side_effect=observe):
            with self.assertRaises(ValueError): self.check()

    def test_external_binding_changed_during_context_cannot_rebind_publication(self):
        def observe(*args, storage_run=None, owned_lock_identity=None):
            self.assertIsNone(storage_run)
            path = self.f.output / "initialized.json"
            path.write_bytes(path.read_bytes() + b"\n")
            self.binding.update(self.bind(path))
            return copy.deepcopy(self.initialized["generation"]), SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None)
        with patch.object(context, "context", side_effect=observe):
            with self.assertRaises(ValueError): self.check()


if __name__ == "__main__":
    unittest.main()
