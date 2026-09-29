"""新目标创建/初始化的离线回归；真实 MySQL、S3、Redis 仍由独立验收证明。"""
import copy
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_target as target
import devex_clone_target_binding as binding
import devex_clone_target_initialization_claim as initialization_claim
import devex_clone_target_resume as target_resume
import devex_clone_target_resume_evidence as target_evidence
import devex_clone_target_runtime_evidence as runtime_evidence
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding as receipt_binding
from devex_clone_target_resources import Resources
from devex_clone_target_state import (generation_checkpoint, generation_guard_checkpoint,
                                      generation_lock)
from devex_clone_target_storage import verify_storage_generation
from devex_clone_target_fixture import Fixture
from devex_clone_inventory import InventoryCaptureError
from restore_build import file_digest
from restore_reference_plan import plan_hash
from workspace_directory import WorkspaceDirectory


class StorageTransitionTests(unittest.TestCase):
    def test_fixture_restart_generation_requires_exact_joint_binding(self):
        generation = {"path": "generation", "bytes": 1, "sha256": "a" * 64}
        storage = {"storage": "new-files", "data_directory": "directory",
                   "api_url": "api", "console_url": "console", "generation": generation}
        cache = {"redis": "new-cache", "generation": generation}
        self.assertEqual(runtime_evidence.fixture_restart_generation(storage, cache), generation)
        cases = ((storage, None),
                 (storage, {**cache, "generation": {**generation, "bytes": 2}}),
                 ({**storage, "unexpected": True}, cache))
        for files, memory in cases:
            with self.subTest(files=files, memory=memory), self.assertRaises(ValueError):
                runtime_evidence.fixture_restart_generation(files, memory)

    def test_only_registered_rustfs_identity_may_change(self):
        before = {"configuration": "fixed", "storage": {"rustfs": {"identity": "old"}, "redis": "fixed"}}
        runtime = {"storage": {"identity": "new"}}
        after = {**before, "storage": {**before["storage"], "rustfs": runtime["storage"]}}
        verify_storage_generation(before, before, None, None)
        verify_storage_generation(before, after, runtime, None)
        self.assertEqual(before["storage"]["rustfs"], {"identity": "old"})
        for observed, transition in ((after, None), ({**after, "configuration": "changed"}, runtime),
                                  ({**after, "storage": {**after["storage"], "redis": "changed"}}, runtime)):
            with self.subTest(observed=observed, transition=transition), self.assertRaises(ValueError):
                verify_storage_generation(before, observed, transition, None)

    def test_independent_cache_transition_does_not_authorize_other_changes(self):
        before = {"configuration": "fixed", "storage": {"rustfs": "old-files", "redis": "old-cache"}}
        cache = {"redis": "new-cache"}
        after = {**before, "storage": {**before["storage"], "redis": cache["redis"]}}
        verify_storage_generation(before, after, None, cache)
        both = {**after, "storage": {**after["storage"], "rustfs": "new-files"}}
        verify_storage_generation(before, both, {"storage": "new-files"}, cache)
        for observed, files, memory in ((after, None, None), (both, None, cache),
                (both, {"storage": "new-files"}, None), ({**after, "configuration": "changed"}, None, cache)):
            with self.subTest(observed=observed, files=files, memory=memory), self.assertRaises(ValueError):
                verify_storage_generation(before, observed, files, memory)
        self.assertEqual(before["storage"], {"rustfs": "old-files", "redis": "old-cache"})

    def test_started_generation_rejects_jointly_forged_fixture_runtime(self):
        expected = {"configuration": "fixed", "storage": {"rustfs": "old-files",
                                                              "redis": "old-cache"}}
        generation = {"path": "generation", "bytes": 1, "sha256": "a" * 64}
        trusted_storage = {"storage": "new-files", "data_directory": "directory",
                           "api_url": "api", "console_url": "console", "generation": generation}
        trusted_cache = {"redis": "new-cache", "generation": generation}
        forged_storage = {**trusted_storage, "storage": "forged-files"}
        forged_cache = {**trusted_cache, "redis": "forged-cache"}
        actual_storage = {"rustfs": forged_storage["storage"], "redis": forged_cache["redis"]}
        started = {"storage": actual_storage, "storage_runtime": forged_storage,
                   "cache_runtime": forged_cache,
                   "current_generation_sha256": plan_hash({**expected, "storage": actual_storage})}
        with patch.object(runtime_evidence, "_fixture_runtime_evidence",
                          return_value=(trusted_storage, trusted_cache)), self.assertRaisesRegex(
                ValueError, "不同于可信发布证据"):
            runtime_evidence.validate_started_generation(Path.cwd(), expected, started)


class TargetTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(
            Path(__file__).resolve().parents[2] / ".local-tests/python-unit",
            prefix="fresh-",
        )
        self.addCleanup(temporary.cleanup)
        self.f = Fixture(Path(temporary.name), self)

    def prepare(self):
        f = self.f
        result = target.prepare_target(f.root, f.path, f.output, f.run)
        f.publish_prepared_files()
        return result

    def initialize(self):
        f = self.f
        publisher = f.publish_initialized_files if (f.local / "prepared-files.json").is_file() else None
        result = target.initialize_target(f.root, f.output, f.run, publish_files=publisher)
        return result

    def test_device_fixture_execution_root_is_bound_to_its_generated_snapshot(self):
        f = self.f
        execution = f.local / "device-backend"
        execution.mkdir()
        (execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        (execution / ".git").write_text("gitdir: fixture", encoding="utf-8")
        generated = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": []}
        fixture = {
            "format_version": 1,
            "fixture": "device",
            "status": "ready",
            "paths": {"backend": str(execution), "frontend": str(f.local / "device-frontend")},
            "generated": {"backend": generated},
        }
        receipt = f.bound(f.local / "device-fixture.json", fixture)
        f.request["execution_backend"] = {"fixture": receipt, "path": str(execution)}

        with patch.object(binding, "snapshot", return_value=(generated, b"")):
            root, evidence = binding.execution_backend(f.root, f.request)

        self.assertEqual(root, execution)
        self.assertEqual(evidence["kind"], "device-fixture")
        self.assertEqual(evidence["source"], generated)

    def test_complete_lifecycle_uses_only_bound_cli_then_readonly_verifies(self):
        f = self.f
        prepared = self.prepare()
        self.assertEqual(prepared["status"], "fresh_creation_prepared")
        self.assertFalse(f.databases)
        self.assertEqual(f.plan_count, 0)
        result = self.initialize()
        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertEqual(len(f.databases), 4)
        self.assertFalse((f.output / "initialize.lock").exists())
        self.assertFalse(result["target_ready"])
        self.assertFalse(result["restore_qualified"])
        commands = [command for command in f.calls if Path(command[0]).stem in ("reset", "migrate")]
        self.assertEqual([c[1] for c in commands[:3]], ["plan", "plan", "execute"])
        self.assertEqual(len([c for c in commands if Path(c[0]).stem == "migrate"]), 10)
        self.assertFalse(any(name.startswith("migrate-") for name in result["history"]))
        self.assertNotIn("--all", [part for command in commands for part in command])
        verified = target.verify_target(f.root, f.output, f.local / "verify", f.run)
        self.assertEqual(verified["remote_writes"], 0)
        self.assertEqual(f.plan_count, 2)

    def test_scope_or_missing_target_rejected_before_resource_call(self):
        for field in ("scope", "database"):
            with self.subTest(field=field):
                value = copy.deepcopy(self.f.request)
                if field == "scope": value["target"]["scope_id"] = "other-scope"
                else: value["target"]["databases"].pop()
                with self.assertRaises(ValueError): binding.request_binding(self.f.root, value)
        self.assertFalse(self.f.calls)

    def test_execution_binary_bindings_cover_all_native_fresh_dependencies(self):
        f = self.f
        f.review["tools"]["wsl"] = {
            "path": str(f.paths["wsl"]),
            "sha256": file_digest(f.paths["wsl"])["sha256"],
        }
        f.request["review"] = {
            **f.bound(f.local / "review.json", f.review),
            "canonical_sha256": plan_hash(f.review),
        }
        f.save_request()

        actual = binding.execution_binary_bindings(f.root, f.request)

        self.assertEqual(
            {Path(item["path"]).name for item in actual},
            {"mysql.exe", "aws.exe", "reset.exe", "migrate.exe", "tenant-data.exe", "rustfs.exe", "wsl.exe"},
        )
        self.assertTrue(all(set(item) == {"path", "sha256"} for item in actual))

    def test_execution_binary_bindings_reject_storage_tool_drift(self):
        self.f.review["tools"]["wsl"] = dict(self.f.request["storage"]["redis"]["wsl"])
        self.f.request["storage"]["redis"]["wsl"]["sha256"] = "0" * 64
        self.f.request["review"] = {
            **self.f.bound(self.f.local / "review.json", self.f.review),
            "canonical_sha256": plan_hash(self.f.review),
        }
        self.f.save_request()

        with self.assertRaisesRegex(ValueError, "不属于审阅版本"):
            binding.execution_binary_bindings(self.f.root, self.f.request)

    def test_unready_review_prevents_prepare(self):
        f = self.f
        f.review["ready_for_execution"] = False
        f.request["review"] = {
            **f.bound(f.local / "review.json", f.review),
            "canonical_sha256": plan_hash(f.review),
        }
        f.save_request()
        with self.assertRaisesRegex(ValueError, "尚未就绪"):
            self.prepare()
        self.assertFalse(f.calls)

    def test_incomplete_review_prevents_resource_operations(self):
        f = self.f
        f.review["services"].pop("redis")
        f.request["review"] = {
            **f.bound(f.local / "review.json", f.review),
            "canonical_sha256": plan_hash(f.review),
        }
        f.save_request()
        with self.assertRaisesRegex(ValueError, "服务定义无效"):
            self.prepare()
        self.assertFalse(f.calls)

    def test_failed_producer_launch_intent_without_pid_blocks_fresh_prepare(self):
        runtime = Path(self.f.review["scopes"]["seed"]["runtime_dir"])
        runtime.mkdir()
        history = runtime / "producer-history.json"
        history.write_text(json.dumps({"events": [{"event": "start-intent", "role": "worker"}]}), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "启动意图"):
            self.prepare()
        self.assertFalse(self.f.calls)
        self.assertFalse((runtime / "worker.json").exists())
        self.assertTrue(history.exists())

    def test_launch_history_after_initialization_blocks_fresh_reverification(self):
        self.prepare()
        self.initialize()
        runtime = Path(self.f.review["scopes"]["seed"]["runtime_dir"])
        runtime.mkdir()
        (runtime / "producer-history.json").write_text('{"events":[{"event":"start-intent"}]}', encoding="utf-8")
        self.f.calls.clear()
        with self.assertRaisesRegex(ValueError, "启动意图"):
            target.verify_target(self.f.root, self.f.output, self.f.local / "verify", self.f.run)
        self.assertFalse(self.f.calls)

    def test_database_collision_with_reference_rejected(self):
        f = self.f
        collision = f.review["reference"]["source"]["databases"][0]["database"]
        f.review["scopes"]["base"]["databases"][0]["database"] = collision
        f.request["review"] = {**f.bound(f.local / "review.json", f.review), "canonical_sha256": plan_hash(f.review)}
        with self.assertRaises(ValueError): binding.request_binding(f.root, f.request)

    def test_existing_database_or_prefix_prevents_prepare(self):
        f = self.f
        f.databases.add(f.request["target"]["databases"][0]["database"])
        with self.assertRaises(ValueError): self.prepare()
        self.assertTrue((f.output / "failure.json").is_file())
        self.assertEqual(len(f.databases), 1)

    def test_nonempty_scoped_prefix_is_not_reset(self):
        self.f.extra_objects["uploads"] = [self.f.scope + "/old"]
        with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(self.f.databases)

    def test_redis_old_scope_value_is_not_adopted(self):
        f = self.f
        f.redis_values[f.review["scopes"]["seed"]["redis"]["namespace"] + "old-lock"] = "old"
        with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(f.databases)

    def test_prepare_then_config_change_refuses_all_writes(self):
        self.prepare()
        with patch.dict(os.environ, {"APP_REDIS_PASSWORD": "changed"}):
            with self.assertRaises(ValueError): self.initialize()
        self.assertFalse(self.f.databases)

    def test_binary_change_after_prepare_refuses_writes(self):
        self.prepare()
        self.f.paths["mysql"].write_bytes(b"changed")
        with self.assertRaises(ValueError): self.initialize()
        self.assertFalse(self.f.databases)

    def test_changed_current_source_binding_refuses_writes(self):
        self.prepare()
        self.f.maintenance["source"]["worktree_fingerprint"] = "sha256:" + "f" * 64
        with self.assertRaises(ValueError): self.initialize()
        self.assertFalse(self.f.databases)

    def test_unknown_create_retains_intent_and_cannot_replay_or_cleanup(self):
        f = self.f
        self.prepare(); f.unknown_create = True
        with self.assertRaises(subprocess.TimeoutExpired): self.initialize()
        self.assertEqual(len(f.databases), 1)
        self.assertEqual(len(list(f.output.glob("create-*.intent.json"))), 1)
        self.assertFalse(list(f.output.glob("create-*.confirmed.json")))
        self.assertEqual(json.loads((f.output / "failure.json").read_text(encoding="utf-8"))["status"], "needs_reconciliation")
        calls = len(f.calls)
        with self.assertRaises(ValueError): self.initialize()
        self.assertEqual(len(f.calls), calls)
        self.assertEqual(len(f.databases), 1)
        diagnostics = "".join(p.read_text(encoding="utf-8") for p in f.output.glob("*.command.json"))
        self.assertNotIn("db-secret", diagnostics)

    def test_new_resource_appearing_after_prepare_is_not_adopted(self):
        self.prepare()
        self.f.databases.add(self.f.request["target"]["databases"][0]["database"])
        with self.assertRaises(ValueError): self.initialize()
        self.assertFalse((self.f.output / "initialize.started.json").exists())

    def test_reset_plan_changed_never_executes(self):
        f = self.f
        self.prepare(); f.plan_changed = True
        with self.assertRaises(ValueError): self.initialize()
        self.assertEqual(len(f.databases), 4)
        self.assertFalse(f.initialized)

    def test_reused_reset_is_not_fresh_success(self):
        self.prepare(); self.f.reset_status = "reused"
        with self.assertRaises(ValueError): self.initialize()
        self.assertFalse((self.f.output / "initialized.json").exists())
        self.assertTrue((self.f.output / "reset.intent.json").exists())

    def test_storage_restart_after_prepare_refuses_create(self):
        self.prepare(); self.f.storage_restarted = True
        with self.assertRaises(ValueError): self.initialize()
        self.assertFalse(self.f.databases)

    def test_registered_rustfs_restart_verifies_original_target_without_rewriting_history(self):
        f = self.f
        self.prepare()
        initial = self.initialize()
        before = file_digest(f.output / "initialized.json")
        original = copy.deepcopy(f.request["storage"]["rustfs"])
        restarted = f.storage_dir / "restarted"
        restarted.mkdir()
        f.identity = {**f.identity, "pid": 101, "started": "54321"}
        process = json.loads(Path(original["process_receipt"]["path"]).read_text(encoding="utf-8"))
        launch = json.loads(Path(original["launch_receipt"]["path"]).read_text(encoding="utf-8"))
        process["identity"] = launch["identity"] = f.identity
        storage = {"identity": f.identity, "sha256": original["sha256"],
                   "process_receipt": f.bound(restarted / "process.json", process),
                   "launch_receipt": f.bound(restarted / "launch.json", launch)}
        data = Path(process["data_dir"])
        runtime = {"storage": storage, "data_directory": {"path": str(data), "device": data.stat().st_dev,
                   "inode": data.stat().st_ino}, "api_url": process["api_url"], "console_url": process["console_url"],
                   "request": f.bound(restarted / "request.json", {"previous": original})}
        with patch("devex_clone_storage.registered_storage_binding", return_value=runtime), \
                patch("devex_clone_cache.registered_cache_binding", return_value=None):
            observed = target.verify_target(f.root, f.output, f.local / "restarted-observation", f.run,
                                             storage_run=f.local)
        self.assertEqual(observed["initialized"]["sha256"], before["sha256"])
        self.assertEqual(file_digest(f.output / "initialized.json"), before)
        self.assertEqual(initial["generation"]["storage"]["rustfs"], original)
        runtime["data_directory"]["inode"] += 1
        with patch("devex_clone_storage.registered_storage_binding", return_value=runtime), self.assertRaises(ValueError):
            target.verify_target(f.root, f.output, f.local / "wrong-storage-observation", f.run, storage_run=f.local)

    def test_rustfs_different_actual_data_root_refuses_prepare(self):
        self.f.actual_argv[-1] = str(self.f.local / "wrong-data")
        with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(self.f.databases)

    def test_registered_cache_restart_preserves_initialization_and_checks_live_identity(self):
        f = self.f
        self.prepare(); initial = self.initialize()
        before = file_digest(f.output / "initialized.json")
        original = copy.deepcopy(f.request["storage"]["redis"])
        restarted = {**original, "pid": 201, "started": "70000", "run_id": "5" * 40}
        proof = {"redis": restarted, "request": f.bound(f.local / "cache-request.json", {"previous": original})}
        def observe(command, **kwargs):
            if Path(command[0]).stem == "wsl" and "/usr/bin/cat" in command:
                return subprocess.CompletedProcess(command, 0, b"201 (redis-server) " + b" ".join([b"S"] + [b"0"] * 18 + [b"70000"]), b"")
            return f.run(command, **kwargs)
        def redis(args):
            if args == ["INFO", "server"]:
                return "process_id:201\r\nrun_id:" + "5" * 40 + "\r\nconfig_file:/fixture/redis.conf"
            return f.redis(args)
        with patch("devex_clone_storage.registered_storage_binding", return_value=None), \
                patch("devex_clone_cache.registered_cache_binding", return_value=proof), \
                patch.object(Resources, "_redis", side_effect=redis):
            observed = target.verify_target(f.root, f.output, f.local / "cache-observation", observe, storage_run=f.local)
            self.assertEqual(observed["initialized"]["sha256"], before["sha256"])
            restarted["run_id"] = "6" * 40
            with self.assertRaises(ValueError):
                target.verify_target(f.root, f.output, f.local / "wrong-cache", observe, storage_run=f.local)
            restarted["run_id"] = "5" * 40
            for field in ("configuration", "wsl", "distribution", "port", "executable", "sha256"):
                valid = restarted[field]
                restarted[field] = "drift"
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, "缓存恢复不能改变"):
                    target.verify_target(f.root, f.output, f.local / ("cache-" + field), observe, storage_run=f.local)
                restarted[field] = valid
        self.assertEqual(file_digest(f.output / "initialized.json"), before)
        self.assertEqual(initial["generation"]["storage"]["redis"], original)

    def test_redis_effective_directory_different_refuses_prepare(self):
        self.f.redis_config["dir"] = "/other"
        with self.assertRaises(ValueError): self.prepare()

    def test_published_initialization_keeps_content_checks_without_fresh_workspace(self):
        import devex_clone_factory_context as factory_context
        from devex_clone_factory_context import initialization_history
        from devex_clone_seed_source import _published_initialization

        f = self.f
        self.prepare()
        initial = self.initialize()
        descriptor = receipt_binding(f.output / "initialized.json")
        for name in ("registration.json", "prepared-files.json", "initialized-files.json"):
            (f.local / name).unlink()
        before = binding.target_files(f.output)
        # 本文件的库存替身只覆盖目标协议；完整库存内容有独立回归，仍核对调用未被跳过。
        with patch.object(factory_context, "inventory_history") as inventory:
            observed, request = _published_initialization(f.root, {"initialized": descriptor})
        inventory.assert_called_once_with(f.root, f.output, initial["inventory"], f.request, resumed=False)
        self.assertEqual((observed, request), (initial, f.request))
        self.assertEqual(before, binding.target_files(f.output))
        with self.assertRaises((FileNotFoundError, ValueError)):
            initialization_history(f.root, f.output / "initialized.json")
        for name in ("initialized-candidate.json", "sentinel.confirmed.json"):
            path = f.output / name
            original = path.read_bytes()
            path.write_text("{}", encoding="utf-8")
            with self.subTest(name=name), self.assertRaises(ValueError):
                _published_initialization(f.root, {"initialized": descriptor})
            path.write_bytes(original)
        with patch.object(factory_context, "inventory_history", side_effect=ValueError("inventory changed")), \
                self.assertRaisesRegex(ValueError, "inventory changed"):
            _published_initialization(f.root, {"initialized": descriptor})

    def test_explicit_exclusive_must_match_actual_configuration(self):
        with patch.dict(os.environ, {"APP_RESET_LEGACY_MYSQL_EXCLUSIVE": "false"}):
            with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(self.f.databases)

    def test_existing_sentinel_never_overwritten(self):
        self.f.redis_values[self.f.request["reset"]["sentinel_key"]] = "existing"
        with self.assertRaises(ValueError): self.prepare()
        self.assertEqual(self.f.redis_values[self.f.request["reset"]["sentinel_key"]], "existing")

    def test_empty_runtime_directory_does_not_count_as_start_history(self):
        runtime = Path(self.f.review["scopes"]["seed"]["runtime_dir"])
        runtime.mkdir()
        self.prepare()
        self.assertTrue(runtime.is_dir())
        self.assertFalse(any(runtime.iterdir()))

    def test_runtime_history_or_unclosed_port_fails(self):
        f = self.f
        runtime = Path(f.review["scopes"]["seed"]["runtime_dir"])
        runtime.mkdir()
        (runtime / "unknown.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(f.databases)

    def test_unknown_port_status_refuses_prepare(self):
        with patch.object(binding, "require_closed_port", side_effect=ValueError("permission denied")):
            with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(self.f.databases)

    def test_existing_output_is_not_overwritten(self):
        self.prepare()
        previous = file_digest(self.f.output / "prepare.json")
        with self.assertRaises(ValueError): self.prepare()
        self.assertEqual(previous, file_digest(self.f.output / "prepare.json"))

    def test_interrupted_readonly_prepare_resumes_without_replacing_old_evidence(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interrupted")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        original_failure = file_digest(f.output / "failure.json")
        original_request = file_digest(f.output / "request.json")

        with patch.object(target, "context", side_effect=OSError("fixture resume interrupted")), \
                self.assertRaises(OSError):
            target.resume_prepare_target(
                f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor
            )
        self.assertEqual(len(list(f.output.glob("resume-prepare-*.failure.json"))), 1)

        result = target.resume_prepare_target(
            f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor
        )

        self.assertEqual(result["status"], "fresh_creation_prepared")
        self.assertEqual(file_digest(f.output / "failure.json"), original_failure)
        self.assertEqual(file_digest(f.output / "request.json"), original_request)
        self.assertEqual(len(list(f.output.glob("resume-prepare-*.intent.json"))), 2)
        self.assertEqual(len(list(f.output.glob("resume-prepare-*.confirmed.json"))), 1)
        self.assertFalse(target.unresolved_failure(f.root, f.output))
        self.assertEqual(self.initialize()["status"], "fresh_target_initialized")

    def test_prepare_resume_rejects_invalid_timestamp(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interrupted")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        target.resume_prepare_target(
            f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor
        )
        intent = next(f.output.glob("resume-prepare-*.intent.json"))
        value = read_json(intent)
        value["at"] = "fixture"
        intent.write_text(json.dumps(value), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "intent 无效"):
            target.prepare_resume_state(f.root, f.output, descriptor)

    def test_prepare_resume_rejects_result_before_intent(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interrupted")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        target.resume_prepare_target(
            f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor
        )
        intent = next(f.output.glob("resume-prepare-*.intent.json"))
        confirmed = next(f.output.glob("resume-prepare-*.confirmed.json"))
        intent_value = read_json(intent)
        intent_value["at"] = "2030-01-02T00:00:00+00:00"
        intent.write_text(json.dumps(intent_value), encoding="utf-8")
        confirmed_value = read_json(confirmed)
        confirmed_value["at"] = "2030-01-01T00:00:00+00:00"
        confirmed_value["intent"] = receipt_binding(intent)
        confirmed.write_text(json.dumps(confirmed_value), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "不属于原 intent"):
            target.prepare_resume_state(f.root, f.output, descriptor)

    def test_prepare_resume_rejects_any_resource_write_intent(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interrupted")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        for name in ("create-shared-control.intent.json", "sentinel.intent.json", "reset.intent.json"):
            with self.subTest(name=name):
                path = f.output / name
                path.write_text('{}', encoding="utf-8")
                calls = len(f.calls)
                with self.assertRaisesRegex(ValueError, "资源写入 intent"):
                    target.resume_prepare_target(
                        f.root, f.output, f.run, storage_run=None,
                        request_descriptor=descriptor,
                    )
                self.assertEqual(len(f.calls), calls)
                path.unlink()

    def test_lock_conflict_does_not_poison_or_clean_other_owner(self):
        self.prepare()
        with generation_lock(self.f.output):
            with self.assertRaises(FileExistsError): self.initialize()
            self.assertTrue((self.f.output / "initialize.lock").exists())
            self.assertFalse((self.f.output / "failure.json").exists())
        self.assertFalse(self.f.databases)

    def test_lock_release_failure_prevents_publishing_and_preserves_local_lock(self):
        self.prepare()
        with patch.object(Path, "rmdir", side_effect=OSError("busy")):
            with self.assertRaises(OSError): self.initialize()
        self.assertTrue((self.f.output / "initialized.json").exists())
        self.assertTrue((self.f.output / "initialized-candidate.json").exists())
        self.assertTrue((self.f.output / "initialize.lock").exists())
        self.assertFalse((self.f.local / "initialized-files.json").exists())
        self.assertTrue(target.unresolved_failure(self.f.root, self.f.output))

    def test_publication_runs_after_initialize_unlock_while_target_guard_is_held(self):
        self.prepare()
        observations = []

        def publish(files):
            self.assertFalse((self.f.output / "initialize.lock").exists())
            observations.append(generation_guard_checkpoint(self.f.output))
            with self.assertRaisesRegex(ValueError, "初始化锁身份变化"):
                generation_checkpoint(self.f.output)
            self.f.publish_initialized_files(files)

        result = target.initialize_target(
            self.f.root, self.f.output, self.f.run, publish_files=publish
        )

        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertEqual(len(observations), 1)
        self.assertTrue((self.f.local / "initialized-files.json").is_file())

    def test_locked_target_scan_checks_guard_before_and_after_walk(self):
        self.prepare()
        with generation_lock(self.f.output):
            with patch("devex_clone_target_state.generation_checkpoint") as checkpoint:
                files = binding.target_files(
                    self.f.output, ignored={"initialize.lock"}, locked_guard=True
                )

        self.assertTrue(files)
        self.assertEqual(checkpoint.call_count, 2)

    def test_resource_command_rejects_lost_generation_before_runner(self):
        f = self.f
        selected = f.review["scopes"]["seed"]
        resource = Resources(
            f.root, f.request, f.output, selected, f.review, f.run,
            owned_lock_identity=123, lock_output=f.output
        )
        calls = len(f.calls)

        with patch("devex_clone_target_state.generation_checkpoint",
                   side_effect=ValueError("lost")), self.assertRaisesRegex(ValueError, "lost"):
            resource.command("guard-check", [str(f.paths["mysql"])])

        self.assertEqual(len(f.calls), calls)

    def test_prepare_tamper_after_baseline_is_rejected_before_resource_writes(self):
        f = self.f
        self.prepare()
        original_context = target.context

        def tamper(*args, **kwargs):
            result = original_context(*args, **kwargs)
            prepared = read_json(f.output / "prepare.json")
            prepared["controlled_generation_only"] = False
            (f.output / "prepare.json").write_text(
                json.dumps(prepared, ensure_ascii=False), encoding="utf-8")
            return result

        with patch.object(target, "context", side_effect=tamper), \
                self.assertRaisesRegex(ValueError, "prepare"):
            self.initialize()
        self.assertFalse(f.databases)
        self.assertFalse((f.output / "initialize.started.json").exists())
        self.assertFalse((f.local / "initialized-files.json").exists())

    def test_unknown_file_before_final_tree_snapshot_prevents_publication(self):
        f = self.f
        self.prepare()
        original_validate = initialization_claim.validate_complete

        def inject(*args, **kwargs):
            (f.output / "unknown-before-final.txt").write_text("unknown", encoding="utf-8")
            return original_validate(*args, **kwargs)

        with patch.object(initialization_claim, "validate_complete", side_effect=inject), \
                self.assertRaisesRegex(ValueError, "未登记"):
            self.initialize()
        self.assertTrue((f.output / "initialized.json").exists())
        self.assertFalse((f.local / "initialized-files.json").exists())

    def test_failed_publication_candidate_is_not_verified(self):
        self.prepare(); self.initialize()
        (self.f.output / "failure.json").write_text('{}', encoding="utf-8")
        with self.assertRaises(ValueError): target.verify_target(self.f.root, self.f.output, self.f.local / "verify", self.f.run)

    def test_missing_creation_history_cannot_be_reverified(self):
        self.prepare(); self.initialize()
        (self.f.output / "create-shared.confirmed.json").unlink()
        with self.assertRaises(ValueError): target.verify_target(self.f.root, self.f.output, self.f.local / "verify", self.f.run)

    def test_post_copy_data_or_objects_cannot_use_initial_verifier(self):
        self.prepare(); self.initialize()
        self.f.extra_objects["uploads"] = [self.f.scope + "/copied-object"]
        with self.assertRaises(ValueError): target.verify_target(self.f.root, self.f.output, self.f.local / "verify", self.f.run)

    def test_mysql_wrong_uuid_is_not_absent(self):
        self.f.uuid = "different-server"
        with self.assertRaises(ValueError): self.prepare()
        self.assertFalse(self.f.databases)

    def test_s3_denied_is_not_absence_and_arbitrary_credential_names_are_redacted(self):
        f = self.f
        f.request["target"]["s3"].update(access_key_env="AUTH_ACCESS", secret_key_env="AUTH_PRIVATE")
        f.save_request()
        def denied(command, **kwargs):
            if Path(command[0]).stem == "aws":
                raise subprocess.CalledProcessError(1, command, stderr=b"403 access secret")
            return f.run(command, **kwargs)
        with patch.dict(os.environ, {"AUTH_ACCESS": "access", "AUTH_PRIVATE": "secret"}):
            with self.assertRaises(subprocess.CalledProcessError): target.prepare_target(f.root, f.path, f.output, denied)
        self.assertFalse(f.databases)
        text = "".join(p.read_text(encoding="utf-8") for p in f.output.glob("*.command.json"))
        self.assertNotIn("403 access secret", text)
        self.assertIn("403 [REDACTED] [REDACTED]", text)

    def test_pending_release_refuses_fresh_even_if_cli_exits_zero(self):
        f = self.f
        self.prepare()
        def incomplete(command, **kwargs):
            result = f.run(command, **kwargs)
            if Path(command[0]).stem == "reset" and command[1] == "execute":
                for file in (f.output / "reset-state").glob("*.json"):
                    content = json.loads(file.read_text(encoding="utf-8"))
                    content["phases"]["release"]["status"] = "running"
                    file.write_text(json.dumps(content), encoding="utf-8")
            return result
        with self.assertRaises(ValueError): target.initialize_target(f.root, f.output, incomplete)
        self.assertFalse((f.output / "initialized.json").exists())

    def test_resp_errors_and_partial_bulk_are_not_absence(self):
        for payload in (b"-NOAUTH denied\r\n", b"$4\r\nab\r\n", b"+OK", b"*10001\r\n"):
            with self.subTest(payload=payload):
                with self.assertRaises(ValueError): Resources._response(io.BytesIO(payload))


if __name__ == "__main__":
    unittest.main()
