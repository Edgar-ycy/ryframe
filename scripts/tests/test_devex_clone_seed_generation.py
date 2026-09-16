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
import source_fingerprints
from test_source_fingerprints import inventory


class GenerationTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        tools = inventory()
        tools["source"]["snapshot"]["clean"] = True
        self.coordinator_source = source_fingerprints.execution_source(tools)
        self.enterContext(patch.object(source_fingerprints, "current_execution_source", return_value=self.coordinator_source))
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
        self.state["attempts"][-1]["sources"] = self.coordinator_source
        self.output = self.directory / "g0054"
        self.runtime = Mock(execution=self.backend, selected=self.source["request"]["source"], runtime=self.output / "runtime")
        self.runtime.environment.return_value = nullcontext({})
        self.runtime.control_environment.side_effect = lambda: nullcontext({})
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

    def generation_evidence(self, _backend, request, _run, **_kwargs):
        return {"source": read_json(Path(self.build["path"]))["sources"]["full"]["source"]["snapshot"],
                "worktree_fingerprint": request["worktree_fingerprint"], "request_sha256": plan_hash(request),
                "runtime": read_json(Path(request["runtime"]["path"])),
                "maintenance": read_json(Path(request["maintenance_build"]["path"])),
                "processes": {role: read_json(Path(item["path"]))["identity"] for role, item in request["processes"].items()},
                "operator_declared_producers": read_json(Path(request["producers_registry"]["path"]))["processes"],
                "physical_binding": self.source["generation"]["physical_binding"],
                "external_writers_discovered": False, "clone_verified": False, "restore_qualified": False}

    def capture(self, *_args, **_kwargs):
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
                patch.object(generation, "load_state", return_value=self.state), \
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

    def test_segmented_start_allows_only_current_storage_and_paired_equivalent_build_receipts(self):
        previous = copy.deepcopy(self.request)
        previous["current_storage"] = {"attempt": 53}
        previous["backend_build"] = self.file("archived-build.json", {"old": "runtime"})
        previous["maintenance_build"] = self.file("archived-maintenance.json", {"old": "maintenance"})
        archive = {"receipt": {"request": self.file("archived-request.json", previous)}}
        storage = {"attempt": 64, "storage": {"identity": {"pid": 6400}}}
        cache = {"redis": {"pid": 6500}}
        segment = {"phase": "ready", "storage": storage, "cache": cache}
        current = {**copy.deepcopy(previous), "current_storage": storage,
                   "backend_build": self.request["backend_build"],
                   "maintenance_build": self.request["maintenance_build"]}
        with patch.object(generation, "_reregistered_builds") as registered:
            generation._archived_request(self.backend, archive, current, segment)
        registered.assert_called_once_with(self.backend, previous, current)
        for field in set(current) - {"current_storage", "backend_build", "maintenance_build"}:
            changed = copy.deepcopy(current)
            changed[field] = "changed"
            with self.subTest(field=field), self.assertRaises(ValueError):
                with patch.object(generation, "_reregistered_builds"):
                    generation._archived_request(self.backend, archive, changed, segment)
        for field in ("backend_build", "maintenance_build"):
            changed = copy.deepcopy(previous)
            changed["current_storage"] = storage
            changed[field] = current[field]
            with self.subTest(unpaired=field), self.assertRaisesRegex(ValueError, "成对"):
                generation._archived_request(self.backend, archive, changed, segment)
        with self.assertRaisesRegex(ValueError, "尚未全部恢复"):
            generation._archived_request(self.backend, archive, current, {**segment, "phase": "storage-ready"})

    def test_reregistered_builds_allow_only_environment_and_build_log_evidence_changes(self):
        source = {"snapshot": {"head": "a" * 40, "clean": True, "files": []}}
        sources = {"full": {"source": source}}
        runtime = {"format_version": 2, "kind": "runtime-build", "sources": sources,
                   "build": {"commands": {"api": ["cargo", "api"], "worker": ["cargo", "worker"]},
                             "profile": "dev", "target": "windows", "jobs": "4",
                             "toolchain": {"cargo": "fixed", "rustc": "fixed"},
                             "environment": {"variables": ["PATH"], "sha256": "a" * 64}},
                   "artifacts": {"api": {"bytes": 10, "sha256": "b" * 64},
                                 "worker": {"bytes": 20, "sha256": "c" * 64}}}
        old_runtime = self.file("runtime-old.json", runtime)
        runtime["build"]["environment"]["sha256"] = "d" * 64
        new_runtime = self.file("runtime-new.json", runtime)

        def maintenance(directory, log_sha):
            artifacts = {}
            for role in ("reset", "migrate", "tenant-data"):
                artifacts[role] = {"executable": str(directory / ("ryframe-" + role + ".exe")),
                                   "cargo_executable": str(self.directory / (role + "-cargo.exe")),
                                   "command": ["cargo", "build", role], "bytes": 30,
                                   "sha256": "e" * 64,
                                   "cargo_output": {"file": role + ".jsonl", "bytes": 40,
                                                    "sha256": log_sha},
                                   "cargo_log": {"file": role + ".log", "bytes": 50,
                                                 "sha256": log_sha}}
            return {"format_version": 1, "kind": "maintenance-build", "backend_root": str(execution),
                    "source": source, "source_inventory": sources["full"],
                    "toolchain": {"same": True}, "target_directory": str(self.directory / "target"),
                    "artifacts": artifacts, "restore_qualified": False, "resources_modified": False}

        execution = self.directory / "execution"
        (execution / ".local-tests").mkdir(parents=True)
        old_dir = execution / ".local-tests/maintenance-old"
        new_dir = execution / ".local-tests/maintenance-new"
        old_dir.mkdir()
        new_dir.mkdir()
        write_json(old_dir / "build.json", maintenance(old_dir, "f" * 64))
        write_json(new_dir / "build.json", maintenance(new_dir, "1" * 64))
        old_maintenance = binding(old_dir / "build.json")
        new_maintenance = binding(new_dir / "build.json")
        previous = {**self.request, "execution_backend": str(execution),
                    "backend_build": old_runtime, "maintenance_build": old_maintenance}
        current = {**self.request, "execution_backend": str(execution),
                   "backend_build": new_runtime, "maintenance_build": new_maintenance}
        validators = (patch("source_inventory.validate_build_source_domains", side_effect=lambda value, _repo: value),
                      patch("restore_build.validate_build_context", side_effect=lambda value: value),
                      patch("restore_build.verify_build_artifacts"),
                      patch("devex_clone_tools.verify_evidence", side_effect=lambda _root, path: read_json(path)))
        with validators[0], validators[1], validators[2], validators[3]:
            generation._reregistered_builds(self.backend, previous, current)

        changed_runtime = read_json(Path(new_runtime["path"]))
        changed_runtime["artifacts"]["api"]["bytes"] += 1
        changed = self.file("runtime-changed.json", changed_runtime)
        with validators[0], validators[1], validators[2], validators[3], \
                self.assertRaisesRegex(ValueError, "API/Worker"):
            generation._reregistered_builds(
                self.backend, previous, {**current, "backend_build": changed})

        changed_maintenance = read_json(Path(new_maintenance["path"]))
        changed_maintenance["artifacts"]["reset"]["sha256"] = "2" * 64
        changed_dir = execution / ".local-tests/maintenance-changed"
        changed_dir.mkdir()
        for artifact in changed_maintenance["artifacts"].values():
            artifact["executable"] = str(changed_dir / Path(artifact["executable"]).name)
        write_json(changed_dir / "build.json", changed_maintenance)
        changed = binding(changed_dir / "build.json")
        with validators[0], validators[1], validators[2], validators[3], \
                self.assertRaisesRegex(ValueError, "维护构建"):
            generation._reregistered_builds(
                self.backend, previous, {**current, "maintenance_build": changed})

    def test_segmented_start_changes_only_registered_process_generations_in_c60_image(self):
        old_redis = {"configuration": {"sha256": "c" * 64}, "distribution": "Ubuntu-24.04",
                     "executable": "/usr/bin/redis-server", "pid": 54, "port": 16390, "run_id": "a" * 40,
                     "sha256": "d" * 64, "started": "old", "wsl": {"sha256": "e" * 64}}
        archive = {"image": {"databases": {"rows": 100044}, "schema": {"all": True},
                              "objects": {"count": 257}, "owners": {"all": True},
                              "redis": {"keys": ["owner", "sentinel"]},
                              "storage": {"rustfs": {"identity": {"pid": 53}, "sha256": "f" * 64},
                                          "redis": old_redis}}}
        rustfs = {"identity": {"pid": 64}, "sha256": "f" * 64,
                  "process_receipt": {"sha256": "1" * 64}, "launch_receipt": {"sha256": "2" * 64}}
        redis = {**copy.deepcopy(old_redis), "pid": 65, "started": "new", "run_id": "b" * 40}
        segment = {"phase": "ready", "storage": {"storage": rustfs}, "cache": {"redis": redis}}
        current = copy.deepcopy(archive["image"])
        current["storage"] = {"rustfs": rustfs, "redis": redis}
        generation._archived_image(archive, current, segment)
        for domain in ("databases", "schema", "objects", "owners", "redis"):
            changed = copy.deepcopy(current)
            changed[domain] = {"changed": True}
            with self.subTest(domain=domain), self.assertRaises(ValueError):
                generation._archived_image(archive, changed, segment)
        for field in set(old_redis) - {"pid", "started", "run_id"}:
            changed_segment = copy.deepcopy(segment)
            changed_segment["cache"]["redis"][field] = "changed"
            with self.subTest(redis_field=field), self.assertRaisesRegex(ValueError, "缓存重启改变"):
                generation._archived_image(archive, current, changed_segment)

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

    def test_preflight_allows_one_audited_receipt_failure_after_closed_archive(self):
        failed = self.record(54, generation.START, status="failed")
        failed["error_type"] = "ValueError"
        state = {"attempts": self.prefix + [failed]}
        archive = {"records": [], "receipt": {"source_registration": self.registration}}
        segment = {"phase": "ready", "storage": {"attempt": 64}, "cache": {"attempt": 65},
                   "records": [failed]}
        with patch.object(generation, "load_state", return_value=state), \
                patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                patch("devex_clone_seed_segment.segmented_resume", return_value=segment) as verified:
            generation.preflight(self.directory, backend=self.backend)
        verified.assert_called_once_with(
            self.backend, self.directory, state["attempts"], archive, self.registration)

        for invalid in ({**segment, "phase": "storage-ready"},
                        {**segment, "storage": None}, {**segment, "cache": None},
                        {**segment, "records": []}):
            with patch.object(generation, "load_state", return_value=state), \
                    patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                    patch("devex_clone_seed_segment.segmented_resume", return_value=invalid), \
                    self.subTest(segment=invalid), self.assertRaisesRegex(ValueError, "续作边界"):
                generation.preflight(self.directory, backend=self.backend)

        for extra in (self.record(55, generation.START, status="failed"),
                      self.record(55, generation.STOP, status="running")):
            with patch.object(generation, "load_state", return_value={"attempts": [*state["attempts"], extra]}), \
                    patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                    self.assertRaisesRegex(ValueError, "只执行一次"):
                generation.preflight(self.directory, backend=self.backend)

    def test_preflight_binds_forensic_replay_request_before_new_attempt(self):
        failed = self.record(54, generation.START, status="failed")
        failed["error_type"] = "ValueError"
        state = {"attempts": self.prefix + [failed]}
        archive = {"records": [], "receipt": {"source_registration": self.registration}}
        segment = {
            "phase": "ready",
            "storage": {"attempt": 64},
            "cache": {"attempt": 65},
            "records": [failed],
            "replay": {"request": binding(self.request_path)},
        }
        with patch.object(generation, "load_state", return_value=state), \
                patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                patch("devex_clone_seed_segment.segmented_resume", return_value=segment):
            generation.preflight(
                self.directory, backend=self.backend, request_path=self.request_path
            )
            with self.assertRaisesRegex(ValueError, "C70"):
                generation.preflight(self.directory, backend=self.backend)
            changed = self.directory / "changed-request.json"
            write_json(changed, {**self.request, "id": "changed"})
            with self.assertRaisesRegex(ValueError, "C70"):
                generation.preflight(
                    self.directory, backend=self.backend, request_path=changed
                )

            swapped = self.directory / "swapped-request.json"
            write_json(swapped, {**self.request, "id": "wrong-before-forensic-check"})

            def replace_with_authorized(*_args):
                swapped.unlink()
                write_json(swapped, self.request)
                return segment

            with patch("devex_clone_seed_segment.segmented_resume",
                       side_effect=replace_with_authorized), \
                    self.assertRaisesRegex(ValueError, "C70"):
                generation.preflight(
                    self.directory, backend=self.backend, request_path=swapped
                )

    def test_forensic_replay_image_drift_blocks_runtime_start(self):
        old_rustfs = {"endpoint": "same", "pid": 6100}
        new_rustfs = {"endpoint": "same", "pid": 6500}
        old_redis = {"endpoint": "same", "pid": 6200}
        new_redis = {"endpoint": "same", "pid": 6600}
        old_storage = {"attempt": 62, "storage": old_rustfs}
        new_storage = {"attempt": 65, "storage": new_rustfs}
        previous = copy.deepcopy(self.request)
        previous["current_storage"] = old_storage
        previous_path = self.directory / "previous-request.json"
        write_json(previous_path, previous)
        self.request["current_storage"] = new_storage
        self.source["storage"]["storage"] = new_storage
        self.request_path.unlink()
        write_json(self.request_path, self.request)
        self.image["storage"] = {"rustfs": new_rustfs, "redis": new_redis}
        archive = {
            "receipt": {"request": binding(previous_path)},
            "image": {
                **copy.deepcopy(self.image),
                "storage": {"rustfs": old_rustfs, "redis": old_redis},
            },
        }
        replay_image = copy.deepcopy(self.image)
        replay_image["database"]["business"] = "changed-after-c69"
        segment = {
            "phase": "ready",
            "storage": new_storage,
            "cache": {"redis": new_redis},
            "records": [],
            "replay": {"request": binding(self.request_path), "image": replay_image},
        }
        with patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                patch("devex_clone_seed_segment.segmented_resume", return_value=segment), \
                self.assertRaisesRegex(ValueError, "C69 授权后完整逻辑像发生变化"):
            self.execute()
        self.runtime.start.assert_not_called()

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
        state["attempts"][-1]["sources"] = self.coordinator_source
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

    def test_live_verification_ignores_only_control_of_sealed_unstarted_ancestor(self):
        import json

        with self.running_fixture() as (descriptor, state, _verifier, _storage):
            failed = self.record(50, generation.START, status="failed")
            recovered = self.record(51, generation.RECOVER, self.rebound)
            state["attempts"][:0] = [failed, recovered]
            path = Path(descriptor["path"])
            value = read_json(path)
            prefix = state["attempts"][:-1]
            value.update(history_length=len(prefix), history_sha256=plan_hash(prefix))
            path.write_text(json.dumps(value), encoding="utf-8")
            descriptor = binding(path)
            state["attempts"][-1]["result"] = descriptor
            with patch("devex_clone_seed_generation_prelaunch.closed", return_value={"start": failed, "recovery": recovered}):
                generation.verify_running_source(self.backend, descriptor, live=True)
                state["attempts"].append(self.record(55, generation.RECOVER, self.rebound))
                with self.assertRaisesRegex(ValueError, "停止或恢复"):
                    generation.verify_running_source(self.backend, descriptor, live=True)

    def test_registered_start_tools_cannot_be_rebound_or_change_during_validation(self):
        with self.running_fixture() as (descriptor, state, _verifier, _storage):
            changed = copy.deepcopy(self.coordinator_source)
            changed["fingerprints"]["test_tools"]["sha256"] = "e" * 64
            with patch.object(source_fingerprints, "current_execution_source", side_effect=[self.coordinator_source, changed]), \
                    self.assertRaisesRegex(ValueError, "test_tools"):
                generation.verify_running_source(self.backend, descriptor, live=False)
            state["attempts"][-1]["sources"] = changed
            with patch.object(source_fingerprints, "current_execution_source", return_value=changed), \
                    self.assertRaisesRegex(ValueError, "完整前缀"):
                generation.verify_running_source(self.backend, descriptor, live=False)

    def test_registered_checkpoint_holds_one_run_lock_and_expires_after_exit(self):
        import os
        from process_environment import Environments, configured

        @contextmanager
        def lock(directory):
            path = directory / "run.lock"
            path.mkdir()
            try:
                yield
            finally:
                path.rmdir()

        facts = {"directory": self.directory, "same": "generation", "environment": {"environment": {}}}
        controller = {**os.environ, "CARGO_BUILD_JOBS": "4", "RUSTUP_HOME": "D:/fixture-rustup"}

        def verified(*_args, **_kwargs):
            self.assertEqual(dict(os.environ), controller)
            return facts

        with patch.dict(os.environ, controller, clear=True), patch.object(generation, "verify_running_source", side_effect=verified), \
                patch("devex_clone_run_state.run_lock", side_effect=lock) as acquire, \
                patch("devex_clone_run._require_owned_run") as owner:
            with generation.registered_running_source(self.backend, self.registration) as checkpoint:
                self.assertTrue((self.directory / "run.lock").is_dir())
                self.assertEqual(checkpoint(), facts)
                service = configured({})
                with Environments(service, {}).use("source"):
                    self.assertEqual(checkpoint(), facts)
                    self.assertEqual(dict(os.environ), service)
            acquire.assert_called_once_with(self.directory)
            self.assertGreaterEqual(owner.call_count, 3)
            with self.assertRaisesRegex(ValueError, "离开控制区间"):
                checkpoint()

    def test_historical_generation_does_not_relax_ordinary_current_source_check(self):
        with self.running_fixture() as (descriptor, _state, _verifier, _storage):
            changed = copy.deepcopy(self.coordinator_source)
            changed["snapshot"]["head"] = "e" * 40
            with patch.object(source_fingerprints, "current_execution_source", return_value=changed):
                with self.assertRaisesRegex(ValueError, "test_tools"):
                    generation.verify_running_source(self.backend, descriptor, live=False)
                facts = generation.verify_historical_running_source(self.backend, descriptor, live=False)
            self.assertEqual(facts["coordinator_source"], self.coordinator_source)

    def test_historical_recovery_rejects_dirty_successor_before_lock(self):
        dirty = copy.deepcopy(self.coordinator_source)
        dirty["snapshot"]["clean"] = False
        with patch.object(generation, "verify_historical_running_source", return_value={}), \
                patch.object(generation, "current_execution_source", return_value=dirty), \
                patch("devex_clone_run_state.run_lock") as lock:
            with self.assertRaisesRegex(ValueError, "先提交为干净来源"):
                with generation.registered_historical_running_source(self.backend, self.registration):
                    self.fail("未提交恢复工具不能进入控制区间")
        lock.assert_not_called()

    def test_historical_recovery_pins_successor_until_lock_release(self):
        @contextmanager
        def lock(directory):
            path = directory / "run.lock"
            path.mkdir()
            try:
                yield
            finally:
                path.rmdir()

        facts = {"directory": self.directory, "environment": {"environment": {}}}
        with patch.object(generation, "verify_historical_running_source", return_value=facts), \
                patch.object(generation, "current_execution_source", return_value=self.coordinator_source), \
                patch("devex_clone_run_state.run_lock", side_effect=lock), \
                patch("devex_clone_run._require_owned_run"):
            with generation.registered_historical_running_source(self.backend, self.registration) as (checkpoint, tools):
                self.assertEqual(tools, self.coordinator_source)
                self.assertEqual(checkpoint(), facts)
                changed = copy.deepcopy(self.coordinator_source)
                changed["fingerprints"]["test_tools"]["sha256"] = "e" * 64
                with patch.object(source_fingerprints, "current_execution_source", return_value=changed), \
                        self.assertRaisesRegex(ValueError, "test_tools"):
                    checkpoint()
            with self.assertRaisesRegex(ValueError, "离开控制区间"):
                checkpoint()


if __name__ == "__main__":
    unittest.main()
