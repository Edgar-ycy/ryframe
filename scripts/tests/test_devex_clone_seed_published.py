"""已发布 seed 源的严格来源恢复与当前存储核验。"""
import json
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_seed_source as source
from devex_clone_capture import write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash


class PublishedSeedTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.directory = self.local / "source-run"
        (self.directory / "results").mkdir(parents=True)
        self.attempt = self.directory / "seed-runtime/attempt-0001"
        self.attempt.mkdir(parents=True)

    def file(self, path: Path, value: dict) -> dict:
        write_json(path, value)
        return binding(path)

    def published_fixture(self, *, tamper_source=False) -> tuple[dict, dict, dict]:
        target_config = {
            "scope_id": "seed-scope",
            "databases": [{"server_uuid": "server-a", "database": "seed_control"}],
            "s3": {"endpoint": "http://127.0.0.1:29200"},
        }
        maintenance = self.file(self.local / "maintenance.json", {"build": True})
        tools = {
            "mysql": self.file(self.local / "mysql.json", {"tool": "mysql"}),
            "aws": self.file(self.local / "aws.json", {"tool": "aws"}),
        }
        seed_target = {
            "side": "seed",
            "target": target_config,
            "maintenance_build": maintenance,
            "tools": tools,
        }
        initialized = self.file(self.local / "initialized.json", {"initialized": True})
        manifest = {
            "copy_stage": "source_to_seed",
            "initialized": initialized,
            "build_bridges": [],
        }
        run_manifest = self.file(self.directory / "manifest.json", manifest)
        handoff = self.file(self.attempt / "handoff.json", {"api_url": "http://127.0.0.1:18210"})
        request_source = {
            **target_config,
            "runtime_dir": str(self.directory / "seed-runtime/runtime"),
            "api_url": "http://127.0.0.1:18210",
        }
        if tamper_source:
            request_source["scope_id"] = "other-scope"
        request = {
            "source": request_source,
            "maintenance_build": maintenance,
            "tools": tools,
            "worktree_fingerprint": "product-a",
        }
        source_request = self.file(self.attempt / "source-request.json", request)
        storage_value = {
            "format_version": 1,
            "kind": "devex-clone-seed-source-storage",
            "run_directory": str(self.directory),
            "run_manifest": run_manifest,
            "side": "target",
            "storage": {"api_url": "http://127.0.0.1:29200", "generation": "seed-a"},
        }
        source_storage = self.file(self.attempt / "source-storage.json", storage_value)
        generation = {
            "request_sha256": plan_hash(request),
            "worktree_fingerprint": "product-a",
            "external_writers_discovered": False,
            "clone_verified": False,
            "restore_qualified": False,
        }
        generation_verified = self.file(self.attempt / "generation-verified.json", generation)
        source_environment = self.file(
            self.attempt / "source-environment.json", {"environment": {"SIDE": "seed"}}
        )
        registration = {
            "format_version": 1,
            "kind": "devex-clone-seed-source-registration",
            "run_directory": str(self.directory),
            "run_manifest": run_manifest,
            "source_to_seed": {"copy": True},
            "post_copy": {"post": True},
            "post_verify": {"verify": True},
            "seed_registration": {"seed": True},
            "seed_registration_stage": {"stage": True},
            "capacity": {"capacity": True},
            "departments": {"departments": True},
            "identity": {"identity": True},
            "seed_handoff": handoff,
            "seed_close": {"close": True},
            "producer_lineage": {"lineage": True},
            "source_storage": source_storage,
            "source_request": source_request,
            "generation_verified": generation_verified,
            "source_environment": source_environment,
            "remote_writes": 0,
            "outbox_drained": True,
            "restore_qualified": False,
        }
        registration_binding = self.file(self.attempt / "source-registration.json", registration)
        result = {
            "status": "seed_source_registered",
            "registration": registration_binding,
            "source_request": source_request,
            "source_storage": source_storage,
            "generation_verified": generation_verified,
            "remote_writes": 0,
            "outbox_drained": True,
            "restore_qualified": False,
        }
        descriptor = self.file(self.directory / "results/0001.json", result)
        return descriptor, seed_target, storage_value["storage"]

    def test_published_source_resolves_original_seed_and_live_storage(self):
        descriptor, seed_target, storage = self.published_fixture()
        state = {"attempts": [{"number": 1, "stage": "seed-runtime", "mode": "source-register",
                               "status": "passed", "result": descriptor}]}
        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "validate_request"), \
                patch.object(source, "_published_initialization", return_value=({}, seed_target)), \
                patch.object(source, "request_binding") as ready, \
                patch.object(source, "current_storage_binding", return_value=storage), \
                patch("devex_clone_seed_rebind.quiet_producers"):
            observed = source.published_source(self.backend, descriptor, live_storage=True)
        ready.assert_called_once_with(self.backend, seed_target)
        self.assertEqual(observed["seed_target"], seed_target)
        self.assertEqual(observed["storage"]["storage"], storage)
        self.assertEqual(observed["registration"]["source_request"], observed["result"]["source_request"])

        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "validate_request"), \
                patch.object(source, "_published_initialization", return_value=({}, seed_target)), \
                patch.object(source, "request_binding"), \
                patch.object(source, "current_storage_binding", return_value={"generation": "restarted"}), \
                self.assertRaises(ValueError):
            source.published_source(self.backend, descriptor, live_storage=True)

    def test_published_source_rejects_request_not_derived_from_seed_target(self):
        descriptor, seed_target, storage = self.published_fixture(tamper_source=True)
        state = {"attempts": [{"number": 1, "stage": "seed-runtime", "mode": "source-register",
                               "status": "passed", "result": descriptor}]}
        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "validate_request"), \
                patch.object(source, "_published_initialization", return_value=({}, seed_target)), \
                patch.object(source, "request_binding"), \
                patch.object(source, "current_storage_binding", return_value=storage), \
                self.assertRaises(ValueError):
            source.published_source(self.backend, descriptor, live_storage=True)

    def test_published_source_does_not_require_current_fresh_wrapper(self):
        descriptor, seed_target, _ = self.published_fixture()
        state = {"attempts": [{"number": 1, "stage": "seed-runtime", "mode": "source-register",
                               "status": "passed", "result": descriptor}]}
        initial = {"initialized": True}
        before = {path: path.read_bytes() for path in self.local.rglob("*") if path.is_file()}
        with patch.object(source, "load_state", return_value=state), patch.object(source, "validate_request"), \
                patch.object(source, "initialization_history", side_effect=ValueError("缺少当前 fresh wrapper")) as fresh, \
                patch("devex_clone_factory_context._initialization_evidence",
                      return_value=(initial, seed_target)) as core:
            observed = source._registered_source(self.backend, descriptor, live_storage=False,
                                                 validate_seed_target=lambda *_args: ({}, {}))
        fresh.assert_not_called()
        core.assert_called_once_with(self.backend, self.local / "initialized.json")
        self.assertEqual(set(observed), {"directory", "result", "registration", "request", "storage", "generation",
                                        "manifest", "seed_target", "initialization", "environment"})
        self.assertEqual(observed["initialization"], initial)
        self.assertEqual(observed["seed_target"], seed_target)
        self.assertEqual(before, {path: path.read_bytes() for path in self.local.rglob("*") if path.is_file()})

    def test_unpublished_or_wrong_result_is_rejected_before_initialization_core(self):
        descriptor, _, _ = self.published_fixture()
        passed = {"number": 1, "stage": "seed-runtime", "mode": "source-register",
                  "status": "passed", "result": descriptor}
        records = ([], [{**passed, "status": "failed"}],
                   [{**passed, "result": {**descriptor, "sha256": "0" * 64}}])
        for attempts in records:
            with self.subTest(attempts=attempts), patch.object(source, "load_state", return_value={"attempts": attempts}), \
                    patch.object(source, "_published_initialization") as core, self.assertRaises(ValueError):
                source._registered_source(self.backend, descriptor, live_storage=False,
                                          validate_seed_target=lambda *_args: ({}, {}))
            core.assert_not_called()

    def test_result_content_and_manifest_chain_are_rejected_before_initialization_core(self):
        descriptor, _, _ = self.published_fixture()
        result_path = Path(descriptor["path"])
        result = source.read_json(result_path)
        for update in ({"status": "not_published"}, {"outbox_drained": False}):
            result_path.write_text(json.dumps({**result, **update}), encoding="utf-8")
            changed = binding(result_path)
            state = {"attempts": [{"number": 1, "stage": "seed-runtime", "mode": "source-register",
                                   "status": "passed", "result": changed}]}
            with self.subTest(update=update), patch.object(source, "load_state", return_value=state), \
                    patch.object(source, "_published_initialization") as core, self.assertRaises(ValueError):
                source._registered_source(self.backend, changed, live_storage=False,
                                          validate_seed_target=lambda *_args: ({}, {}))
            core.assert_not_called()
        result_path.write_text(json.dumps(result), encoding="utf-8")
        changed = binding(result_path)
        state["attempts"][0]["result"] = changed
        (self.directory / "manifest.json").write_text('{"copy_stage":"other"}', encoding="utf-8")
        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "_published_initialization") as core, self.assertRaises(ValueError):
            source._registered_source(self.backend, changed, live_storage=False,
                                      validate_seed_target=lambda *_args: ({}, {}))
        core.assert_not_called()

    def test_published_initialization_checks_bound_result_before_core(self):
        filename = self.local / "initialized.json"
        descriptor = self.file(filename, {"initialized": True})
        filename.write_text('{"initialized":false}', encoding="utf-8")
        with patch("devex_clone_factory_context._initialization_evidence") as core, self.assertRaises(ValueError):
            source._published_initialization(self.backend, {"initialized": descriptor})
        core.assert_not_called()

    def test_published_initialization_rejects_result_or_tree_change_during_core(self):
        filename = self.local / "initialized.json"
        descriptor = self.file(filename, {"initialized": True})
        phase = self.local / "phase.json"
        self.file(phase, {"phase": "before"})
        initial_bytes, phase_bytes = filename.read_bytes(), phase.read_bytes()
        for target in (filename, phase):
            def mutate(*_args):
                target.write_text('{"changed":true}', encoding="utf-8")
                return {"initialized": True}, {"side": "seed"}
            with self.subTest(target=target.name), \
                    patch("devex_clone_factory_context._initialization_evidence", side_effect=mutate) as core, \
                    self.assertRaises(ValueError):
                source._published_initialization(self.backend, {"initialized": descriptor})
            core.assert_called_once_with(self.backend, filename)
            filename.write_bytes(initial_bytes)
            phase.write_bytes(phase_bytes)

    def test_published_initialization_does_not_fallback_after_core_rejects_evidence(self):
        filename = self.local / "initialized.json"
        descriptor = self.file(filename, {"initialized": True})
        with patch("devex_clone_factory_context._initialization_evidence", side_effect=ValueError("原初始化内容或阶段不符")), \
                patch.object(source, "initialization_history", side_effect=AssertionError("不得回退其他初始化路径")) as fresh, \
                self.assertRaisesRegex(ValueError, "原初始化内容或阶段"):
            source._published_initialization(self.backend, {"initialized": descriptor})
        fresh.assert_not_called()


if __name__ == "__main__":
    unittest.main()
