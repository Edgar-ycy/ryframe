"""cgroup CI 的权限、精确归属、失败清理和发布接线回归；不连接 Linux 或创建真实 cgroup。"""

import copy
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ci_devex_cgroup as gate
from release_evidence import EvidenceError, validate_run
from workspace_directory import WorkspaceDirectory
from verify_release_ci import requirements

REPOSITORY = Path(__file__).resolve().parents[2]


class CgroupTests(unittest.TestCase):
    def setUp(self):
        self.directory = WorkspaceDirectory(REPOSITORY / ".local-tests/python-unit", "cgroup-")
        self.addCleanup(self.directory.cleanup)
        self.backend = Path(self.directory.name)
        self.mount = self.backend / "cgroup"
        self.mount.mkdir()
        self.output = self.backend / ".local-tests/cgroup"
        self.output.mkdir(parents=True)
        for attribute, value in (("ROOT", self.mount), ("BACKEND", self.backend)):
            context = patch.object(gate, attribute, value)
            context.start()
            self.addCleanup(context.stop)
        self.plan = {"format_version": 1, "run_id": "123", "attempt": "2", "nonce": "a" * 32,
                     "source_sha": "b" * 40, "uid": 1001, "gid": 1001, "backend": str(self.backend),
                     "cargo": sys.executable, "environment": {"PATH": "fixed-path", "HOME": "fixed-home"},
                     "root": str(self.mount / ("ryframe-devex-ci-123-2-" + "a" * 32))}
        self.root = Path(self.plan["root"])
        self.parent = {"identity": gate.kernel_identity(self.mount), "controllers": ["memory", "pids"]}

    def created(self):
        self.root.mkdir()
        gate.write_json(self.output / "created.json", {"root": str(self.root), "run_id": "123", "attempt": "2",
                                                       "identity": gate.kernel_identity(self.root), "parent": self.parent})

    def test_plan_rejects_other_run_root_user_zero_and_unbounded_environment(self):
        gate.validate_plan(self.plan)
        for changes in ({"run_id": "../1"}, {"attempt": "0"}, {"uid": 0}, {"root": str(self.mount)},
                        {"environment": {"APP_DATABASE_PASSWORD": "never-persist"}}, {"source_sha": "unknown"}):
            with self.subTest(changes=tuple(changes)), self.assertRaises(ValueError):
                gate.validate_plan({**self.plan, **changes})

    def test_output_cannot_leave_current_ignored_workspace(self):
        self.assertEqual(gate.output_path(self.output), self.output.resolve())
        with self.assertRaises(ValueError):
            gate.output_path(self.backend / "another")

    def test_prepare_never_changes_parent_controller_or_resource_limits(self):
        with patch.object(gate, "parent_state", return_value=self.parent), patch.object(gate.os, "chown", create=True) as chown:
            result = gate.prepare(self.plan, self.output)
        self.assertFalse(result["limits_modified"])
        self.assertFalse(result["global_controller_modified"])
        self.assertFalse((self.mount / "cgroup.subtree_control").exists())
        self.assertEqual((self.root / "cgroup.subtree_control").read_text(encoding="utf-8"), "+memory")
        self.assertEqual((self.root / "measure/cgroup.subtree_control").read_text(encoding="utf-8"), "+memory")
        self.assertEqual({call.args[0] for call in chown.call_args_list}, {
            directory / name if name else directory for directory in (self.root, self.root / "measure")
            for name in ("", "cgroup.procs", "cgroup.threads", "cgroup.subtree_control")})
        with patch.object(gate, "parent_state", return_value=self.parent), self.assertRaises(FileExistsError):
            gate.prepare(self.plan, self.output)

    def test_missing_parent_memory_fails_before_creating_child(self):
        with patch.object(gate, "parent_state", side_effect=ValueError("父 cgroup 未启用 memory")), self.assertRaises(ValueError):
            gate.prepare(self.plan, self.output)
        self.assertFalse(self.root.exists())

    def test_kernel_identity_and_attempt_mismatch_prevent_cleanup(self):
        self.created()
        original = gate.read_json(self.output / "created.json")
        for change in ({"attempt": "1"}, {"identity": {"inode": -1, "device": -1}}):
            (self.output / "created.json").write_text(json.dumps({**original, **change}))
            with self.assertRaises(ValueError):
                gate.owned_root(self.plan, self.output)
        self.assertTrue(self.root.exists())

    def test_privileged_boundary_requires_original_runner_ids(self):
        with patch.object(gate.sys, "platform", "linux"), patch.object(gate.os, "geteuid", return_value=0, create=True):
            with patch.dict(os.environ, {"SUDO_UID": "1001", "SUDO_GID": "1001"}):
                gate.require_privilege(self.plan)
            with patch.dict(os.environ, {"SUDO_UID": "0", "SUDO_GID": "1001"}), self.assertRaises(ValueError):
                gate.require_privilege(self.plan)

    def test_descendants_register_by_known_ancestry_and_unknown_processes_block_cleanup(self):
        parent = {"pid": 10, "started": "100", "uid": 1001, "parent": 1}
        child = {"pid": 11, "started": "101", "uid": 1001, "parent": 10}
        grandchild = {"pid": 12, "started": "102", "uid": 1001, "parent": 11}
        unknown = {"pid": 13, "started": "103", "uid": 1001, "parent": 9}
        registry = {"10": parent}
        gate.register_children({12: grandchild, 11: child, 13: unknown, 10: parent}, registry, 1001)
        self.assertEqual(set(registry), {"10", "11", "12"})
        gate.verified_processes({12: grandchild}, registry, 1001)
        for fact in (unknown, {**grandchild, "started": "new"}, {**grandchild, "uid": 0}):
            with self.assertRaises(ValueError):
                gate.verified_processes({fact["pid"]: fact}, registry, 1001)

    def test_historical_parent_pid_cannot_authorize_child_when_parent_is_absent(self):
        parent = {"pid": 10, "started": "100", "uid": 1001, "parent": 1}
        child = {"pid": 11, "started": "101", "uid": 1001, "parent": 10}
        registry = {"10": parent}
        gate.register_children({11: child}, registry, 1001)
        self.assertEqual(registry, {"10": parent})
        with self.assertRaises(ValueError):
            gate.verified_processes({11: child}, registry, 1001)

    def test_reused_or_different_uid_parent_is_rejected_before_registering_child(self):
        parent = {"pid": 10, "started": "100", "uid": 1001, "parent": 1}
        child = {"pid": 11, "started": "101", "uid": 1001, "parent": 10}
        for current_parent in ({**parent, "started": "200"}, {**parent, "uid": 1002}):
            registry = {"10": parent}
            with self.subTest(current_parent=current_parent), self.assertRaises(ValueError):
                gate.register_children({11: child, 10: current_parent}, registry, 1001)
            self.assertNotIn("11", registry)

    def test_cleanup_unknown_process_does_not_kill_or_delete(self):
        self.created()
        unknown = {77: {"pid": 77, "started": "77", "parent": 1, "uid": 1001}}
        with patch.object(gate, "freeze", return_value={"frozen": "1"}), \
             patch.object(gate, "processes", return_value=unknown), self.assertRaises(ValueError):
            gate.cleanup(self.plan, self.output)
        self.assertTrue(self.root.exists())
        self.assertFalse((self.root / "cgroup.kill").exists())

    def test_cleanup_empty_group_removes_only_exact_created_root(self):
        self.created()
        sentinel = self.mount / "other-job"
        sentinel.mkdir()
        with patch.object(gate, "freeze", return_value={"frozen": "1"}), patch.object(gate, "processes", return_value={}), \
             patch.object(gate, "parent_state", return_value=self.parent):
            result = gate.cleanup(self.plan, self.output)
        self.assertTrue(result["root_absent"])
        self.assertTrue(sentinel.exists())
        self.assertTrue(self.mount.exists())
        gate.write_json(self.output / "cleanup.json", {"run_id": "123", "attempt": "2", **result})
        self.assertEqual(gate.cleanup(self.plan, self.output)["status"], "cleaned")
        changed = {**self.plan, "attempt": "3", "root": self.plan["root"].replace("123-2-", "123-3-")}
        with self.assertRaises(ValueError):
            gate.cleanup(changed, self.output)

    def test_freeze_must_complete_before_observing_processes_or_killing(self):
        self.created()
        with patch.object(gate, "freeze", side_effect=ValueError("freeze denied")), patch.object(gate, "processes") as observe, \
             self.assertRaisesRegex(ValueError, "freeze denied"):
            gate.cleanup(self.plan, self.output)
        observe.assert_not_called()
        self.assertFalse((self.root / "cgroup.kill").exists())
        self.assertTrue(self.root.exists())

    def test_freeze_requires_kernel_confirmation_and_preserves_failed_scope(self):
        self.created()
        (self.root / "cgroup.events").write_text("populated 1\nfrozen 1\n")
        self.assertEqual(gate.freeze(self.root)["frozen"], "1")
        self.assertEqual((self.root / "cgroup.freeze").read_text(encoding="utf-8"), "1")
        (self.root / "cgroup.events").write_text("populated 1\nfrozen 0\n")
        with patch.object(gate.time, "monotonic", side_effect=[0, 6]), self.assertRaises(ValueError):
            gate.freeze(self.root)
        self.assertTrue(self.root.exists())

    def test_missing_creation_receipt_cannot_claim_existing_group(self):
        self.assertEqual(gate.cleanup(self.plan, self.output)["status"], "not_created")
        self.root.mkdir()
        with self.assertRaises(ValueError):
            gate.cleanup(self.plan, self.output)

    def test_persisted_not_created_receipt_is_reusable_without_created_receipt(self):
        with patch.object(gate, "require_privilege"):
            self.assertEqual(gate.privileged("cleanup", self.plan, self.output)["status"], "not_created")
            first = (self.output / "cleanup.json").read_bytes()
            persisted = gate.read_json(self.output / "cleanup.json")
            self.assertEqual((persisted["run_id"], persisted["attempt"], persisted["root"]), ("123", "2", str(self.root)))
            self.assertEqual(gate.privileged("cleanup", self.plan, self.output), persisted)
        self.assertEqual((self.output / "cleanup.json").read_bytes(), first)
        self.assertFalse((self.output / "created.json").exists())
        self.assertFalse(self.root.exists())

    def test_persisted_not_created_rejects_other_run_attempt_or_root(self):
        with patch.object(gate, "require_privilege"):
            gate.privileged("cleanup", self.plan, self.output)
            for field, value in (("run_id", "124"), ("attempt", "3"), ("nonce", "b" * 32)):
                changed = {**self.plan, field: value}
                changed["root"] = str(self.mount / f"ryframe-devex-ci-{changed['run_id']}-{changed['attempt']}-{changed['nonce']}")
                with self.subTest(field=field), self.assertRaises(ValueError):
                    gate.privileged("cleanup", changed, self.output)
        self.assertEqual(gate.read_json(self.output / "cleanup.json")["run_id"], "123")
        self.assertFalse(self.root.exists())

    def test_persisted_not_created_rejects_later_resource_or_creation_evidence(self):
        with patch.object(gate, "require_privilege"):
            gate.privileged("cleanup", self.plan, self.output)
            self.root.mkdir()
            with self.assertRaises(ValueError):
                gate.privileged("cleanup", self.plan, self.output)
            self.assertTrue(self.root.exists())
            self.root.rmdir()
            gate.write_json(self.output / "created.json", {"root": str(self.root)})
            with self.assertRaises(ValueError):
                gate.privileged("cleanup", self.plan, self.output)
        self.assertTrue((self.output / "created.json").exists())
        self.assertEqual(gate.read_json(self.output / "cleanup.json")["status"], "not_created")

    def test_test_parser_rejects_missing_skipped_or_failed_case(self):
        good = f"test {gate.CASE} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 2 filtered out;"
        gate.validate_test_output(0, good)
        for code, text in ((1, good), (0, good.replace(gate.CASE, "other")),
                           (0, good.replace("1 passed", "0 passed")), (0, good.replace("0 ignored", "1 ignored"))):
            with self.assertRaises(ValueError):
                gate.validate_test_output(code, text)

    def test_cargo_runs_only_after_permanent_uid_and_gid_drop(self):
        self.created()
        (self.root / "driver").mkdir()
        (self.root / "measure").mkdir()
        order = []
        fact = {"pid": os.getpid(), "started": "1", "parent": 1, "uid": 1001}
        command = gate.cargo_command(self.plan)
        class Process:
            pid = 999
            def poll(self): return 0
            def wait(self): return 0
        def spawn(actual, **kwargs):
            order.append("cargo")
            self.assertEqual(order, ["groups", "gid", "uid", "cargo"])
            self.assertEqual(actual, command)
            self.assertEqual((self.root / "driver/cgroup.procs").read_text(encoding="utf-8"), "0")
            self.assertEqual(kwargs["env"]["RYFRAME_DEVEX_CGROUP_ROOT"], str(self.root / "measure"))
            self.assertNotIn("sudo", actual)
            kwargs["stdout"].write(f"test {gate.CASE} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;".encode())
            return Process()
        with patch.object(gate.os, "setgroups", side_effect=lambda _: order.append("groups"), create=True), \
             patch.object(gate.os, "setgid", side_effect=lambda _: order.append("gid"), create=True), \
             patch.object(gate.os, "setuid", side_effect=lambda _: order.append("uid"), create=True), \
             patch.object(gate.os, "getuid", return_value=1001, create=True), \
             patch.object(gate.os, "geteuid", return_value=1001, create=True), \
             patch.object(gate.os, "getgid", return_value=1001, create=True), \
             patch.object(gate, "process_fact", return_value=fact), patch.object(gate.subprocess, "Popen", side_effect=spawn):
            self.assertEqual(gate.run_test(self.plan, self.output)["status"], "passed")

    def test_failure_receipt_preserves_original_error(self):
        with patch.object(gate, "require_privilege"), patch.object(gate, "prepare", side_effect=PermissionError("memory denied")), \
             self.assertRaises(PermissionError):
            gate.privileged("prepare", self.plan, self.output)
        failures = list(self.output.glob("prepare-failure-*.json"))
        self.assertEqual(len(failures), 1)
        self.assertEqual(gate.read_json(failures[0])["error_type"], "PermissionError")


