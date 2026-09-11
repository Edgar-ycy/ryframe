"""源后继的账本顺序、失败不可重放和全像零漂移；不访问业务服务。"""
from contextlib import ExitStack, contextmanager, nullcontext
import copy
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_generation as generation
import devex_clone_seed_generation_runtime as runtime
import devex_clone_seed_rebind as rebind
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash


class GenerationTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temp = WorkspaceDirectory(dir=self.backend / ".local-tests/t", prefix="g")
        self.addCleanup(temp.cleanup)
        self.directory = Path(temp.name)
        self.registration = self.file("c52.json", {"immutable": "C52"})
        self.rebound = self.file("rebind.json", {"immutable": "same-storage"})
        self.successor = self.file("successor.json", {"immutable": "review"})
        self.build = self.file("build.json", {"sources": {"full": {"source": {
            "worktree_fingerprint": "sha256:" + "f" * 64, "snapshot": {"head": "a" * 40}}}}})
        self.request = {"format_version": 1, "kind": "devex-clone-seed-source-generation", "id": "r24-source",
                        "source_registration": self.registration, "source_rebind": self.rebound,
                        "review_successor": self.successor, "current_storage": {"same": True},
                        "execution_backend": str(self.backend), "expected_backend_sha": "a" * 40,
                        "adapter_contract": None, "product_backend": None, "backend_build": self.build,
                        "maintenance_build": self.file("maintenance.json", {"maintenance": True}),
                        "source_environment": self.file("env.json", {"environment": {}})}
        self.request_path = self.directory / "request.json"
        write_json(self.request_path, self.request)
        self.source = {"directory": self.directory, "request": {"source": {"scope_id": "same-source", "databases": ["all"]},
                       "tools": {"mysql": "fixed"}, "max_object_bytes": 1024},
                       "review_successor": {"source_result": self.registration}, "review_successor_binding": self.successor,
                       "source_rebind": self.rebound, "storage": {"storage": {"same": True}}, "source_generation": None}
        self.source["generation"] = {"physical_binding": {"same": "physical-source"}}
        self.source["registration"] = {"source_request": self.file("old-request.json", self.source["request"]),
            "generation_verified": self.file("old-generation.json", self.source["generation"]), "source_environment": self.request["source_environment"]}
        self.prefix = [self.record(52, "source-register", self.registration), self.record(53, "source-rebind", self.rebound)]
        self.state = {"attempts": self.prefix + [self.record(54, generation.START, status="running")]}
        self.output = self.directory / "g0054"
        self.runtime = Mock(execution=self.backend, selected=self.source["request"]["source"], runtime=self.output / "runtime")
        self.runtime.environment.return_value = nullcontext({})
        self.runtime.prepare.side_effect = self.prepare_runtime
        self.runtime.finish.side_effect = lambda: write_json(self.output / "runtime-evidence.json", self.source["generation"])
        self.runtime.running_evidence.side_effect = lambda: write_json(self.output / "running-evidence.json", self.source["generation"])
        self.runtime.start.side_effect = lambda checkpoint: checkpoint()
        self.image = {"database": {"business": "same", "preserved": "same", "placement": "same"},
                      "objects": {"bytes": "same", "metadata": "same", "owners": "same"}, "redis": {"state": "same"}}

    def file(self, filename, value):
        path = self.directory / filename
        write_json(path, value)
        return binding(path)

    @staticmethod
    def record(number, mode, result=None, status="passed"):
        return {"number": number, "stage": "seed-runtime", "mode": mode, "status": status, "result": result}

    def prepare_runtime(self):
        self.runtime.runtime.mkdir()
        write_json(self.output / "intent.json", {"operations": {"api": "a" * 32, "worker": "b" * 32}})
        write_json(self.runtime.runtime / "runtime.json", {"backend_root": str(self.backend)})
        for role in ("api", "worker"):
            write_json(self.runtime.runtime / f"{role}.json", {"identity": {"role": role}})
        write_json(self.output / "producers.json", {"processes": [{"name": "ancestor", "identity": {"old": True}}]})

    def generation_evidence(self, _backend, request, _run):
        return {"source": read_json(Path(self.build["path"]))["sources"]["full"]["source"]["snapshot"],
                "worktree_fingerprint": request["worktree_fingerprint"], "request_sha256": plan_hash(request),
                "runtime": read_json(Path(request["runtime"]["path"])),
                "maintenance": read_json(Path(request["maintenance_build"]["path"])),
                "processes": {role: read_json(Path(item["path"]))["identity"] for role, item in request["processes"].items()},
                "operator_declared_producers": read_json(Path(request["producers_registry"]["path"]))["processes"],
                "physical_binding": self.source["generation"]["physical_binding"],
                "external_writers_discovered": False, "clone_verified": False, "restore_qualified": False}

    def capture(self, *_args):
        output = _args[-2]
        output.mkdir()
        image = copy.deepcopy(self.image)
        if output.name in {"running", "after"} and getattr(self, "drift", None):
            category, field = self.drift
            image[category][field] = "external-write"
        write_json(output / "image.json", {"image": image})
        return binding(output / "image.json")

    def execute(self):
        with patch.object(generation, "inputs", return_value=(self.request, self.source, self.prefix)), \
                patch("restore_source_lineage.derive_dataset_lineage", return_value={"derived": "C52"}), \
                patch.object(runtime, "GenerationRuntime", return_value=self.runtime), \
                patch("devex_clone_seed_generation_images.capture_image", side_effect=self.capture), \
                patch("devex_clone_seed_generation_images.verify_image", side_effect=lambda _b, descriptor, *_a, **_kw: read_json(Path(descriptor["path"]))), \
                patch("devex_clone_source_proof.verify_generation", side_effect=self.generation_evidence):
            return generation.execute_generation(self.backend, self.directory, self.request_path, 54)

    def test_success_preserves_predecessor_and_only_publishes_after_equal_complete_images(self):
        original = {path: path.read_bytes() for path in self.directory.glob("*.json")}
        value = self.execute()
        self.assertEqual(value["status"], "seed_source_generation_running")
        self.assertEqual(value["remote_writes"], 0)
        self.assertFalse(value["restore_qualified"])
        self.assertEqual(value["source_registration"], self.registration)
        self.assertEqual(value["history_sha256"], plan_hash(self.prefix))
        self.assertEqual(original, {path: path.read_bytes() for path in self.directory.glob("*.json")})
        self.runtime.start.assert_called_once()
        self.runtime.stop.assert_not_called()
        self.runtime.retain.assert_called_once()
        self.assertFalse((self.output / "source-request.json").exists())
        self.assertEqual(read_json(Path(value["before"]["path"]))["image"], read_json(Path(value["running"]["path"]))["image"])

    def test_full_table_placement_object_metadata_and_owner_drift_never_publish(self):
        for category, fields in self.image.items():
            for field in fields:
                with self.subTest(category=category, field=field):
                    self.output = self.directory / ("g-" + category + field)
                    self.runtime.runtime = self.output / "runtime"
                    self.drift = category, field
                    with patch.object(generation, "local_path", side_effect=lambda _b, _p, **_kw: self.output), self.assertRaisesRegex(ValueError, "前后像变化"):
                        self.execute()
                    self.assertTrue((self.output / "before/image.json").is_file())
                    self.assertTrue((self.output / "after/image.json").is_file())
                    self.assertFalse((self.output / "source-request.json").exists())
        self.runtime.finish.assert_not_called()

    def test_ready_failure_stops_tree_and_captures_after_image_without_publishing(self):
        self.runtime.start.side_effect = TimeoutError("ready")
        with self.assertRaisesRegex(TimeoutError, "ready"):
            self.execute()
        self.runtime.stop.assert_called_once()
        self.assertTrue((self.output / "after/image.json").is_file())
        self.assertFalse((self.output / "source-request.json").exists())
        self.runtime.finish.assert_not_called()

    def test_preserves_original_failure_if_stop_cannot_prove_quiescence(self):
        self.runtime.start.side_effect = TimeoutError("ready")
        self.runtime.stop.side_effect = ValueError("unknown process")
        with self.assertRaises(TimeoutError) as raised:
            self.execute()
        self.assertTrue(raised.exception.__notes__)
        self.assertFalse((self.output / "source-request.json").exists())

    def test_preflight_rejects_every_previous_generation_attempt_before_new_intent(self):
        for status in ("running", "failed", "passed"):
            state = {"attempts": self.prefix + [self.record(54, generation.START, status=status)]}
            with self.subTest(status=status), patch.object(generation, "load_state", return_value=state), self.assertRaisesRegex(ValueError, "只执行一次"):
                generation.preflight(self.directory)
        self.assertFalse(self.output.exists())

    def test_history_allows_only_owned_single_generation_after_rebind_before_export(self):
        with patch("devex_clone_run._require_owned_run") as owned:
            rebind.history(self.directory, self.state, self.registration, current=54)
        owned.assert_called_once_with(self.directory)
        for records in ([self.prefix[0], self.state["attempts"][-1]],
                        self.prefix + [self.record(54, generation.START, status="failed"), self.record(55, generation.START, status="running")],
                        self.prefix + [self.record(54, generation.START), self.record(55, "source-export"), self.record(56, generation.START)]):
            with self.subTest(records=records), self.assertRaises(ValueError):
                rebind.history(self.directory, {"attempts": records}, self.registration)

    def test_inputs_reject_extra_fields_wrong_lineage_storage_and_unowned_controller(self):
        with patch.object(generation, "load_state", return_value=self.state), \
                patch("reference_fixture_successor_generation.rebuild", return_value=self.request), \
                patch("devex_clone_seed_export._source", return_value=self.source), patch("devex_clone_run._require_owned_run"):
            self.assertEqual(generation.inputs(self.backend, self.directory, self.request_path, 54)[0], self.request)
            for field in ("extra", "source_registration", "source_rebind", "review_successor", "current_storage"):
                request = {**self.request, field: {"different": True}}
                self.request_path.write_text(__import__("json").dumps(request), encoding="utf-8")
                with self.subTest(field=field), self.assertRaises(ValueError):
                    generation.inputs(self.backend, self.directory, self.request_path, 54)
        with patch("devex_clone_run._require_owned_run", side_effect=ValueError("unowned")), self.assertRaisesRegex(ValueError, "unowned"):
            generation.inputs(self.backend, self.directory, self.request_path, 54)

    def test_unknown_runtime_files_block_stop_before_any_process_or_port_action(self):
        directory = self.directory / "unknown-runtime"
        directory.mkdir()
        (directory / "unregistered.exe").write_bytes(b"unknown")
        instance = runtime.GenerationRuntime.__new__(runtime.GenerationRuntime)
        instance.runtime, instance.operations = directory, {"api": "a" * 32, "worker": "b" * 32}
        with patch.object(runtime, "terminate_owned_process_tree") as stop, patch.object(runtime, "require_closed_port") as port, self.assertRaises(ValueError):
            instance.stop()
        stop.assert_not_called()
        port.assert_not_called()

    @contextmanager
    def running_fixture(self):
        value = self.execute()
        (self.directory / "results").mkdir()
        start_path = self.directory / "results/0054.json"
        write_json(start_path, value)
        descriptor = binding(start_path)
        self.file("state.json", {"fixed": "state"})
        state = {"attempts": self.prefix + [self.record(54, generation.START, descriptor)]}
        with ExitStack() as stack:
            stack.enter_context(patch.object(generation, "load_state", return_value=state))
            stack.enter_context(patch.object(generation, "predecessor", return_value=self.source))
            stack.enter_context(patch("restore_source_lineage.verify_dataset_lineage", return_value={"derived": "C52"}))
            stack.enter_context(patch("devex_clone_seed_generation_images.verify_image",
                side_effect=lambda _b, document, *_args, **_kwargs: read_json(Path(document["path"]))))
            verifier = stack.enter_context(patch.object(runtime, "verify_running_evidence", return_value={"runtime": "same"}))
            storage = stack.enter_context(patch("devex_clone_storage.current_storage_binding", return_value=self.request["current_storage"]))
            yield descriptor, state, verifier, storage

    def test_running_verification_is_readonly_and_live_check_is_explicit(self):
        with self.running_fixture() as (descriptor, _state, verifier, storage):
            before = {str(path): path.read_bytes() for path in self.directory.rglob("*") if path.is_file()}
            with patch("subprocess.run", side_effect=AssertionError("只读 verifier 不应执行工具或写 Git")):
                facts = generation.verify_running_source(self.backend, descriptor, live=False)
            self.assertEqual(facts["receipt"]["generation_id"], "g0054")
            self.assertFalse(verifier.call_args.kwargs["live"])
            storage.assert_not_called()
            generation.verify_running_source(self.backend, descriptor, live=True)
            self.assertTrue(verifier.call_args.kwargs["live"])
            storage.assert_called_once()
            self.assertEqual(before, {str(path): path.read_bytes() for path in self.directory.rglob("*") if path.is_file()})

    def test_running_verification_rejects_unbound_fields_and_stopped_generation(self):
        with self.running_fixture() as (descriptor, state, _verifier, _storage):
            state["attempts"].append(self.record(55, generation.STOP, self.rebound))
            with self.assertRaisesRegex(ValueError, "停止或恢复"):
                generation.verify_running_source(self.backend, descriptor, live=True)
            state["attempts"].pop()
            path = Path(descriptor["path"])
            value = read_json(path)
            value["extra"] = True
            path.write_text(__import__("json").dumps(value), encoding="utf-8")
            changed = binding(path)
            state["attempts"][-1]["result"] = changed
            with self.assertRaises(ValueError):
                generation.verify_running_source(self.backend, changed, live=False)

    def test_registered_checkpoint_holds_one_run_lock_and_expires_after_exit(self):
        @contextmanager
        def lock(directory):
            path = directory / "run.lock"
            path.mkdir()
            try:
                yield
            finally:
                path.rmdir()

        facts = {"directory": self.directory, "same": "generation"}
        with patch.object(generation, "verify_running_source", return_value=facts), \
                patch("devex_clone_run_state.run_lock", side_effect=lock) as acquire, \
                patch("devex_clone_run._require_owned_run") as owner:
            with generation.registered_running_source(self.backend, self.registration) as checkpoint:
                self.assertTrue((self.directory / "run.lock").is_dir())
                self.assertEqual(checkpoint(), facts)
            acquire.assert_called_once_with(self.directory)
            self.assertGreaterEqual(owner.call_count, 3)
            with self.assertRaisesRegex(ValueError, "离开控制区间"):
                checkpoint()


if __name__ == "__main__":
    unittest.main()
