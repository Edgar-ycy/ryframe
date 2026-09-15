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


class ObservationFailureTests(unittest.TestCase):
    def setUp(self):
        self.c = CollectionFailureTests()
        self.c.setUp()
        self.addCleanup(self.c.doCleanups)
        self.backend, self.directory = self.c.backend, self.c.directory
        self.failed = copy.deepcopy(self.c.failed)
        self.failed["number"] = 59
        self.failed["sources"]["snapshot"]["head"] = "a387e3669f18499b8428906262557b307ce630c4"
        self.owner = copy.deepcopy(self.c.owner)
        self.owner["identity"]["pid"] += 1
        self.controller, self.failure = self.directory / "controller-0059.json", self.directory / "failure-0059.json"
        running = {**self.failed, "status": "running", "finished_at": None, "error_type": None}
        write_json(self.controller, {"format_version": 1, "kind": "devex-stage-controller", "owner": self.owner,
                                    "attempt": 59, "attempt_sha256": plan_hash(running)})
        write_json(self.failure, {"format_version": 1, "kind": "devex-stage-failure", "attempt": 59,
            "stage": "seed-runtime", "mode": prelaunch.RECOVER, "error_type": "CalledProcessError",
            "frames": copy.deepcopy(prelaunch.OBSERVATION_FRAMES), "controller": binding(self.controller)})
        self.output = self.directory / "seed-runtime/attempt-0059"
        for folder in ("before/databases", "runtime"):
            (self.output / folder).mkdir(parents=True)
        for name in ("runtime/binaries.json", "runtime/runtime.json", "before/schema-before.json", "before/databases/inventory.json"):
            write_json(self.output / name, {"previous_read_only_capture": name})
        self.binary = self.directory / "old-maintenance/ryframe-tenant-data.exe"
        self.binary.parent.mkdir()
        self.binary.write_bytes(b"audited parser before configuration or database access")
        self.maintenance = self.binary.parent / "build.json"
        source = copy.deepcopy(self.c.failed["sources"])
        source["snapshot"]["head"] = "358a4f579cbcecb2fbcc0425b0ab07250bd156f2"
        write_json(self.maintenance, {"source": {key: source[key] for key in ("snapshot", "worktree_fingerprint")},
            "backend_root": str(self.backend), "artifacts": {"tenant-data": {"executable": str(self.binary),
                **{key: binding(self.binary)[key] for key in ("bytes", "sha256")}}}})
        self.command = self.output / "before/inventory-before.command.json"
        write_json(self.command, {"command": [str(self.binary), "backup-inventory", "--output",
            str(self.output / "before/inventory-before.json"), "--source-sha", source["snapshot"]["head"],
            "--observed-at", "2026-09-15T08:55:13.454483+00:00"], "cwd": str(self.backend), "remote_operations": "read_only"})
        self.diagnostic = self.output / "before/inventory-before.diagnostic.json"
        write_json(self.diagnostic, {"returncode": 1, "error_type": "CalledProcessError", "stdout": "",
                                    "stderr": 'Error: Validation("缺少 --quiesced-at\\nusage")'})
        self.files_sha = plan_hash(_manifest(self.output))
        self.maintenance_sha = binding(self.maintenance)["sha256"]

    def context(self):
        stack = self.c.context()
        def tree(_backend, _operation, revision):
            trees = {fixtures.AUDITED_HEAD: fixtures.AUDITED_TREE,
                     "d781c20fdf5d836a49e0b899eced58634fe2bd4d": prelaunch.COLLECTION_TREE,
                     "a387e3669f18499b8428906262557b307ce630c4": prelaunch.OBSERVATION_TREE,
                     "358a4f579cbcecb2fbcc0425b0ab07250bd156f2": prelaunch.OBSERVATION_PRODUCT_TREE}
            return trees[revision.removesuffix("^{tree}")].encode()
        stack.enter_context(patch.object(prelaunch, "git", side_effect=tree))
        stack.enter_context(patch.object(prelaunch, "OBSERVATION_FILES", self.files_sha))
        stack.enter_context(patch.object(prelaunch, "OBSERVATION_MAINTENANCE", self.maintenance_sha))
        return stack

    def prove(self):
        return prelaunch.collection_failure(self.backend, self.directory, self.c.p.start, self.failed, previous=self.c.failed)

    def test_parser_failure_binds_both_collections_and_separate_product_source(self):
        before = self.c.p.files()
        with self.context():
            result = self.prove()
        self.assertEqual(result["tree"], prelaunch.OBSERVATION_TREE)
        self.assertEqual(result["previous"]["tree"], prelaunch.COLLECTION_TREE)
        self.assertEqual(result["tenant-data"], binding(self.binary))
        self.assertEqual(result["files"], _manifest(self.output))
        self.assertEqual(before, self.c.p.files())

    def test_prior_or_current_files_binary_and_controller_changes_reject(self):
        for path in (self.c.output / "runtime/runtime.json", self.output / "before/databases/inventory.json",
                     self.diagnostic, self.controller, self.maintenance, self.binary):
            with self.subTest(path=path):
                raw = path.read_bytes()
                path.write_bytes(raw + b" ")
                with self.context(), self.assertRaises(ValueError):
                    self.prove()
                path.write_bytes(raw)
        write_json(self.output / "before/inventory-before.json", {"unexpected_complete_inventory": True})
        with self.context(), self.assertRaises(ValueError):
            self.prove()

    def test_live_controller_or_generation_directory_does_not_count_as_parser_only_failure(self):
        with self.context(), patch.object(prelaunch, "process_identity", side_effect=lambda pid:
                self.owner["identity"] if pid == self.owner["identity"]["pid"] else None), self.assertRaises(ValueError):
            self.prove()
        (self.directory / "g0059").mkdir()
        with self.context(), self.assertRaises(ValueError):
            self.prove()

    def test_later_stack_frame_cannot_claim_failure_before_database_access(self):
        value = read_json(self.failure)
        value["frames"].append({"file": "scripts/writer.py", "function": "execute", "line": 1})
        write_json(self.failure, value)
        with self.context(), self.assertRaises(ValueError):
            self.prove()

    def test_changed_arguments_or_nonempty_output_reject_even_with_local_manifest_recomputed(self):
        original = read_json(self.command)
        for index, changed in ((1, "backup-register"), (3, str(self.output / "other.json")),
                               (5, "a" * 40), (6, "--quiesced-at")):
            value = copy.deepcopy(original)
            value["command"][index] = changed
            write_json(self.command, value)
            self.files_sha = plan_hash(_manifest(self.output))
            with self.context(), self.assertRaises(ValueError):
                self.prove()
        write_json(self.command, original)
        diagnostic = read_json(self.diagnostic)
        for changed in ({"stdout": "inventory produced"}, {"returncode": 0}, {"stderr": "failed after write"}):
            write_json(self.diagnostic, {**diagnostic, **changed})
            self.files_sha = plan_hash(_manifest(self.output))
            with self.context(), self.assertRaises(ValueError):
                self.prove()

    def test_more_failed_recoveries_or_wrong_predecessor_cannot_extend_the_exception(self):
        with self.context():
            for change in ({"number": 60}, {"mode": prelaunch.START}, {"status": "passed"}, {"error_type": "ValueError"}):
                with self.subTest(change=change), self.assertRaises(ValueError):
                    prelaunch.collection_failure(self.backend, self.directory, self.c.p.start, {**self.failed, **change}, previous=self.c.failed)
            with self.assertRaises(ValueError):
                prelaunch.collection_failure(self.backend, self.directory, self.c.p.start, self.failed, previous=self.failed)

    def test_coordinator_and_parser_product_must_both_match_their_audited_trees(self):
        for head in ("a387e3669f18499b8428906262557b307ce630c4", "358a4f579cbcecb2fbcc0425b0ab07250bd156f2"):
            with self.subTest(head=head), self.context():
                original = prelaunch.git
                def changed(root, operation, revision):
                    return b"a" * 40 if revision.startswith(head) else original(root, operation, revision)
                with patch.object(prelaunch, "git", side_effect=changed), self.assertRaises(ValueError):
                    self.prove()


if __name__ == "__main__":
    unittest.main()
