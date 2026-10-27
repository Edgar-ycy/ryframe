"""新目标创建/初始化的离线回归；真实 MySQL、S3、Redis 仍由独立验收证明。"""
import copy
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_target_binding as binding
import devex_clone_target_runtime_evidence as runtime_evidence
from devex_clone_capture import write_json
from devex_clone_run_state import binding as receipt_binding
from devex_clone_target_resources import Resources
from devex_clone_target_state import generation_lock
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
            runtime_evidence.validate_started_generation(Path.cwd(), Path.cwd(), expected, started)


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


    def test_database_collision_with_reference_rejected(self):
        f = self.f
        collision = f.review["reference"]["source"]["databases"][0]["database"]
        f.review["scopes"]["base"]["databases"][0]["database"] = collision
        f.request["review"] = {**f.bound(f.local / "review.json", f.review), "canonical_sha256": plan_hash(f.review)}
        with self.assertRaises(ValueError): binding.request_binding(f.root, f.request)


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


    def test_resp_errors_and_partial_bulk_are_not_absence(self):
        for payload in (b"-NOAUTH denied\r\n", b"$4\r\nab\r\n", b"+OK", b"*10001\r\n"):
            with self.subTest(payload=payload):
                with self.assertRaises(ValueError): Resources._response(io.BytesIO(payload))


if __name__ == "__main__":
    unittest.main()