class CgroupWorkflowTests(unittest.TestCase):
    def test_linux_job_is_explicit_always_uploads_and_preserves_default_ignore(self):
        workflow = yaml.safe_load((REPOSITORY / ".github/workflows/extended-ci.yml").read_text(encoding="utf-8"))
        job = workflow["jobs"]["devex-cgroup"]
        self.assertEqual(job["runs-on"], "ubuntu-latest")
        self.assertNotIn("if", job)
        self.assertNotIn("continue-on-error", job)
        steps = job["steps"]
        run = next(step for step in steps if step.get("id") == "cgroup")
        self.assertEqual(run["run"], "python scripts/ci_devex_cgroup.py run --output .local-tests/ci-devex-cgroup")
        cleanup = next(step for step in steps if "cleanup --output" in step.get("run", ""))
        upload = next(step for step in steps if str(step.get("uses", "")).startswith("actions/upload-artifact@"))
        self.assertIn("always()", cleanup["if"])
        self.assertEqual(upload["if"], "${{ always() }}")
        self.assertEqual(upload["with"]["if-no-files-found"], "error")
        self.assertEqual(upload["with"]["path"], "backend/.local-tests/ci-devex-cgroup")
        self.assertIn("github.run_attempt", upload["with"]["name"])
        self.assertLess(steps.index(run), steps.index(cleanup))
        self.assertLess(steps.index(cleanup), steps.index(upload))
        source = (REPOSITORY / "xtask/tests/internal/devex_memory.rs").read_text(encoding="utf-8")
        self.assertIn('#[ignore = "需要显式提供具有 memory controller 委托权限的 RYFRAME_DEVEX_CGROUP_ROOT"]', source)

    def test_release_rejects_missing_skipped_failed_or_cancelled_memory_job(self):
        args = SimpleNamespace(backend_repository="owner/backend", frontend_repository="owner/frontend",
                               backend_sha="a" * 40, frontend_sha="b" * 40, tag="v0.13.0")
        requirement = requirements(args)[2]
        self.assertIn("Linux DevEx Cgroup Memory", requirement.jobs)
        run = {"id": 1, "run_attempt": 2, "head_sha": args.backend_sha, "status": "completed", "conclusion": "success",
               "event": "push", "head_branch": args.tag}
        jobs = [{"id": index + 1, "name": name, "run_id": 1, "run_attempt": 2, "head_sha": args.backend_sha,
                 "status": "completed", "conclusion": "success"} for index, name in enumerate(requirement.jobs)]
        validate_run(run, jobs, requirement, jobs_attempt=2)
        for outcome in ("skipped", "failure", "cancelled", None):
            failing = copy.deepcopy(jobs)
            failing[-1]["conclusion"] = outcome
            with self.subTest(outcome=outcome), self.assertRaises(EvidenceError):
                validate_run(run, failing, requirement, jobs_attempt=2)
        with self.assertRaises(EvidenceError):
            validate_run(run, jobs[:-1], requirement, jobs_attempt=2)


if __name__ == "__main__":
    unittest.main()
