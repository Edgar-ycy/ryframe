"""fresh 目标 inventory 与严格迁移前缀续作回归。"""
import copy
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_target as target
import devex_clone_target_binding as target_binding
import devex_clone_target_inventory_resume as inventory_resume
import devex_clone_target_prepared_files as prepared_evidence
import devex_clone_target_resume as target_resume
import devex_clone_target_resume_evidence as target_evidence
import devex_clone_target_resources as target_resources
from artifact_digests import filesystem_path
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding as receipt_binding
from devex_clone_target_resources import Resources
from devex_clone_target_fixture import Fixture
from devex_clone_inventory import InventoryCaptureError
from restore_build import file_digest
from restore_reference_plan import plan_hash
from workspace_directory import WorkspaceDirectory


def prepared_files(output):
    path = output.parent / "prepared-files.json"
    value = read_json(path)
    return {"descriptor": receipt_binding(path), "registration": value["registration"],
            "predecessor": value["predecessor"]}


def publish_initialized_files(output, files=None):
    workspace = output.parent
    path = workspace / "initialized-files.json"
    write_json(path, {
        "format_version": 1, "kind": "devex-clone-fresh-target-initialized-files",
        "registration": receipt_binding(workspace / "registration.json"),
        "predecessor": receipt_binding(workspace / "prepared-files.json"),
        "files": target_evidence.target_files(output) if files is None else files,
    })


def inventory_capture_failure(_backend, _request, _resources, output):
    """生成与真实 capture_side_inventory inputs 阶段一致的只读失败目录。"""
    output.mkdir()
    write_json(output / "failure.json", {
        "format_version": 1,
        "status": "side_inventory_failed",
        "stage": "inputs",
        "error_type": "FileNotFoundError",
        "remote_writes": 0,
        "target_ready": False,
        "clone_verified": False,
    })
    raise InventoryCaptureError(output)


def replace_json(path, value):
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def fail_readonly_receipt(path, value):
    if path.name.startswith("migrate-tenant-data---target-dedicated-a-verify-"):
        raise FileNotFoundError(2, "fixture long path", str(path))
    return write_json(path, value)


def migration_resume_state(backend, output, descriptor):
    return target_evidence.migration_resume_state(
        backend, output, descriptor, target.target_evidence_hooks(), prepared_files(output)
    )


def inventory_resume_state(backend, output, descriptor):
    return inventory_resume.inventory_resume_state(
        backend, output, descriptor, prepared_files(output)
    )


def resume_inventory_target(backend, output, run, *, request_descriptor):
    result = inventory_resume.resume_inventory_target(
        backend, output, run, request_descriptor=request_descriptor,
        prepared_files=prepared_files(output),
        publish_files=lambda files: publish_initialized_files(output, files))
    return result


def resume_initialize_target(backend, output, run, *, request_descriptor, storage_run=None):
    result = target_resume.resume_initialize_target(
        backend, output, run, request_descriptor=request_descriptor, storage_run=storage_run,
        prepared_files=prepared_files(output),
        publish_files=lambda files: publish_initialized_files(output, files))
    return result


class TargetResumeTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(
            Path(__file__).resolve().parents[2] / ".local-tests/python-unit",
            prefix="resume-",
        )
        self.addCleanup(temporary.cleanup)
        self.f = Fixture(Path(temporary.name), self)

    def prepare(self):
        result = target.prepare_target(self.f.root, self.f.path, self.f.output, self.f.run)
        self.f.publish_prepared_files()
        return result

    def initialize(self):
        result = target.initialize_target(self.f.root, self.f.output, self.f.run)
        self.f.publish_initialized_files()
        return result

    def validate_current_prepare_tree(self, descriptor):
        prepared_evidence.validate_prepared_tree(
            self.f.root, self.f.output, target_binding.target_files(self.f.output),
            descriptor, read_json(self.f.path),
        )

    def interrupt_migration_prefix(self, completed: int, *, prepared: bool = False) -> dict:
        """构造与真实 R25 相同的“命令启动前文件访问失败”现场。"""
        if not prepared:
            self.prepare()
        original = Resources.command
        observed = 0

        def command(resources, stage, arguments, **kwargs):
            nonlocal observed
            if stage.startswith("migrate-"):
                if observed == completed:
                    raise FileNotFoundError("fixture executable temporarily unavailable")
                observed += 1
            return original(resources, stage, arguments, **kwargs)

        with patch.object(Resources, "command", new=command), self.assertRaises(FileNotFoundError):
            self.initialize()
        return {"path": str(self.f.path), **file_digest(self.f.path)}

    def fixture_restart(self):
        f = self.f
        run, results = f.local / "service-run", f.local / "service-run/results"
        results.mkdir(parents=True)
        (run / "manifest.json").write_text(
            json.dumps({"kind": "reference-fixture-service-run"}), encoding="utf-8"
        )
        generation = f.bound(results / "restart.json", {"generation": "restart"})
        directory = Path(f.review["services"]["rustfs"]["data_dir"])
        storage_runtime = {
            "storage": copy.deepcopy(f.request["storage"]["rustfs"]),
            "data_directory": {"path": str(directory), "device": directory.stat().st_dev,
                               "inode": directory.stat().st_ino},
            "api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201",
            "generation": generation,
        }
        redis = copy.deepcopy(f.request["storage"]["redis"]); redis["run_id"] = "4" * 40
        f.storage_restarted = True; f.redis_values.clear()
        return run, storage_runtime, {"redis": redis, "generation": generation}

    def use_long_path_fixture(self):
        temporary = WorkspaceDirectory(
            Path(__file__).resolve().parents[2] / ".local-tests/python-unit",
            prefix="resume-long-",
        )
        self.addCleanup(temporary.cleanup)
        base = Path(temporary.name)
        self.addCleanup(shutil.rmtree, filesystem_path(base), True)
        suffix = len(str(Path(".local-tests") / "new-generation")) + 1
        label = "中文 空格-"
        padding = 167 - len(str(base)) - suffix - 1 - len(label)
        root = base / (label + "x" * padding)
        root.mkdir()
        self.f = Fixture(root, self)
        self.assertEqual(len(str(self.f.output)), 167)

    def test_inventory_failure_resumes_without_replaying_reset(self):
        f = self.f
        self.prepare()
        original_inventory = target.inventory
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError): self.initialize()
        completed = len([command for command in f.calls if Path(command[0]).stem == "reset" and command[1] == "execute"])
        self.assertEqual(completed, 1)
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        self.assertTrue(inventory_resume_state(f.root, f.output, descriptor)["resumable"])

        with patch.object(target, "inventory", side_effect=original_inventory):
            result = resume_inventory_target(f.root, f.output, f.run, request_descriptor=descriptor)

        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertEqual(completed, len([command for command in f.calls
                                         if Path(command[0]).stem == "reset" and command[1] == "execute"]))
        self.assertTrue((f.output / "initialized.json").is_file())
        self.assertFalse(target.unresolved_failure(f.root, f.output))
        extra = f.output / "unexpected-success-artifact.txt"
        extra.write_text("unknown", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "initialized"):
            target_binding.initialized_target_files(f.root, f.output)
        extra.unlink()
        self.assertEqual(target.verify_target(f.root, f.output, f.local / "verify-resumed", f.run)["remote_writes"], 0)
        unknown = f.output / ("resume-initialize-" + "f" * 32 + ".confirmed.json")
        write_json(unknown, {"kind": "unknown"})
        self.assertTrue(target.unresolved_failure(f.root, f.output))
        unknown.unlink()
        migration_started = f.output / ("resume-initialize-" + "e" * 32 + ".started.json")
        write_json(migration_started, {"kind": "unexpected-migration-evidence"})
        self.assertTrue(target.unresolved_failure(f.root, f.output))

    def test_inventory_resume_rejects_other_initialization_failures(self):
        self.prepare()
        self.f.plan_changed = True
        with self.assertRaises(ValueError): self.initialize()
        descriptor = {"path": str(self.f.path), **file_digest(self.f.path)}
        state = inventory_resume_state(self.f.root, self.f.output, descriptor)
        self.assertFalse(state["resumable"])
        with self.assertRaises(ValueError): resume_inventory_target(
            self.f.root, self.f.output, self.f.run, request_descriptor=descriptor
        )

    def test_inventory_resume_history_keeps_prior_readonly_failure(self):
        f = self.f
        self.prepare()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError): self.initialize()
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError): resume_inventory_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        failed = next(f.output.glob("resume-initialize-*.failure.json"))
        with patch.object(target, "inventory", wraps=target.inventory):
            result = resume_inventory_target(f.root, f.output, f.run, request_descriptor=descriptor)
        self.assertEqual(result["history"][failed.name], file_digest(failed))

    def test_migration_prefix_resumes_at_readonly_verify_without_replay(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original_failure = file_digest(f.output / "failure.json")
        before = [command for command in f.calls if Path(command[0]).stem in {"reset", "migrate"}]
        state = migration_resume_state(f.root, f.output, descriptor)
        self.assertTrue(state["resumable"])
        self.assertEqual([item["id"] for item in state["completed"]],
                         ["control-up", "control-verify", "tenant-data-dedicated-a-up"])
        self.assertEqual(state["operations"][state["next_index"]]["id"],
                         "tenant-data-dedicated-a-verify")
        self.assertFalse(state["operations"][state["next_index"]]["write"])

        result = resume_initialize_target(
            f.root, f.output, f.run, request_descriptor=descriptor
        )

        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertEqual(file_digest(f.output / "failure.json"), original_failure)
        after = [command for command in f.calls if Path(command[0]).stem in {"reset", "migrate"}]
        self.assertEqual(sum(Path(command[0]).stem == "reset" and command[1] == "execute"
                             for command in after), 1)
        migration_commands = [command[1:] for command in after if Path(command[0]).stem == "migrate"]
        self.assertEqual(len(migration_commands), 10)
        self.assertTrue(all(migration_commands.count(command[1:]) == 1
                            for command in before if Path(command[0]).stem == "migrate"))
        confirmation = read_json(next(f.output.glob("resume-initialize-*.confirmed.json")))
        self.assertEqual(confirmation["remote_write_operations"], 3)
        self.assertFalse(target.unresolved_failure(f.root, f.output))
        verified = target.verify_target(f.root, f.output, f.local / "verify-migration-resume", f.run)
        self.assertEqual(verified["remote_writes"], 0)
        confirmation_path = next(f.output.glob("resume-initialize-*.confirmed.json"))
        started_path = next(f.output.glob("resume-initialize-*.started.json"))

        original_confirmation = copy.deepcopy(confirmation)
        confirmation["at"] = ""
        replace_json(confirmation_path, confirmation)
        self.assertFalse(target_evidence.migration_resume_confirmed(
            f.root, f.output, result, target.target_evidence_hooks()
        ))
        confirmation = copy.deepcopy(original_confirmation)
        replace_json(confirmation_path, confirmation)

        started = read_json(started_path)
        original_started = copy.deepcopy(started)
        started["storage"] = copy.deepcopy(started["storage"])
        started["storage"]["redis"]["run_id"] = "f" * 40
        started["current_generation_sha256"] = plan_hash({
            **result["generation"], "storage": started["storage"],
        })
        replace_json(started_path, started)
        confirmation["started"] = receipt_binding(started_path)
        replace_json(confirmation_path, confirmation)
        self.assertTrue(target.unresolved_failure(f.root, f.output))

        replace_json(started_path, original_started)
        confirmation["started"] = receipt_binding(started_path)
        replace_json(confirmation_path, confirmation)
        unexpected = f.output / ("resume-initialize-" + "f" * 32 + ".confirmed.json")
        write_json(unexpected, {"kind": "unknown"})
        self.assertTrue(target.unresolved_failure(f.root, f.output))
        unexpected.unlink()

        mismatched = f.output / ("resume-initialize-" + "e" * 32 + ".confirmed.json")
        confirmation_path.rename(mismatched)
        self.assertTrue(target.unresolved_failure(f.root, f.output))
        mismatched.rename(confirmation_path)

        started = copy.deepcopy(original_started)
        started["resources_before"]["databases"][0]["server_uuid"] = "wrong"
        replace_json(started_path, started)
        confirmation["started"] = receipt_binding(started_path)
        replace_json(confirmation_path, confirmation)
        self.assertTrue(target.unresolved_failure(f.root, f.output))

        replace_json(started_path, original_started)
        confirmation["started"] = receipt_binding(started_path)
        create_path = f.output / "create-dedicated-a.confirmed.json"
        create_confirmation = read_json(create_path)
        original_create_confirmation = copy.deepcopy(create_confirmation)
        create_confirmation["observed"]["exists"] = False
        replace_json(create_path, create_confirmation)
        intent_path = next(f.output.glob("resume-initialize-*.intent.json"))
        resume_intent = read_json(intent_path)
        original_resume_intent = copy.deepcopy(resume_intent)
        resume_intent["creation"]["create-dedicated-a"]["confirmed"] = \
            receipt_binding(create_path)
        replace_json(intent_path, resume_intent)
        confirmation["intent"] = receipt_binding(intent_path)
        replace_json(confirmation_path, confirmation)
        self.assertTrue(target.unresolved_failure(f.root, f.output))

        replace_json(create_path, original_create_confirmation)
        replace_json(intent_path, original_resume_intent)
        confirmation["intent"] = receipt_binding(intent_path)
        confirmation["remote_write_operations"] = 2
        replace_json(confirmation_path, confirmation)
        self.assertTrue(target.unresolved_failure(f.root, f.output))

    def test_fixture_restart_rebinds_only_redis_markers_before_resume(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        migrations_before = [call for call in f.calls if Path(call[0]).stem == "migrate"]
        run, storage_runtime, cache_runtime = self.fixture_restart()
        transition = (storage_runtime, cache_runtime)
        with patch("reference_fixture_service_context.runtime_transition",
                   return_value=transition), \
                patch.object(target_evidence, "validate_started_generation"):
            result = resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run
            )
            self.assertFalse(target.unresolved_failure(f.root, f.output))
        redis = f.review["scopes"]["seed"]["redis"]
        self.assertEqual(f.redis_values, {
            redis["ownership_key"]: redis["ownership_value"],
            f.request["reset"]["sentinel_key"]: f.request["reset"]["sentinel_value"],
        })
        marker = read_json(next(f.output.glob("resume-redis-markers-*.confirmed.json")))
        confirmation = read_json(next(f.output.glob("resume-initialize-*.confirmed.json")))
        self.assertEqual(marker["atomic_result"], 2)
        self.assertEqual(confirmation["redis_marker_rebind"], receipt_binding(next(
            f.output.glob("resume-redis-markers-*.confirmed.json"))))
        self.assertEqual(confirmation["remote_write_operations"], 4)
        migrations_after = [call for call in f.calls if Path(call[0]).stem == "migrate"]
        self.assertEqual(len(migrations_after), 10)
        self.assertTrue(all(migrations_after.count(call) == 1 for call in migrations_before))
        self.assertEqual(result["redis"], marker["after"])

    def test_fixture_restart_rejects_existing_sentinel_before_marker_intent(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        migrations = len([call for call in f.calls if Path(call[0]).stem == "migrate"])
        run, storage_runtime, cache_runtime = self.fixture_restart()
        f.redis_values[f.request["reset"]["sentinel_key"]] = "unexpected"
        with patch("reference_fixture_service_context.runtime_transition",
                   return_value=(storage_runtime, cache_runtime)), \
                self.assertRaisesRegex(ValueError, "Redis"):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run
            )
        self.assertFalse(any(f.output.glob("resume-redis-markers-*.intent.json")))
        self.assertEqual(migrations, len(
            [call for call in f.calls if Path(call[0]).stem == "migrate"]
        ))

    def test_fixture_restart_marker_command_failure_is_unknown_and_not_retried(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        migrations = len([call for call in f.calls if Path(call[0]).stem == "migrate"])
        run, storage_runtime, cache_runtime = self.fixture_restart()
        eval_calls = []

        def redis(args):
            if args[0] == "EVAL":
                eval_calls.append(args)
                f.redis(args)
                raise TimeoutError("fixture reply lost after atomic write")
            return f.redis(args)

        with patch("reference_fixture_service_context.runtime_transition",
                   return_value=(storage_runtime, cache_runtime)), \
                patch.object(Resources, "_redis", side_effect=redis), \
                self.assertRaises(TimeoutError):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run
            )
        marker_failure = read_json(next(f.output.glob("resume-redis-markers-*.failure.json")))
        failed = read_json(next(f.output.glob("resume-initialize-*.failure.json")))
        self.assertTrue(marker_failure["unknown_result"])
        self.assertEqual(failed["unknown_write_operation"], "redis-marker-rebind")
        self.assertEqual(failed["active_operation"]["failure"], receipt_binding(next(
            f.output.glob("resume-redis-markers-*.failure.json"))))
        self.assertEqual(len(eval_calls), 1)
        self.assertEqual(migrations, len(
            [call for call in f.calls if Path(call[0]).stem == "migrate"]
        ))
        calls = len(f.calls)
        with self.assertRaises(ValueError):
            resume_initialize_target(f.root, f.output, f.run, request_descriptor=descriptor)
        self.assertEqual(len(f.calls), calls)

    def test_inventory_resume_does_not_repeat_after_success_confirmation(self):
        f = self.f
        self.prepare()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError):
                self.initialize()
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        resume_inventory_target(f.root, f.output, f.run, request_descriptor=descriptor)
        (f.output / "initialized.json").unlink()
        calls = len(f.calls)

        state = inventory_resume_state(f.root, f.output, descriptor)

        self.assertFalse(state["resumable"])
        with self.assertRaises(ValueError):
            resume_inventory_target(f.root, f.output, f.run, request_descriptor=descriptor)
        self.assertEqual(len(f.calls), calls)

    def test_inventory_resume_rejects_migration_started_or_operation_evidence(self):
        f = self.f
        self.prepare()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError):
                self.initialize()
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        calls = len(f.calls)
        for name in ("resume-initialize-" + "a" * 32 + ".started.json",
                     "resume-migrate-" + "a" * 32 + "-03.intent.json"):
            with self.subTest(name=name):
                path = f.output / name
                write_json(path, {"unknown": True})
                self.assertFalse(inventory_resume_state(
                    f.root, f.output, descriptor)["resumable"])
                with self.assertRaises(ValueError):
                    resume_inventory_target(
                        f.root, f.output, f.run, request_descriptor=descriptor
                    )
                self.assertEqual(len(f.calls), calls)
                path.unlink()

    def test_inventory_resume_rejects_orphan_or_non_readonly_attempts_and_candidate(self):
        f = self.f
        self.prepare()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError):
                self.initialize()
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        attempt = "a" * 32
        path = f.output / f"resume-initialize-{attempt}.intent.json"
        base = {"format_version": 1, "kind": "devex-clone-initialize-resume-intent",
                "attempt": attempt, "at": read_json(f.output / "failure.json")["at"], "failure": receipt_binding(
                    f.output / "failure.json"), "prepared_files": prepared_files(f.output),
                "remote_writes": 0}
        calls = len(f.calls)

        write_json(path, base)
        with self.assertRaisesRegex(ValueError, "挂起"):
            inventory_resume_state(f.root, f.output, descriptor)
        path.unlink()
        write_json(path, {**base, "remote_writes": 1})
        with self.assertRaisesRegex(ValueError, "只读失败"):
            inventory_resume_state(f.root, f.output, descriptor)
        path.unlink()
        write_json(f.output / "initialized-candidate.json", {"status": "unknown"})
        self.assertFalse(inventory_resume_state(
            f.root, f.output, descriptor)["resumable"])
        self.assertEqual(len(f.calls), calls)

    def test_inventory_resume_rejects_linked_evidence_before_intent(self):
        f = self.f
        self.prepare()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError):
                self.initialize()
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        linked_path = f.output / "reset-state"
        original = target_evidence.linked
        binding_linked = target_binding.linked
        calls = len(f.calls)

        with patch.object(target_evidence, "linked",
                          side_effect=lambda path: path == linked_path or original(path)), \
                patch.object(target_binding, "linked",
                             side_effect=lambda path: path == linked_path or binding_linked(path)), \
                self.assertRaisesRegex(ValueError, "经过链接"):
            resume_inventory_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )

        self.assertFalse(any(f.output.glob("resume-initialize-*.intent.json")))
        self.assertEqual(len(f.calls), calls)

    def test_migration_resume_rejects_prefix_whose_first_missing_operation_writes(self):
        descriptor = self.interrupt_migration_prefix(2)
        calls = len(self.f.calls)
        with self.assertRaisesRegex(ValueError, "首个缺失操作必须是只读 verify"):
            migration_resume_state(self.f.root, self.f.output, descriptor)
        with self.assertRaises(ValueError):
            resume_initialize_target(
                self.f.root, self.f.output, self.f.run, request_descriptor=descriptor
            )
        self.assertEqual(len(self.f.calls), calls)

    def test_migration_resume_accepts_valid_completed_prepare_resume_evidence(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interruption")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        target.resume_prepare_target(
            f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor)
        f.publish_prepared_files()
        self.interrupt_migration_prefix(3, prepared=True)

        state = migration_resume_state(f.root, f.output, descriptor)
        self.assertTrue(state["resumable"], state)
        result = resume_initialize_target(
            f.root, f.output, f.run, request_descriptor=descriptor
        )

        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertFalse(target.unresolved_failure(f.root, f.output))

    def test_prepared_tree_requires_one_mysql_output_line_with_platform_ending(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        target.prepare_target(f.root, f.path, f.output, f.run)
        receipt = next(f.output.glob("mysql-shared-control-*.command.json"))
        value = read_json(receipt)
        self.assertEqual(value["stdout"], f.uuid + "\r\n")
        self.validate_current_prepare_tree(descriptor)
        value["stdout"] = f.uuid + "\n"
        replace_json(receipt, value)
        self.validate_current_prepare_tree(descriptor)

        for stdout in (f.uuid, f.uuid + "\r\nextra\r\n", " " + f.uuid + "\r\n",
                       f.uuid + " \r\n", f.uuid + "\r\n\r\n"):
            with self.subTest(stdout=repr(stdout)):
                value["stdout"] = stdout
                replace_json(receipt, value)
                with self.assertRaisesRegex(ValueError, "MySQL"):
                    self.validate_current_prepare_tree(descriptor)

    def test_prepared_tree_compares_stable_redis_proc_identity(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        target.prepare_target(f.root, f.path, f.output, f.run)
        receipts = []
        for path in f.output.glob("redis-kernel-*.command.json"):
            value = read_json(path)
            if value["command"][-2:] == ["/usr/bin/cat", "/proc/200/stat"]:
                receipts.append(path)
        self.assertEqual(len(receipts), 2)
        changed = read_json(receipts[1])
        fields = changed["stdout"].removesuffix("\n").rpartition(") ")[2].split(" ")
        fields[0], fields[11], fields[12] = "R", "41", "73"
        changed["stdout"] = "200 (redis-server) " + " ".join(fields) + "\r\n"
        replace_json(receipts[1], changed)
        self.validate_current_prepare_tree(descriptor)

        stable = changed["stdout"]
        for stdout in (stable.replace("200 (", "201 (", 1),
                       stable.replace("(redis-server)", "(other)"),
                       stable.rsplit("67890", 1)[0] + "67891\r\n"):
            with self.subTest(stdout=stdout):
                changed["stdout"] = stdout
                replace_json(receipts[1], changed)
                with self.assertRaisesRegex(ValueError, "Redis"):
                    self.validate_current_prepare_tree(descriptor)

    def test_migration_resume_allows_only_volatile_proc_stat_changes(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        prepared = {item["path"] for item in read_json(
            f.output.parent / "prepared-files.json")["files"]}
        receipts = []
        for path in f.output.glob("redis-kernel-*.command.json"):
            value = read_json(path)
            if (path.name not in prepared
                    and value["command"][-2:] == ["/usr/bin/cat", "/proc/200/stat"]):
                receipts.append(path)
        self.assertEqual(len(receipts), 15)
        changed = read_json(receipts[-1])
        fields = changed["stdout"].removesuffix("\n").rpartition(") ")[2].split(" ")
        fields[0], fields[11], fields[12] = "R", "41", "73"
        volatile = "200 (redis-server) " + " ".join(fields) + "\n"
        changed["stdout"] = volatile
        replace_json(receipts[-1], changed)
        self.assertTrue(migration_resume_state(f.root, f.output, descriptor)["resumable"])

        for stdout in (volatile.replace("200 (", "201 (", 1),
                       volatile.replace("(redis-server)", "(other)"),
                       volatile.rsplit("67890", 1)[0] + "67891\n"):
            with self.subTest(stdout=stdout):
                changed["stdout"] = stdout
                replace_json(receipts[-1], changed)
                with self.assertRaisesRegex(ValueError, "Redis"):
                    migration_resume_state(f.root, f.output, descriptor)

    def test_prepared_tree_rejects_unclaimed_prepare_history_files(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interruption")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        target.resume_prepare_target(
            f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor,
        )
        self.validate_current_prepare_tree(descriptor)
        attempt = next(f.output.glob("resume-prepare-*.intent.json")).name.split(".")[0]

        for name in ("resume-prepare-unclaimed.bin", attempt + ".intent.json.bak",
                     "resume-prepare-extra.json"):
            with self.subTest(name=name):
                extra = f.output / name
                extra.write_text("{}", encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "未声明|无法归属"):
                    self.validate_current_prepare_tree(descriptor)
                extra.unlink()

    def test_inventory_resume_accepts_failure_after_completed_prepare_resume(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with patch.object(target, "context", side_effect=OSError("fixture interruption")), \
                self.assertRaises(OSError):
            target.prepare_target(f.root, f.path, f.output, f.run)
        target.resume_prepare_target(
            f.root, f.output, f.run, storage_run=None, request_descriptor=descriptor
        )
        f.publish_prepared_files()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure), \
                self.assertRaises(InventoryCaptureError):
            target.initialize_target(f.root, f.output, f.run)

        state = inventory_resume_state(f.root, f.output, descriptor)
        self.assertTrue(state["resumable"], state)
        self.assertRegex(Path(state["failure"]["path"]).name, r"failure-[a-f0-9]{32}[.]json")
        result = resume_inventory_target(
            f.root, f.output, f.run, request_descriptor=descriptor
        )

        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertFalse(target.unresolved_failure(f.root, f.output))

    def test_migration_resume_lock_release_failure_does_not_publish_outer_snapshot(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)

        with patch.object(Path, "rmdir", side_effect=OSError("busy")), \
                self.assertRaises(OSError):
            resume_initialize_target(f.root, f.output, f.run, request_descriptor=descriptor)

        self.assertTrue((f.output / "initialized.json").is_file())
        self.assertTrue((f.output / "initialize.lock").is_dir())
        self.assertFalse((f.local / "initialized-files.json").exists())
        self.assertEqual(len(list(f.output.glob("resume-initialize-*.failure.json"))), 1)
        self.assertTrue(target.unresolved_failure(f.root, f.output))

    def test_migration_resume_rejects_prepare_resume_created_after_initialization_write(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        attempt = "a" * 32
        intent = f.output / f"resume-prepare-{attempt}.intent.json"
        write_json(intent, {
            "format_version": 1, "kind": "devex-clone-prepare-resume-intent",
            "attempt": attempt, "at": read_json(f.output / "initialize.started.json")["at"],
            "request": descriptor,
            "files_before": {"reset.intent.json": receipt_binding(
                f.output / "reset.intent.json")},
        })
        write_json(f.output / f"resume-prepare-{attempt}.confirmed.json", {
            "format_version": 1, "kind": "devex-clone-prepare-resume-confirmed",
            "attempt": attempt, "at": read_json(f.output / "initialize.started.json")["at"],
            "intent": receipt_binding(intent),
            "prepare": receipt_binding(f.output / "prepare.json"),
            "request": receipt_binding(f.output / "request.json"), "remote_writes": 0,
        })

        with self.assertRaisesRegex(ValueError, "初始化前因果链|晚于初始化开始"):
            migration_resume_state(f.root, f.output, descriptor)

    def test_migration_resume_rejects_linked_required_evidence_before_intent(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original = target_evidence.linked
        binding_linked = target_binding.linked
        calls = len(f.calls)
        for linked_path in (f.output / "reset-plan.json", f.output / "reset-state"):
            with self.subTest(path=linked_path), patch.object(
                    target_evidence, "linked",
                    side_effect=lambda path, expected=linked_path: path == expected or original(path)), \
                    patch.object(target_binding, "linked", side_effect=lambda path, expected=linked_path:
                                 path == expected or binding_linked(path)), \
                    self.assertRaisesRegex(ValueError, "经过链接"):
                resume_initialize_target(
                    f.root, f.output, f.run, request_descriptor=descriptor
                )
            self.assertFalse(any(f.output.glob("resume-initialize-*.intent.json")))
            self.assertEqual(len(f.calls), calls)

    def test_migration_resume_rechecks_links_after_initialize_lock(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        linked_path = f.output / "reset-state"
        original = target_evidence.linked
        binding_linked = target_binding.linked
        calls = len(f.calls)

        def linked_after_lock(path):
            return ((f.output / "initialize.lock").exists() and path == linked_path) or original(path)

        with patch.object(target_evidence, "linked", side_effect=linked_after_lock), \
                patch.object(target_binding, "linked", side_effect=linked_after_lock), \
                self.assertRaisesRegex(ValueError, "经过链接"):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )

        self.assertFalse(any(f.output.glob("resume-initialize-*.intent.json")))
        self.assertEqual(len(f.calls), calls)

    def test_migration_resume_rejects_duplicate_or_noncontiguous_receipts(self):
        descriptor = self.interrupt_migration_prefix(3)
        original = next(self.f.output.glob("migrate-control-up-*.command.json"))
        duplicate = self.f.output / ("migrate-control-up-" + "f" * 32 + ".command.json")
        duplicate.write_bytes(original.read_bytes())
        with self.assertRaisesRegex(ValueError, "重复命令收据"):
            migration_resume_state(self.f.root, self.f.output, descriptor)
        duplicate.unlink()
        future = target_evidence.migration_operations(self.f.maintenance)[4]
        write_json(self.f.output / (future["stage"] + "-" + "e" * 32 + ".command.json"), {
            "command": future["command"], "returncode": 0, "error_type": None,
            "stdout": future["stdout"], "stderr": "",
        })
        with self.assertRaisesRegex(ValueError, "越过首个缺失操作"):
            migration_resume_state(self.f.root, self.f.output, descriptor)

    def test_migration_resume_rejects_tampered_observation_stdout(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        baseline = {item["path"] for item in read_json(
            f.output.parent / "prepared-files.json")["files"]}
        receipt = next(path for path in f.output.glob("mysql-shared-*.command.json")
                       if path.name not in baseline)
        value = read_json(receipt)
        value["stdout"] = value["stdout"] + "unexpected"
        replace_json(receipt, value)

        with self.assertRaisesRegex(ValueError, "MySQL.*输出多重集"):
            migration_resume_state(f.root, f.output, descriptor)

    def test_migration_resume_rejects_unbound_prepared_predecessor(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        snapshot_path = f.output.parent / "prepared-files.json"
        snapshot = read_json(snapshot_path)
        snapshot["predecessor"] = {"path": "untrusted"}
        replace_json(snapshot_path, snapshot)
        evidence = {"descriptor": receipt_binding(snapshot_path),
                    "registration": snapshot["registration"],
                    "predecessor": snapshot["predecessor"]}

        with self.assertRaisesRegex(ValueError, "predecessor"):
            target_evidence.migration_resume_state(
                f.root, f.output, descriptor, target.target_evidence_hooks(), evidence
            )

    def test_migration_resume_rejects_unknown_create_or_reset_evidence(self):
        descriptor = self.interrupt_migration_prefix(3)
        calls = len(self.f.calls)
        extra_create = self.f.output / "create-unknown.intent.json"
        write_json(extra_create, {"stage": "unknown"})
        with self.assertRaisesRegex(ValueError, "未知证据"):
            migration_resume_state(self.f.root, self.f.output, descriptor)
        extra_create.unlink()
        unknown_intent = self.f.output / "unknown.intent.json"
        write_json(unknown_intent, {"stage": "unknown"})
        with self.assertRaisesRegex(ValueError, "未登记"):
            migration_resume_state(self.f.root, self.f.output, descriptor)
        unknown_intent.unlink()
        extra_reset = self.f.output / "reset-plan-unknown.json"
        write_json(extra_reset, {"plan": "unknown"})
        with self.assertRaisesRegex(ValueError, "未知证据"):
            migration_resume_state(self.f.root, self.f.output, descriptor)
        extra_reset.unlink()
        unknown_reset_command = self.f.output / ("reset-plan-" + "f" * 32 + ".command.json")
        unknown_reset_command.write_bytes(next(self.f.output.glob(
            "reset-plan-*.command.json")).read_bytes())
        with self.assertRaisesRegex(ValueError, "唯一命令收据"):
            migration_resume_state(self.f.root, self.f.output, descriptor)
        self.assertEqual(len(self.f.calls), calls)

    def test_migration_resume_requires_complete_reset_and_exact_failure(self):
        descriptor = self.interrupt_migration_prefix(3)
        report = next((self.f.output / "reset-state").glob("*.report.json"))
        value = read_json(report)
        value["phases"]["release"]["status"] = "running"
        report.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "reset 有未完成阶段"):
            migration_resume_state(self.f.root, self.f.output, descriptor)

    def test_migration_resume_binds_reset_plan_stdout_document_hash(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        receipt = next(path for path in f.output.glob("reset-plan-*.command.json")
                       if not path.name.startswith("reset-plan-confirm-"))
        value = read_json(receipt)
        original = '\"environment\": \"test\"'
        self.assertIn(original, value["stdout"])
        value["stdout"] = value["stdout"].replace(
            original, '\"environment\" : \"test\"', 1
        )
        replace_json(receipt, value)

        with self.assertRaisesRegex(ValueError, "固定清单"):
            migration_resume_state(f.root, f.output, descriptor)

    def test_migration_resume_requires_exact_previous_reset_ledger(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        previous = next((f.output / "reset-state").glob(".*.ledger.json.previous"))
        original = previous.read_bytes()
        previous.unlink()
        with self.assertRaises((FileNotFoundError, ValueError)):
            migration_resume_state(f.root, f.output, descriptor)
        previous.write_bytes(original)
        value = read_json(previous)
        value["phases"]["release"] = {
            "status": "complete", "completed_at": "2026-09-04T00:00:00Z"
        }
        replace_json(previous, value)
        with self.assertRaisesRegex(ValueError, "直接前像"):
            migration_resume_state(f.root, f.output, descriptor)

    def test_inventory_resume_rejects_unknown_failure_artifact(self):
        f = self.f
        self.prepare()
        with patch.object(target, "inventory", side_effect=inventory_capture_failure):
            with self.assertRaises(InventoryCaptureError):
                self.initialize()
        (f.output / "inventory-initial" / "unknown.json").write_text(
            "{}", encoding="utf-8"
        )
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        with self.assertRaisesRegex(ValueError, "未知证据"):
            inventory_resume_state(f.root, f.output, descriptor)

    def test_migration_resume_rejects_locked_tree_injection_before_context(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original_write = target_resume.write_json
        calls = len(f.calls)

        def write(path, value):
            original_write(path, value)
            if value.get("kind") == "devex-clone-migration-resume-intent":
                (f.output / "injected-before-context.txt").write_text(
                    "unknown", encoding="utf-8"
                )

        with patch.object(target_resume, "write_json", new=write), \
                self.assertRaisesRegex(ValueError, "唯一声明文件"):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        self.assertEqual(len(f.calls), calls)

    def test_migration_resume_rejects_locked_tree_injection_before_operation(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original_write = target_resume.write_json
        migrations = len([call for call in f.calls if Path(call[0]).stem == "migrate"])

        def write(path, value):
            original_write(path, value)
            if value.get("kind") == "devex-clone-migration-resume-started":
                (f.output / "injected-before-operation.txt").write_text(
                    "unknown", encoding="utf-8"
                )

        with patch.object(target_resume, "write_json", new=write), \
                self.assertRaisesRegex(ValueError, "started 前像"):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        self.assertEqual(migrations, len(
            [call for call in f.calls if Path(call[0]).stem == "migrate"]
        ))

    def test_migration_resume_rejects_other_failure_types_without_commands(self):
        descriptor = self.interrupt_migration_prefix(3)
        failed = read_json(self.f.output / "failure.json")
        failed["error_type"] = "CalledProcessError"
        self.f.output.joinpath("failure.json").write_text(json.dumps(failed), encoding="utf-8")
        state = migration_resume_state(self.f.root, self.f.output, descriptor)
        self.assertFalse(state["resumable"])
        calls = len(self.f.calls)
        with self.assertRaises(ValueError):
            resume_initialize_target(
                self.f.root, self.f.output, self.f.run, request_descriptor=descriptor
            )
        self.assertEqual(len(self.f.calls), calls)

    def test_failed_resumed_write_is_unknown_and_cannot_be_retried(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original = Resources.command

        def command(resources, stage, arguments, **kwargs):
            if stage == "migrate-tenant-data---target-dedicated-b-up":
                raise subprocess.TimeoutExpired(arguments, 30)
            return original(resources, stage, arguments, **kwargs)

        with patch.object(Resources, "command", new=command), self.assertRaises(subprocess.TimeoutExpired):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        failed = read_json(next(f.output.glob("resume-initialize-*.failure.json")))
        self.assertEqual(failed["unknown_write_operation"], "tenant-data-dedicated-b-up")
        self.assertEqual(failed["remote_write_operations_confirmed"], 0)
        self.assertEqual(failed["active_operation"]["id"], "tenant-data-dedicated-b-up")
        self.assertEqual(failed["active_operation"]["command_receipts"], [])
        self.assertEqual(failed["started"], receipt_binding(next(
            f.output.glob("resume-initialize-*.started.json"))))
        calls = len(f.calls)
        with self.assertRaises(ValueError):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        self.assertEqual(len(f.calls), calls)

    def test_failed_resumed_verify_binds_started_and_operation_intent(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original = Resources.command

        def command(resources, stage, arguments, **kwargs):
            if stage == "migrate-tenant-data---target-dedicated-a-verify":
                raise FileNotFoundError(arguments[0])
            return original(resources, stage, arguments, **kwargs)

        with patch.object(Resources, "command", new=command), self.assertRaises(FileNotFoundError):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )

        failed = read_json(next(f.output.glob("resume-initialize-*.failure.json")))
        active = failed["active_operation"]
        self.assertIsNone(failed["unknown_write_operation"])
        self.assertEqual(active["id"], "tenant-data-dedicated-a-verify")
        self.assertEqual(active["index"], 3)
        self.assertEqual(active["command_receipts"], [])
        self.assertEqual(active["intent"], receipt_binding(
            f.output / Path(active["intent"]["path"]).name))
        self.assertEqual(failed["started"], receipt_binding(next(
            f.output.glob("resume-initialize-*.started.json"))))
        calls = len(f.calls)
        with self.assertRaises(ValueError):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        self.assertEqual(len(f.calls), calls)

    def test_receipt_publish_failure_allows_one_readonly_successor(self):
        self.use_long_path_fixture()
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        receipt = f.output / ("migrate-tenant-data---target-dedicated-a-verify-"
                              + "0" * 32 + ".command.json")
        self.assertGreater(len(str(receipt)), 260)
        self.assertIn("中文 空格", str(receipt))
        run, storage_runtime, cache_runtime = self.fixture_restart()
        with patch("reference_fixture_service_context.runtime_transition",
                   return_value=(storage_runtime, cache_runtime)), \
                patch.object(target_evidence, "validate_started_generation"), \
                patch.object(Resources, "_redis", side_effect=f.redis) as redis:
            with patch.object(target_resources, "write_json", new=fail_readonly_receipt), \
                    self.assertRaises(FileNotFoundError):
                resume_initialize_target(
                    f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run
                )
            failed = read_json(next(f.output.glob("resume-initialize-*.failure.json")))
            self.assertEqual(failed["phase"], "command-receipt-publish")
            self.assertEqual(failed["os_error"]["filename_role"], "command_receipt")
            failure_path = next(f.output.glob("resume-initialize-*.failure.json"))
            self.assertEqual(migration_resume_state(f.root, f.output, descriptor)["mode"],
                             "migration-successor")
            corruptions = [
                ("unknown write", {"unknown_write_operation": "redis-marker-rebind"}),
                ("already confirmed", {"confirmed_operations": [{"id": "unexpected"}]}),
                ("wrong errno", {"os_error": {**failed["os_error"], "errno": 13}}),
                ("wrong filename", {"os_error": {
                    **failed["os_error"], "filename_role": "other"}}),
                ("wrong length", {"os_error": {
                    **failed["os_error"], "filename_length": len(str(receipt)) + 4}}),
                ("earlier phase", {"phase": "command-running"}),
                ("wrong operation", {"active_operation": {
                    **failed["active_operation"], "index": 4}}),
                ("existing receipt", {"active_operation": {
                    **failed["active_operation"], "command_receipts": [descriptor]}}),
            ]
            before_calls = len(f.calls)
            for label, changed in corruptions:
                with self.subTest(predecessor=label):
                    replace_json(failure_path, {**failed, **changed})
                    with self.assertRaises(ValueError):
                        migration_resume_state(f.root, f.output, descriptor)
                    replace_json(failure_path, failed)
            self.assertEqual(len(f.calls), before_calls)
            failed.pop("phase")
            failed.pop("os_error")
            for field in ("phase", "receipt_path_length", "receipt_path_limit_risk"):
                failed["active_operation"].pop(field)
            replace_json(failure_path, failed)
            state = migration_resume_state(f.root, f.output, descriptor)
            self.assertEqual(state["mode"], "migration-successor")
            result = resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run
            )
            self.assertEqual(sum(call.args[0][0] == "EVAL" for call in redis.call_args_list), 1)

        calls = [call[1:] for call in f.calls if Path(call[0]).stem == "migrate"]
        self.assertEqual(calls.count(["tenant-data", "verify", "--target", "dedicated-a"]), 2)
        for operation in target_evidence.migration_operations(f.maintenance)[0:3]:
            self.assertEqual(calls.count(operation["command"][1:]), 1)
        successor = next(read_json(path) for path in f.output.glob(
            "resume-initialize-*.intent.json") if "predecessor" in read_json(path))
        self.assertEqual(successor["remote_write_operations_before_resume"], 1)
        self.assertEqual(successor["predecessor"], receipt_binding(next(
            f.output.glob("resume-initialize-*.failure.json"))))
        self.assertEqual(result["status"], "fresh_target_initialized")
        with patch.object(target_evidence, "validate_started_generation"), \
                patch("reference_fixture_service_context.runtime_transition",
                      return_value=(storage_runtime, cache_runtime)):
            self.assertTrue(target_evidence._migration_resume_confirmed(
                f.root, f.output, result, target.target_evidence_hooks()
            ))
            self.assertFalse(target.unresolved_failure(f.root, f.output))
            confirmation_path = next(f.output.glob("resume-initialize-*.confirmed.json"))
            confirmation = read_json(confirmation_path)
            started_path = Path(confirmation["started"]["path"])
            started = read_json(started_path)
            runtime = copy.deepcopy(started)
            runtime["storage_runtime"]["generation"]["sha256"] = "0" * 64
            runtime["cache_runtime"]["generation"]["sha256"] = "0" * 64
            preimage = copy.deepcopy(started)
            preimage["resources_before"]["redis"]["owner"] = "changed-owner"
            for label, changed in (("generation", runtime), ("preimage", preimage)):
                with self.subTest(consumer=label):
                    replace_json(started_path, changed)
                    replace_json(confirmation_path, {**confirmation,
                                 "started": receipt_binding(started_path)})
                    self.assertFalse(target_evidence.migration_resume_confirmed(
                        f.root, f.output, result, target.target_evidence_hooks()))
                    replace_json(started_path, started)
                    replace_json(confirmation_path, confirmation)
            replace_json(confirmation_path, {**confirmation, "predecessor": descriptor})
            self.assertFalse(target_evidence.migration_resume_confirmed(
                f.root, f.output, result, target.target_evidence_hooks()))
            replace_json(confirmation_path, confirmation)
        with self.assertRaises(ValueError):
            resume_initialize_target(f.root, f.output, f.run, request_descriptor=descriptor)

    def test_second_receipt_failure_rejects_third_attempt_without_rebinding(self):
        self.use_long_path_fixture()
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        run, storage_runtime, cache_runtime = self.fixture_restart()
        with patch("reference_fixture_service_context.runtime_transition",
                   return_value=(storage_runtime, cache_runtime)), \
                patch.object(target_evidence, "validate_started_generation"), \
                patch.object(target_resources, "write_json", new=fail_readonly_receipt), \
                patch.object(Resources, "_redis", side_effect=f.redis) as redis:
            for _ in range(2):
                with self.assertRaises(FileNotFoundError):
                    resume_initialize_target(
                        f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run)
            self.assertEqual(len(list(f.output.glob("resume-initialize-*.failure.json"))), 2)
            calls = len(f.calls)
            with self.assertRaises(ValueError):
                resume_initialize_target(
                    f.root, f.output, f.run, request_descriptor=descriptor, storage_run=run)
            self.assertEqual(len(f.calls), calls)
            self.assertEqual(sum(call.args[0][0] == "EVAL" for call in redis.call_args_list), 1)

    def test_receipt_diagnostic_normalizes_windows_extended_path(self):
        filename = r"D:\中文 空格\receipt.json"
        error = FileNotFoundError(2, "fixture", "\\\\?\\" + filename)
        diagnostic = target_resume._safe_os_error(error, {"_receipt_path": filename})
        self.assertEqual(diagnostic["filename_role"], "command_receipt")
        self.assertEqual(diagnostic["filename_length"], len(filename))

    def test_command_success_before_confirmation_failure_is_bound_and_not_retried(self):
        f = self.f
        descriptor = self.interrupt_migration_prefix(3)
        original_write = target_resume.write_json

        def write(path, value):
            if re.fullmatch(r"resume-migrate-[a-f0-9]{32}-04\.confirmed\.json", path.name):
                raise OSError("fixture confirmation publication failed")
            return original_write(path, value)

        with patch.object(target_resume, "write_json", new=write), self.assertRaises(OSError):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )

        failed = read_json(next(f.output.glob("resume-initialize-*.failure.json")))
        active = failed["active_operation"]
        self.assertEqual(failed["unknown_write_operation"], "tenant-data-dedicated-b-up")
        self.assertEqual([item["id"] for item in failed["confirmed_operations"]],
                         ["tenant-data-dedicated-a-verify"])
        self.assertEqual(active["id"], "tenant-data-dedicated-b-up")
        self.assertEqual(active["index"], 4)
        self.assertEqual(len(active["command_receipts"]), 1)
        self.assertEqual(active["command_receipts"][0], receipt_binding(
            Path(active["command_receipts"][0]["path"])))
        calls = len(f.calls)
        with self.assertRaises(ValueError):
            resume_initialize_target(
                f.root, f.output, f.run, request_descriptor=descriptor
            )
        self.assertEqual(len(f.calls), calls)



if __name__ == "__main__":
    unittest.main()
