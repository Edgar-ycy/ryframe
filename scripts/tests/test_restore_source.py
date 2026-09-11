import contextlib
import copy
import datetime as dt
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_build
import restore_reference
import restore_source as source
import source_inventory
from restore_reference_fixture import environment, inventory


class SourceRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = restore_reference.work_directory(self.plan)
        self.snapshot = {"head": "a" * 40, "clean": True, "patch_sha256": "b" * 64, "files": []}
        self.inventory = self.source_inventory()
        self.context = {
            "commands": {role: restore_build.build_command(role) for role in restore_build.ROLES},
            "profile": "dev", "target": "x86_64-pc-windows-msvc", "jobs": "cargo-default",
            "toolchain": {"cargo": "cargo fixture", "rustc": "rustc fixture"},
            "environment": {"variables": [], "sha256": source_inventory.canonical_digest([])},
        }
        self.build = {"format_version": 2, "kind": "restore-backend-build",
                      "sources": source_inventory.build_source_domains(self.inventory, "backend"),
                      "build": self.context, "artifacts": {}}
        self.identities = {}
        for index, (role, (feature, name)) in enumerate(restore_build.ROLES.items()):
            binary = self.backend / name
            binary.write_bytes(name.encode())
            self.build["artifacts"][role] = {"executable": str(binary), **source.file_digest(binary),
                "command": ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                            "--features", feature, "--bin", name, "--message-format=json"]}
            self.identities[role] = {"pid": index + 100, "created_at": "exact-creation", "executable": str(binary)}
        self.runtime = {"scope_id": self.plan["source"]["scope_id"], "worker_ready_url": "http://127.0.0.1:19200/readyz",
                        "artifacts": {role: {"path": value["executable"], "sha256": value["sha256"]}
                                      for role, value in self.build["artifacts"].items()}}
        self.dataset = {"format_version": 1, "plan_sha256": source.plan_hash(self.plan),
                        "source_scope_id": self.plan["source"]["scope_id"], "records": 100_000,
                        "object_bytes": 1024**3, "completed_at": "2026-01-01T00:00:00Z", "tenants": []}
        for index in range(11):
            tenant = "system" if index == 0 else f"{self.plan['source']['scope_id']}-{index:02d}"
            self.dataset["tenants"].append({"tenant_id": tenant, "records": 9091 if index < 10 else 9090,
                "posts": [{"id": str(i + 1)} for i in range(3)],
                "files": [{"bytes": 4 * 1024**2} for _ in range(246 if index == 0 else 1)]})
        self.plan_path = self.write("plan.json", self.plan)
        self.build_path = self.write("build.json", self.build)
        self.dataset_path = self.write("dataset.json", self.dataset)
        self.stop = False
        self.addCleanup(patch.stopall)
        patch.object(restore_build, "source_snapshot", side_effect=lambda _: self.snapshot).start()
        patch.object(restore_build, "capture_inventory", side_effect=lambda *_: self.source_inventory()).start()
        patch.object(restore_build, "build_context", return_value=self.context).start()
        patch.object(source, "verify_runtime", side_effect=lambda *_: copy.deepcopy(self.runtime)).start()
        self.binding = patch.object(source, "source_binding", return_value={"scope_id": self.plan["source"]["scope_id"],
                                                                            "sha256": "e" * 64}).start()
        patch.object(source, "read_process", side_effect=lambda _dir, role, _scope: copy.deepcopy(self.identities[role])).start()
        self.process = patch.object(source, "process_identity", side_effect=self.identity).start()
        self.listener = patch.object(source, "verify_listener").start()
        response = Mock(status=200)
        handle = Mock()
        handle.__enter__ = Mock(return_value=response)
        handle.__exit__ = Mock(return_value=False)
        patch.object(source.urllib.request, "build_opener", return_value=Mock(open=Mock(return_value=handle))).start()
        self.tools = patch.object(source, "ExternalTools").start().return_value
        self.tools.command.return_value = ["node"]
        self.tools.execute.side_effect = self.execute_node

    def source_inventory(self):
        return {
            "source": {"snapshot": copy.deepcopy(self.snapshot),
                       "worktree_fingerprint": "sha256:" + "c" * 64},
            "files": [],
            "guard": {"head": self.snapshot["head"], "index_sha256": "d" * 64,
                      "modes_sha256": "e" * 64},
        }

    def write(self, name, value):
        path = self.work / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def identity(self, pid):
        return None if self.stop else next((copy.deepcopy(item) for item in self.identities.values() if item["pid"] == pid), None)

    def execute_node(self, *_args, **_kwargs):
        result = source.expected_result(self.plan, self.dataset, source.file_digest(self.dataset_path)["sha256"])
        return subprocess.CompletedProcess([], 0, stdout=json.dumps(result).encode())

    def verify(self):
        return source.verify_source(self.backend, self.plan_path, self.build_path, self.dataset_path)

    def stopped(self, receipt=None):
        receipt = receipt or self.verify()
        path = self.write("source-runtime.json", receipt)
        self.stop = True
        self.quiescence_path = self.write("source-stopped.json", source.quiesce_source(self.backend, self.plan, path))
        capture = inventory(self.plan)
        capture.update(source_sha=self.snapshot["head"], quiesced_at=dt.datetime.now(dt.timezone.utc).isoformat(),
                       captured_at=dt.datetime.now(dt.timezone.utc).isoformat())
        return receipt, path, capture

    def test_live_verification_binds_both_actual_binaries_and_calls_source_only(self):
        receipt = self.verify()
        self.assertEqual(receipt["processes"], self.identities)
        self.assertEqual(self.listener.call_count, 4)
        command = self.tools.execute.call_args.args[0]
        self.assertEqual(command[command.index("--side") + 1], "source")
        self.assertIn("--write", command)
        self.assertEqual(self.tools.execute.call_args.kwargs["env"]["RYFRAME_PYTHON"], sys.executable)
        self.assertFalse(receipt["verified"]["restore_success"])
        _, path, capture = self.stopped(receipt)
        self.assertEqual(source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path),
                         {"source_runtime_sha256": source.file_digest(path)["sha256"],
                          "source_quiescence_sha256": source.file_digest(self.quiescence_path)["sha256"]})

    def test_dirty_source_wrong_binary_and_wrong_process_fail_before_business_calls(self):
        for change in (lambda: self.snapshot.update(clean=False),
                       lambda: self.identities["api"].update(executable=str(self.backend / "old-api")),
                       lambda: self.runtime.update(scope_id="other")):
            previous = copy.deepcopy((self.snapshot, self.identities, self.runtime))
            change()
            with self.assertRaises(ValueError):
                self.verify()
            self.tools.execute.assert_not_called()
            self.snapshot, self.identities, self.runtime = previous

    def test_node_failure_and_wrong_side_cannot_create_source_proof(self):
        self.tools.execute.side_effect = subprocess.CalledProcessError(1, ["node"], stderr=b"failed")
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify()
        result = json.loads(self.execute_node().stdout)
        result["side"] = "target"
        self.tools.execute.side_effect = None
        self.tools.execute.return_value = subprocess.CompletedProcess([], 0, stdout=json.dumps(result).encode())
        with self.assertRaisesRegex(ValueError, "检查侧"):
            self.verify()

    def test_physical_binding_failure_prevents_business_verification(self):
        self.binding.side_effect = ValueError("physical mismatch")
        with self.assertRaisesRegex(ValueError, "physical mismatch"):
            self.verify()
        self.tools.execute.assert_not_called()

    def test_restart_or_input_change_during_verification_is_rejected(self):
        def changed(*args, **kwargs):
            result = self.execute_node(*args, **kwargs)
            self.identities["worker"]["pid"] += 20
            return result
        self.tools.execute.side_effect = changed
        with self.assertRaisesRegex(ValueError, "代次"):
            self.verify()
        def altered(*args, **kwargs):
            result = self.execute_node(*args, **kwargs)
            self.plan_path.write_text(json.dumps(self.plan) + " ")
            return result
        self.tools.execute.side_effect = altered
        with self.assertRaisesRegex(ValueError, "输入证据"):
            self.verify()

    def test_stopped_verification_rejects_restart_pid_reuse_and_wrong_inventory_sha(self):
        _, path, capture = self.stopped()
        self.identities["worker"]["created_at"] = "new-creation"
        with self.assertRaisesRegex(ValueError, "曾重启"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)
        self.identities["worker"]["created_at"] = "exact-creation"
        self.stop = False
        with self.assertRaisesRegex(ValueError, "尚未停止"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)
        self.stop = True
        capture["source_sha"] = "c" * 40
        with self.assertRaisesRegex(ValueError, "干净 SHA"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)

    def test_stopped_verification_rejects_changed_build_dataset_and_time_order(self):
        receipt, path, capture = self.stopped()
        capture["quiesced_at"] = "2025-01-01T00:00:00Z"
        with self.assertRaisesRegex(ValueError, "时间顺序"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)
        capture["quiesced_at"] = source.read_json(self.quiescence_path)["observed_stopped_at"]
        self.dataset_path.write_text(json.dumps(self.dataset) + " ")
        with self.assertRaisesRegex(ValueError, "证据已变化"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)
        self.write("dataset.json", self.dataset)
        Path(self.build["artifacts"]["api"]["executable"]).write_bytes(b"old binary")
        with self.assertRaisesRegex(ValueError, "二进制文件"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)

    def test_declared_scale_and_tenant_samples_must_match(self):
        for change in (lambda value: value.update(records=99_999),
                       lambda value: value["tenants"][0]["files"].pop(),
                       lambda value: value["tenants"][0].update(tenant_id="target")):
            dataset = copy.deepcopy(self.dataset)
            change(dataset)
            with self.assertRaises(ValueError):
                source.expected_result(self.plan, dataset, "a" * 64)

    def test_backup_rejects_missing_source_proof_before_work_directory_or_external_access(self):
        args = ["restore_reference", "backup", "--backend-dir", str(self.backend), "--plan", str(self.plan_path),
                "--inventory", "unused.json", "--write"]
        with patch.object(sys, "argv", args), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(restore_reference, "work_directory") as work, self.assertRaises(SystemExit) as error:
            restore_reference.main()
        self.assertEqual(error.exception.code, 2)
        work.assert_not_called()
        capture = inventory(self.plan)
        with patch.object(restore_reference, "verify_stopped_source", side_effect=ValueError("unbound")), \
                self.assertRaisesRegex(ValueError, "unbound"):
            restore_reference.backup(self.plan, self.tools, self.work, capture, self.backend,
                                     self.work / "missing.json", self.work / "missing-stopped.json", self.work / "export-result.json")
        self.tools.verify_databases.assert_not_called()
        self.assertFalse((self.work / "backup").exists())

    def test_source_cli_requires_write_and_delegates_only_generation_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        args = ["restore_source", "verify", "--backend-dir", str(self.backend),
                "--source-generation", str(start), "--output", str(output)]
        with patch.object(sys, "argv", args), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(source, "execute_source_verification") as verify, \
                self.assertRaises(SystemExit) as error:
            source.main()
        self.assertEqual(error.exception.code, 2)
        verify.assert_not_called()
        result = {"output": str(output), "status": "source_runtime_verified"}
        with patch.object(sys, "argv", [*args, "--write"]), patch.object(
                source, "execute_source_verification", return_value=result) as verify, \
                patch("builtins.print") as printed:
            source.main()
        verify.assert_called_once_with(self.backend.resolve(), start, output)
        printed.assert_called_once_with(json.dumps(result))

    def test_backup_cannot_write_manifest_when_source_proof_changes_after_export(self):
        capture = inventory(self.plan)
        capture.update(captured_at="2026-01-01T00:00:00Z")
        self.tools.dump.side_effect = lambda _db, _tables, path: path.write_text("data")
        with patch.object(restore_reference, "verify_stopped_source", side_effect=["a" * 64, "b" * 64]), \
                patch.object(restore_reference, "backup_source", return_value={}), \
                patch.object(restore_reference, "require_stopped"), \
                patch.object(restore_reference, "verify_artifacts"), self.assertRaisesRegex(ValueError, "被替换"):
            restore_reference.backup(self.plan, self.tools, self.work, capture, self.backend,
                                     self.work / "source.json", self.work / "stopped.json", self.work / "export-result.json")
        self.assertFalse((self.work / "backup/manifest.json").exists())

    def test_inventory_captured_before_observed_stop_is_rejected_even_if_source_is_now_stopped(self):
        receipt = self.verify()
        early_capture = dt.datetime.now(dt.timezone.utc).isoformat()
        _, path, capture = self.stopped(receipt)
        capture.update(quiesced_at=receipt["verified_at"], captured_at=early_capture)
        with self.assertRaisesRegex(ValueError, "实际停止观察"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)

    def test_quiescence_is_bound_to_exact_source_proof_and_cannot_be_observed_while_running(self):
        receipt = self.verify()
        path = self.write("source-runtime.json", receipt)
        with self.assertRaisesRegex(ValueError, "尚未停止"):
            source.quiesce_source(self.backend, self.plan, path)
        _, path, capture = self.stopped(receipt)
        observed = source.read_json(self.quiescence_path)
        observed["source_runtime_sha256"] = "f" * 64
        self.write("source-stopped.json", observed)
        with self.assertRaisesRegex(ValueError, "停止观察收据"):
            source.verify_stopped_source(self.backend, self.plan, capture, path, self.quiescence_path)

    def test_cli_failure_propagates_without_creating_legacy_lock_or_success_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        args = ["restore_source", "verify", "--backend-dir", str(self.backend),
                "--source-generation", str(start), "--output", str(output), "--write"]
        failure = subprocess.CalledProcessError(1, ["node"], stderr=b"preserved protocol failure")
        with patch.object(sys, "argv", args), patch.object(
                source, "execute_source_verification", side_effect=failure
        ), self.assertRaises(subprocess.CalledProcessError):
            source.main()
        self.assertFalse(output.exists())
        self.assertFalse((self.work / ".reference-lock").exists())

    def test_comparison_capture_requires_write_and_binds_the_exact_export_result(self):
        export = self.write("source-export-result.json", {"published": True})
        output = self.work / "comparison-sources.json"
        roots = {
            "b0-backend": self.backend / "b0-backend",
            "b0-adapter-backend": self.backend / "b0-adapter",
            "b0-frontend": self.backend / "b0-frontend",
            "b1-backend": self.backend / "b1-backend",
            "b1-frontend": self.backend / "b1-frontend",
        }
        builds = {
            "b0-backend-build": self.backend / "b0-backend.json",
            "b0-frontend-build": self.backend / "b0-frontend.json",
            "b1-backend-build": self.backend / "b1-backend.json",
            "b1-frontend-build": self.backend / "b1-frontend.json",
        }
        args = ["restore_source", "comparison-capture", "--backend-dir", str(self.backend)]
        for name, path in {**roots, **builds}.items():
            args.extend(["--" + name, str(path)])
        args.extend(["--source-export-result", str(export), "--output", str(output)])
        with patch.object(sys, "argv", args), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(source, "capture_comparison_sources") as capture, \
                self.assertRaises(SystemExit) as error:
            source.main()
        self.assertEqual(error.exception.code, 2)
        capture.assert_not_called()

        receipt = {"format_version": 1, "kind": "restore-comparison-sources"}
        with patch.object(sys, "argv", [*args, "--write"]), \
                patch.object(source, "validate_new_output", return_value=output) as validate, \
                patch.object(source, "capture_comparison_sources", return_value=receipt) as capture, \
                patch.object(source, "write_new") as write, contextlib.redirect_stdout(io.StringIO()):
            source.main()
        validate.assert_called_once_with(output, self.backend)
        self.assertEqual(capture.call_args.kwargs["source_export_result"], {
            "path": str(export), **source.file_digest(export),
        })
        write.assert_called_once_with(output, receipt, self.backend)

    def test_comparison_verify_is_read_only_and_rejects_write_argument(self):
        receipt = self.write("comparison-sources.json", {
            "format_version": 1, "kind": "restore-comparison-sources",
        })
        result = {"source_export": {"identity_sha256": "a" * 64}}
        args = ["restore_source", "comparison-verify", "--backend-dir", str(self.backend),
                "--receipt", str(receipt)]
        with patch.object(sys, "argv", [*args, "--write"]), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(source, "verify_comparison_sources") as verify, \
                self.assertRaises(SystemExit) as error:
            source.main()
        self.assertEqual(error.exception.code, 2)
        verify.assert_not_called()
        with patch.object(sys, "argv", args), \
                patch.object(source, "verify_comparison_sources", return_value=result) as verify, \
                patch.object(source, "write_new") as write, contextlib.redirect_stdout(io.StringIO()):
            source.main()
        verify.assert_called_once_with(self.backend, source.read_json(receipt))
        write.assert_not_called()


if __name__ == "__main__":
    unittest.main()
