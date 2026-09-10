"""已发布 seed 源的严格来源恢复与当前存储核验。"""
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
                patch.object(source, "initialization_history", return_value=({}, seed_target)), \
                patch.object(source, "request_binding") as ready, \
                patch.object(source, "current_storage_binding", return_value=storage):
            observed = source.published_source(self.backend, descriptor, live_storage=True)
        ready.assert_called_once_with(self.backend, seed_target)
        self.assertEqual(observed["seed_target"], seed_target)
        self.assertEqual(observed["storage"]["storage"], storage)
        self.assertEqual(observed["registration"]["source_request"], observed["result"]["source_request"])

        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "validate_request"), \
                patch.object(source, "initialization_history", return_value=({}, seed_target)), \
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
                patch.object(source, "initialization_history", return_value=({}, seed_target)), \
                patch.object(source, "request_binding"), \
                patch.object(source, "current_storage_binding", return_value=storage), \
                self.assertRaises(ValueError):
            source.published_source(self.backend, descriptor, live_storage=True)


if __name__ == "__main__":
    unittest.main()
