"""已审计采集失败的完整文件、来源与只读二进制闭包；不接触业务资源。"""
from contextlib import ExitStack
import copy
from pathlib import Path
import unittest
from unittest.mock import patch

import devex_clone_seed_generation_prelaunch as prelaunch
from devex_clone_capture import read_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash
from restore_source_runtime import _manifest
import test_devex_clone_seed_generation_prelaunch as fixtures

write_json = fixtures.PrelaunchProofTests.write


class CollectionFailureTests(unittest.TestCase):
    def setUp(self):
        self.p = fixtures.PrelaunchProofTests()
        self.p.setUp()
        self.addCleanup(self.p.doCleanups)
        self.backend, self.directory = self.p.backend, self.p.directory
        self.failed = {**copy.deepcopy(self.p.start), "number": 58, "mode": prelaunch.RECOVER,
                       "error_type": "CalledProcessError"}
        self.failed["sources"]["snapshot"]["head"] = "d781c20fdf5d836a49e0b899eced58634fe2bd4d"
        self.owner = copy.deepcopy(self.p.owner)
        self.owner["identity"]["pid"] += 1
        self.controller = self.directory / "controller-0058.json"
        self.failure = self.directory / "failure-0058.json"
        self.save_controller()
        self.output = self.directory / "seed-runtime/attempt-0058"
        for name in prelaunch.COLLECTION_FILES:
            path = self.output / name
            path.parent.mkdir(parents=True, exist_ok=True)
            write_json(path, {"fixture": name})
        self.migrate = self.directory / "maintenance/ryframe-migrate.exe"
        self.migrate.parent.mkdir()
        self.migrate.write_bytes(b"audited read-only migrate binary")
        self.maintenance = self.migrate.parent / "build.json"
        write_json(self.maintenance, {"source": {key: self.failed["sources"][key] for key in ("snapshot", "worktree_fingerprint")},
            "artifacts": {"migrate": {"executable": str(self.migrate),
                                     **{key: binding(self.migrate)[key] for key in ("bytes", "sha256")}}}})
        write_json(self.output / "runtime/binaries.json", {"ryframe-migrate": str(self.migrate)})
        write_json(self.output / "before/migrations-control.command.json", {
            "command": [str(self.migrate), "control", "verify"], "cwd": str(self.backend), "remote_operations": "read_only"})
        self.files = {item["path"]: item["sha256"] for item in _manifest(self.output)}
        self.maintenance_sha = binding(self.maintenance)["sha256"]

    def save_controller(self):
        running = {**self.failed, "status": "running", "finished_at": None, "error_type": None}
        write_json(self.controller, {"format_version": 1, "kind": "devex-stage-controller", "owner": self.owner,
                                     "attempt": 58, "attempt_sha256": plan_hash(running)})
        write_json(self.failure, {"format_version": 1, "kind": "devex-stage-failure", "attempt": 58,
            "stage": "seed-runtime", "mode": prelaunch.RECOVER, "error_type": "CalledProcessError",
            "frames": copy.deepcopy(prelaunch.COLLECTION_FRAMES), "controller": binding(self.controller)})

    def context(self):
        stack = ExitStack()
        stack.enter_context(self.p.context())
        def tree(_root, _operation, revision):
            value = fixtures.AUDITED_TREE if revision.startswith(fixtures.AUDITED_HEAD) else prelaunch.COLLECTION_TREE
            return value.encode()
        stack.enter_context(patch.object(prelaunch, "git", side_effect=tree))
        stack.enter_context(patch.object(prelaunch, "COLLECTION_FILES", self.files))
        stack.enter_context(patch.object(prelaunch, "COLLECTION_MAINTENANCE", self.maintenance_sha))
        return stack

    def prove(self):
        return prelaunch.collection_failure(self.backend, self.directory, self.p.start, self.failed)

    def test_exact_failed_collection_preserves_files_and_binds_source_tree_and_migrate(self):
        before = self.p.files()
        with self.context():
            value = self.prove()
        self.assertEqual(value["files"], _manifest(self.output))
        self.assertEqual(value["source"], self.failed["sources"])
        self.assertEqual(value["maintenance"], binding(self.maintenance))
        self.assertEqual(value["migrate"], binding(self.migrate))
        self.assertEqual(value["attempt"], plan_hash(self.failed))
        self.assertEqual(self.p.files(), before)

    def test_any_partial_file_change_missing_file_or_unknown_file_rejects(self):
        for name in self.files:
            with self.subTest(name=name):
                path = self.output / name
                raw = path.read_bytes()
                path.write_bytes(raw + b" ")
                with self.context(), self.assertRaises(ValueError):
                    self.prove()
                path.unlink()
                with self.context(), self.assertRaises(ValueError):
                    self.prove()
                path.write_bytes(raw)
        for name in ("intent.json", "after/image.json", "before/unexpected.json"):
            path = self.output / name
            path.parent.mkdir(parents=True, exist_ok=True)
            write_json(path, {"unknown": True})
            with self.context(), self.assertRaises(ValueError):
                self.prove()
            path.unlink()
            if path.parent.name == "after":
                path.parent.rmdir()
        (self.output / "unknown-empty").mkdir()
        with self.context(), self.assertRaises(ValueError):
            self.prove()

    def test_failure_stack_cannot_include_later_capture_or_write_path(self):
        original = read_json(self.failure)
        for frames in (original["frames"][:-1], [*original["frames"], {"file": "scripts/writer.py", "function": "execute", "line": 1}],
                       [*original["frames"][:-1], {**original["frames"][-1], "line": 56}]):
            write_json(self.failure, {**original, "frames": frames})
            with self.context(), self.assertRaises(ValueError):
                self.prove()

    def test_other_attempt_status_source_or_error_cannot_use_collection_allowance(self):
        for field, value in (("number", 59), ("status", "passed"), ("status", "running"), ("mode", prelaunch.START),
                             ("result", binding(self.failure)), ("error_type", "ValueError")):
            with self.subTest(field=field):
                original = self.failed[field]
                self.failed[field] = value
                with self.context(), self.assertRaises(ValueError):
                    self.prove()
                self.failed[field] = original
        for field, value in (("clean", False), ("patch_sha256", "a" * 64), ("files", [{"path": "changed.py"}])):
            snapshot = self.failed["sources"]["snapshot"]
            original = snapshot[field]
            snapshot[field] = value
            self.save_controller()
            with self.context(), self.assertRaises(ValueError):
                self.prove()
            snapshot[field] = original
        self.save_controller()
        with self.context(), patch.object(prelaunch, "git", side_effect=lambda _b, _a, rev:
                (fixtures.AUDITED_TREE if rev.startswith(fixtures.AUDITED_HEAD) else "a" * 40).encode()), self.assertRaises(ValueError):
            self.prove()

    def test_live_reused_controller_or_original_start_directory_rejects(self):
        for identity in (self.owner["identity"], {**self.owner["identity"], "started": "reused"}):
            with self.context(), patch.object(prelaunch, "process_identity", side_effect=lambda pid:
                    identity if pid == self.owner["identity"]["pid"] else None), self.assertRaises(ValueError):
                self.prove()
        self.p.output.mkdir()
        with self.context(), self.assertRaises(ValueError):
            self.prove()

    def test_modified_controller_maintenance_or_migrate_is_not_trusted(self):
        for path in (self.controller, self.maintenance, self.migrate):
            raw = path.read_bytes()
            path.write_bytes(raw + b" ")
            with self.context(), self.assertRaises(ValueError):
                self.prove()
            path.write_bytes(raw)

    def test_command_cannot_select_mutation_even_with_a_recomputed_local_file_digest(self):
        path = self.output / "before/migrations-control.command.json"
        value = read_json(path)
        value["command"][-1] = "up"
        write_json(path, value)
        self.files[path.relative_to(self.output).as_posix()] = binding(path)["sha256"]
        with self.context(), self.assertRaises(ValueError):
            self.prove()

    def test_partial_files_or_binary_changed_during_death_observation_rejects(self):
        for path in (self.output / "runtime/runtime.json", self.migrate):
            raw = path.read_bytes()
            def observe(pid):
                if pid == self.owner["identity"]["pid"]:
                    path.write_bytes(raw + b" ")
                return None
            with self.context(), patch.object(prelaunch, "process_identity", side_effect=observe), self.assertRaises(ValueError):
                self.prove()
            path.write_bytes(raw)


if __name__ == "__main__":
    unittest.main()
