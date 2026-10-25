"""新目标创建/初始化的离线回归；真实 MySQL、S3、Redis 仍由独立验收证明。"""
import copy
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_target as target
import devex_clone_target_binding as binding
import devex_clone_target_tree_claim as initialization_claim
import devex_clone_target_runtime_evidence as runtime_evidence
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding as receipt_binding
from devex_clone_target_resources import Resources
from devex_clone_target_state import (generation_checkpoint, generation_guard_checkpoint,
                                      generation_lock)
from devex_clone_target_storage import verify_storage_generation
from devex_clone_target_fixture import Fixture
from restore_build import file_digest
from restore_reference_plan import plan_hash
from workspace_directory import WorkspaceDirectory
from full_stack_runtime import environment_scope, worker_ready_url


class FileConfigurationTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(
            Path(__file__).resolve().parents[2] / ".local-tests/python-unit", prefix="fresh-config-",
        )
        self.addCleanup(temporary.cleanup)
        self.f = Fixture(Path(temporary.name), self)

    def override(self, text):
        (self.f.configuration / "app.test.toml").write_text(text, encoding="utf-8")

    def test_worker_and_scope_use_selected_file(self):
        self.assertEqual(self.f.environment, {"APP_ENV": "test"})
        self.assertEqual(environment_scope(self.f.root, self.f.root / "config"), self.f.scope)
        self.assertEqual(worker_ready_url(self.f.root, self.f.root / "config"), "http://127.0.0.1:19210/readyz")
        self.override('[jobs]\nhealth_port=19211\n')
        self.assertEqual(worker_ready_url(self.f.root, self.f.root / "config"), "http://127.0.0.1:19211/readyz")

    def test_worker_errors_identify_invalid_field(self):
        for field, value in (("health_host", '"0.0.0.0"'), ("health_port", '"19210"'),
                             ("health_port", "true"), ("health_port", "65536")):
            with self.subTest(field=field, value=value):
                self.override(f'[jobs]\n{field}={value}\n')
                with self.assertRaisesRegex(ValueError, "jobs." + field):
                    worker_ready_url(self.f.root, self.f.root / "config")

    def test_jobs_mode_error_uses_file_value(self):
        self.override('[jobs]\nmode="embedded"\n')
        with self.assertRaisesRegex(ValueError, "jobs.mode"):
            environment_scope(self.f.root, self.f.root / "config")

    def test_digest_binds_only_common_and_selected_configuration(self):
        original = binding.configuration_digest(self.f.root, self.f.root / "config")
        (self.f.configuration / "app.prod.toml").write_text('scope_id="other-scope"\n', encoding="utf-8")
        self.assertEqual(binding.configuration_digest(self.f.root, self.f.root / "config"), original)
        self.override('[jobs]\nhealth_port=19211\n')
        self.assertNotEqual(binding.configuration_digest(self.f.root, self.f.root / "config"), original)
        (self.f.configuration / "app.test.toml").unlink()
        with self.assertRaises((ValueError, FileNotFoundError)):
            binding.configuration_digest(self.f.root, self.f.root / "config")

    def test_native_configuration_checks_use_the_same_selected_file_digest(self):
        selected = self.f.review["scopes"]["seed"]
        original = self.f.request["configuration_sha256"]
        unused = self.f.configuration / "app.prod.toml"
        unused.write_text('scope_id="unused"\n', encoding="utf-8")
        self.assertEqual(binding.configuration_digest(self.f.root, self.f.configuration), original)
        binding.redis_configuration(self.f.root, self.f.request, selected,
                                    environment={"APP_ENV": "test"})
        unused.write_text('scope_id="changed-unused"\n', encoding="utf-8")
        binding.redis_configuration(self.f.root, self.f.request, selected,
                                    environment={"APP_ENV": "test"})
        self.override('[jobs]\nhealth_port=19211\n')
        with self.assertRaisesRegex(ValueError, "摘要"):
            binding.redis_configuration(self.f.root, self.f.request, selected,
                                        environment={"APP_ENV": "test"})

    def test_redis_errors_identify_file_field(self):
        selected = self.f.review["scopes"]["seed"]
        for field, value in (("host", '"other-host"'), ("port", '"16390"'),
                             ("database", "true"), ("tls", "true")):
            with self.subTest(field=field):
                self.override(f'[redis]\n{field}={value}\n')
                with self.assertRaisesRegex(ValueError, "redis." + field):
                    binding.redis_configuration(self.f.root, self.f.request, selected,
                                                environment={"APP_ENV": "test"})

    def test_reset_errors_identify_file_field(self):
        selected = self.f.review["scopes"]["seed"]
        for field, value in (("legacy_mysql_exclusive", '"true"'),
                             ("credential_version", '"different"'),
                             ("redis_outside_sentinel_key", '"other-key"')):
            with self.subTest(field=field):
                self.override(f'[reset]\n{field}={value}\n')
                with self.assertRaisesRegex(ValueError, "reset." + field):
                    binding.reset_config(self.f.root, self.f.request, selected)


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

    def test_fixture_cache_tool_succession_reads_only_ledger_signed_request(self):
        from devex_clone_target_resources import fixture_redis_tool_wsl

        temporary = WorkspaceDirectory(
            dir=Path(__file__).resolve().parents[2] / ".local-tests/tmp", prefix="redis-wsl-")
        self.addCleanup(temporary.cleanup)
        backend = Path(temporary.name).resolve()
        run = backend / ".local-tests/run"
        request_path = run / "lifecycle-0011/redis/request.json"
        request_path.parent.mkdir(parents=True)
        wsl = {"path": str(backend / ".local-tests/wsl.exe"), "sha256": "2" * 64}
        write_json(request_path, {"wsl": wsl})
        generation_path = run / "lifecycle-0011/restart.json"
        write_json(generation_path, {"redis": {"request": receipt_binding(request_path)}})
        result_path = run / "results/0011.json"
        result_path.parent.mkdir(parents=True)
        write_json(result_path, {"generation": receipt_binding(generation_path)})
        cache = {"redis": {"wsl": wsl}, "generation": receipt_binding(result_path)}
        self.assertEqual(fixture_redis_tool_wsl(backend, cache), wsl)
        request_path.write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            fixture_redis_tool_wsl(backend, cache)

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
        result = f.prepared()
        f.publish_prepared_files()
        return result

    def initialize(self):
        f = self.f
        publisher = f.publish_initialized_files if (f.local / "prepared-files.json").is_file() else None
        result = target.initialize_target(f.root, f.output, f.run, publish_files=publisher)
        return result

    def test_business_fixture_execution_root_is_bound_to_its_generated_snapshot(self):
        f = self.f
        execution = f.local / "business-backend"
        execution.mkdir()
        (execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        (execution / ".git").write_text("gitdir: fixture", encoding="utf-8")
        generated = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": []}
        fixture = {
            "format_version": 1,
            "fixture": "business",
            "status": "ready",
            "paths": {"backend": str(execution), "frontend": str(f.local / "business-frontend")},
            "generated": {"backend": generated},
        }
        receipt = f.bound(f.local / "business-fixture.json", fixture)
        f.request["execution_backend"] = {"fixture": receipt, "path": str(execution)}

        with patch.object(binding, "snapshot", return_value=(generated, b"")):
            root, evidence = binding.execution_backend(f.root, f.request)

        self.assertEqual(root, execution)
        self.assertEqual(evidence["kind"], "business-fixture")
        self.assertEqual(evidence["source"], generated)

    def test_initialization_uses_only_bound_cli_then_readonly_verifies(self):
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
        self.assertEqual(set(f.request["tools"]), {"mysql"})
        self.assertNotIn("aws", f.review["tools"])
        receipts = [read_json(path) for path in f.output.glob("*.command.json")]
        native = [value for value in receipts if value.get("transport") == "rust-s3-sigv4"]
        self.assertTrue(native)
        configuration = f.request["target"]["s3"]
        for value in native:
            self.assertEqual(value["scope_id"], f.scope)
            self.assertEqual(value["endpoint"], configuration["endpoint"])
            self.assertEqual(value["configuration_sha256"], plan_hash(configuration))
            self.assertEqual(value["prefix"], f.scope + "/" if value["operation"] == "list-objects-v2" else None)
            self.assertIsNone(value["continuation"])

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
            {"mysql.exe", "reset.exe", "migrate.exe", "tenant-data.exe", "rustfs.exe", "wsl.exe"},
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


    def test_prepare_then_config_change_refuses_all_writes(self):
        self.prepare()
        path = self.f.configuration / "app.test.toml"
        original = path.read_bytes()
        path.write_text('[redis]\npassword="changed"\n', encoding="utf-8")
        with self.assertRaises(ValueError): self.initialize()
        self.assertFalse(self.f.databases)
        path.write_bytes(original)
        calls = copy.deepcopy(self.f.calls)

        def drift(_url):
            if path.read_bytes() == original:
                path.write_bytes(original + b"\n# observed drift\n")

        with patch.object(binding, "require_closed_port", side_effect=drift), self.assertRaisesRegex(ValueError, "绑定配置发生变化"):
            binding.generation(self.f.root, self.f.request, self.f.run)
        self.assertEqual(path.read_bytes(), original + b"\n# observed drift\n")
        self.assertEqual(self.f.calls, calls)
        self.assertFalse(self.f.databases)

    def test_seed_digest_binds_only_explicit_administrator_password(self):
        f = self.f
        request_before = copy.deepcopy(f.request)
        with patch.object(binding, "native_invoke", wraps=binding.native_invoke) as native:
            original = binding.generation(f.root, f.request, f.run)
        self.assertEqual(f.request, request_before)
        physical = [call.args[1]["arguments"]["source"] for call in native.call_args_list
                    if call.args[1].get("operation") == "physical-binding"]
        self.assertEqual(physical, [{**f.request["target"], "s3": {
            "configuration": binding.configuration_binding(f.root, f.configuration)}}])
        self.assertNotIn("secret_key", json.dumps(physical))
        wrong = copy.deepcopy(f.request)
        wrong["target"]["s3"]["secret_key"] = "different-private-secret"
        with patch.object(binding, "verify_tools") as tools, self.assertRaisesRegex(ValueError, "对象连接身份") as rejected:
            binding.generation(f.root, wrong, f.run)
        self.assertNotIn("different-private-secret", str(rejected.exception))
        tools.assert_not_called()
        self.assertEqual(original["seed_credentials_sha256"], plan_hash({
            "bootstrap.admin_password": "Fixture!Admin123",
        }))
        path = f.configuration / "app.test.toml"
        path.write_text('[bootstrap]\nadmin_password="Changed!Admin123"\n', encoding="utf-8")
        f.request["configuration_sha256"] = binding.configuration_digest(f.root, f.root / "config")
        changed = binding.generation(f.root, f.request, f.run)
        self.assertNotEqual(changed["seed_credentials_sha256"], original["seed_credentials_sha256"])
        self.assertNotEqual(changed["configuration_sha256"], original["configuration_sha256"])
        self.assertNotIn("Changed!Admin123", json.dumps(changed))
        for password in (None, "", "   "):
            with self.subTest(password=password):
                path.write_text("# 未覆盖管理员密码\n", encoding="utf-8")
                base = f.configuration / "app.toml"
                text = base.read_text(encoding="utf-8")
                replacement = "" if password is None else "admin_password=" + json.dumps(password) + "\n"
                base.write_text(text.replace('admin_password="Fixture!Admin123"\n', replacement), encoding="utf-8")
                f.request["configuration_sha256"] = binding.configuration_digest(f.root, f.root / "config")
                with self.assertRaisesRegex(ValueError, "bootstrap.admin_password"):
                    binding.generation(f.root, f.request, f.run)
                base.write_text(text, encoding="utf-8")
        self.assertFalse(f.databases)
        self.assertFalse(f.calls)

    def test_changed_administrator_password_after_prepare_refuses_all_writes(self):
        self.prepare()
        self.f.calls.clear()
        path = self.f.configuration / "app.test.toml"
        path.write_text('[bootstrap]\nadmin_password="Changed!Admin123"\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            self.initialize()
        self.assertFalse(self.f.databases)
        self.assertFalse((self.f.output / "initialize.started.json").exists())
        self.assertFalse(any(Path(command[0]).stem == "reset" for command in self.f.calls))

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
                return "process_id:201\r\nrun_id:" + "5" * 40 + "\r\nconfig_file:" + f.linux_path(Path(f.request["storage"]["redis"]["configuration"]["path"]))
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
        path = self.f.configuration / "app.test.toml"
        path.write_text('[reset]\nlegacy_mysql_exclusive=false\n', encoding="utf-8")
        self.f.request["configuration_sha256"] = binding.configuration_digest(self.f.root, self.f.root / "config")
        self.f.save_request()
        with self.assertRaisesRegex(ValueError, "reset.legacy_mysql_exclusive"): self.prepare()
        self.assertFalse(self.f.databases)


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
