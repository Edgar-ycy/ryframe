"""剩余Python历史消费适配契约；完整历史/库存/锁风险由Rust真实模型覆盖。"""
import copy
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory
import devex_clone_factory_context as context
from devex_clone_target_fixture import Fixture
from devex_clone_target_state import generation_lock
from restore_build import file_digest

class TargetHistoryTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(Path(__file__).resolve().parents[2] / ".local-tests/t", prefix="")
        self.addCleanup(temporary.cleanup)
        self.f = Fixture(Path(temporary.name), self)
        prepared = self.f.prepared()
        self.f.publish_prepared_files()
        # 明确替代已由Rust验证的历史端口，只验证当前Python消费者的参数/绑定/代次复核。
        self.initialized = {"generation": copy.deepcopy(prepared["generation"])}
        self.write(self.f.output / "initialized.json", self.initialized)
        self.binding = self.bind(self.f.output / "initialized.json")
        self.counter = 0
        self.enterContext(patch.object(context, "initialization_history", side_effect=lambda *args:
            (copy.deepcopy(self.initialized), copy.deepcopy(self.f.request))))

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


    def test_history_is_valid_after_copy_without_rechecking_initial_database_image(self):
        self.f.extra_objects["uploads"] = [self.f.scope + "/copied-business-object"]
        calls = len(self.f.calls)
        result, request, resources = self.check()
        self.assertEqual(result, self.initialized)
        self.assertEqual(request, self.f.request)
        self.assertEqual(resources.request, request)
        self.assertFalse(any(Path(command[0]).stem in {"mysql", "reset", "migrate", "tenant-data"}
                             or any(argument.startswith("__s3-") for argument in command)
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


    def test_generation_change_rejected(self):
        original = copy.deepcopy(self.initialized["generation"]); original["configuration_sha256"] = "1" * 64
        with patch.object(context, "context", return_value=(original, SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None))):
            with self.assertRaises(ValueError): self.check()


    def test_external_binding_changed_during_context_cannot_rebind_publication(self):
        def observe(*args, storage_run=None, owned_lock_identity=None, request_descriptor=None):
            self.assertIsNone(storage_run)
            prepared = json.loads((self.f.output / "prepare.json").read_text(encoding="utf-8"))
            self.assertEqual(request_descriptor, prepared["request"])
            path = self.f.output / "initialized.json"
            path.write_bytes(path.read_bytes() + b"\n")
            self.binding.update(self.bind(path))
            return copy.deepcopy(self.initialized["generation"]), SimpleNamespace(storage_runtime_binding=None, cache_runtime_binding=None)
        with patch.object(context, "context", side_effect=observe):
            with self.assertRaises(ValueError): self.check()
