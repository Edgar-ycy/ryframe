"""仅证明已审计源码的启动前失败；缺目录本身不赋予重放资格。"""
from contextlib import ExitStack
import copy
import hashlib
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import devex_clone_seed_generation_prelaunch as prelaunch
from devex_clone_capture import read_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash
from source_fingerprints import execution_source
from test_source_fingerprints import inventory
from workspace_directory import WorkspaceDirectory


AUDITED_HEAD = "b1e88466ea689210a5433401c60786771d8bf833"
AUDITED_TREE = "110383e2c6859b86633c2c3b65b037a23c6367e3"
EMPTY_PATCH = hashlib.sha256(b"").hexdigest()
FRAMES = [
    {"file": "scripts/devex_clone_run.py", "function": "execute", "line": 705},
    {"file": "scripts/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 341},
    {"file": "scripts/devex_clone_seed_generation.py", "function": "execute_generation", "line": 221},
    {"file": "scripts/devex_clone_seed_generation_runtime.py", "function": "preflight", "line": 183},
    {"file": "scripts/devex_clone_seed_generation_runtime.py", "function": "checkpoint", "line": 192},
    {"file": "scripts/devex_provenance.py", "function": "verify_source", "line": 174},
    {"file": "scripts/source_fingerprints.py", "function": "reusable_artifact_source", "line": 349},
]


class PrelaunchProofTests(unittest.TestCase):
    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/t", prefix="prelaunch-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.output = self.directory / "g0057"
        self.controller_path = self.directory / "controller-0057.json"
        self.failure_path = self.directory / "failure-0057.json"
        self.write(self.directory / "manifest.json", {"kind": "fixture-run"})
        tools = inventory()
        tools["source"]["snapshot"].update(head=AUDITED_HEAD, clean=True, files=[], patch_sha256=EMPTY_PATCH)
        self.start = {
            "number": 57, "stage": "seed-runtime", "mode": "source-generation-start", "status": "failed",
            "started_at": "2026-09-15T01:00:00Z", "finished_at": "2026-09-15T01:00:03Z",
            "sources": execution_source(tools), "result": None, "error_type": "ValueError",
        }
        self.identity = {"pid": 45123, "started": "fixture-created", "executable": "D:/tools/python.exe"}
        self.owner = {"format_version": 1, "identity": self.identity, "directory": str(self.directory),
                      "manifest_sha256": binding(self.directory / "manifest.json")["sha256"]}
        self.save()

    def save(self):
        running = {**copy.deepcopy(self.start), "status": "running", "finished_at": None, "error_type": None}
        self.write(self.controller_path, {"format_version": 1, "kind": "devex-stage-controller", "owner": self.owner,
                                        "attempt": 57, "attempt_sha256": plan_hash(running)})
        self.failure = {"format_version": 1, "kind": "devex-stage-failure", "attempt": 57,
                        "stage": "seed-runtime", "mode": "source-generation-start", "error_type": "ValueError",
                        "frames": copy.deepcopy(FRAMES), "controller": binding(self.controller_path)}
        self.write(self.failure_path, self.failure)

    def context(self, *, tree=AUDITED_TREE, identity=None):
        stack = ExitStack()
        stack.enter_context(patch.object(prelaunch, "git", return_value=(tree + "\n").encode()))
        stack.enter_context(patch.object(prelaunch, "process_identity", return_value=identity))
        return stack

    def files(self):
        return {str(path.relative_to(self.directory)): path.read_bytes()
                for path in self.directory.rglob("*") if path.is_file()}

    def test_exact_audited_failure_proves_prelaunch_without_creating_files(self):
        before = self.files()
        with self.context(), patch.object(prelaunch, "git", return_value=(AUDITED_TREE + "\n").encode()) as git:
            result = prelaunch.proof(self.backend, self.directory, self.start)
        self.assertEqual(result, {"start": plan_hash(self.start), "failure": binding(self.failure_path),
                                  "controller": binding(self.controller_path), "source": self.start["sources"],
                                  "tree": AUDITED_TREE})
        self.assertEqual(git.call_args.args, (self.backend, "rev-parse", AUDITED_HEAD + "^{tree}"))
        self.assertEqual(self.files(), before)
        self.assertFalse(self.output.exists())

    def test_other_source_tree_or_nonclean_snapshot_cannot_use_old_failure_stack(self):
        with self.context(tree="a" * 40), self.assertRaises(ValueError):
            prelaunch.proof(self.backend, self.directory, self.start)
        for changed in ({"clean": False}, {"patch_sha256": "a" * 64}, {"files": [{"path": "scripts/changed.py"}]},
                        {"head": "invalid"}):
            with self.subTest(snapshot=changed):
                self.setUp()
                self.start["sources"]["snapshot"].update(changed)
                self.save()
                with self.context(), self.assertRaises(ValueError):
                    prelaunch.proof(self.backend, self.directory, self.start)

    def test_only_failed_start_without_a_published_result_is_eligible(self):
        for changed in ({"status": "passed"}, {"status": "running"}, {"error_type": "TimeoutError"},
                        {"mode": "source-generation-stop"}, {"stage": "copy"},
                        {"result": binding(self.failure_path)}):
            with self.subTest(start=changed):
                self.setUp()
                self.start.update(changed)
                self.save()
                with self.context(), self.assertRaises(ValueError):
                    prelaunch.proof(self.backend, self.directory, self.start)

    def test_failure_stack_requires_exact_files_functions_lines_and_order(self):
        variants = []
        for field, value in (("file", "scripts/other.py"), ("function", "start"), ("line", 222)):
            frames = copy.deepcopy(FRAMES)
            frames[2][field] = value
            variants.append(frames)
        variants.extend([FRAMES[:-1], [*FRAMES, FRAMES[-1]], list(reversed(FRAMES))])
        for frames in variants:
            with self.subTest(frames=frames):
                self.failure["frames"] = frames
                self.write(self.failure_path, self.failure)
                before = self.files()
                with self.context(), self.assertRaises(ValueError):
                    prelaunch.proof(self.backend, self.directory, self.start)
                self.assertEqual(self.files(), before)

    def test_failure_fields_and_controller_binding_are_not_advisory(self):
        for field, value in (("kind", "other"), ("format_version", 2), ("format_version", True), ("attempt", 56),
                             ("stage", "copy"), ("mode", "source-generation-stop"),
                             ("error_type", "RuntimeError"), ("extra", "unknown"),
                             ("controller", {**binding(self.controller_path), "sha256": "a" * 64})):
            with self.subTest(field=field):
                self.save()
                self.failure[field] = value
                self.write(self.failure_path, self.failure)
                with self.context(), self.assertRaises(ValueError):
                    prelaunch.proof(self.backend, self.directory, self.start)

    def test_controller_requires_original_running_record_and_same_owner(self):
        changes = (
            lambda value: value.update(attempt_sha256=plan_hash(self.start)),
            lambda value: value.update(format_version=True),
            lambda value: value.update(attempt=56),
            lambda value: value["owner"].update(directory=str(self.directory.parent)),
            lambda value: value["owner"].update(manifest_sha256="a" * 64),
            lambda value: value["owner"].update(format_version=2),
            lambda value: value["owner"].update(format_version=True),
            lambda value: value["owner"].update(extra="unknown"),
            lambda value: value["owner"]["identity"].update(pid=1),
            lambda value: value["owner"]["identity"].update(started=""),
            lambda value: value["owner"]["identity"].update(executable=""),
        )
        for change in changes:
            with self.subTest(change=change):
                self.save()
                controller = read_json(self.controller_path)
                change(controller)
                self.write(self.controller_path, controller)
                self.failure["controller"] = binding(self.controller_path)
                self.write(self.failure_path, self.failure)
                with self.context(), self.assertRaises(ValueError):
                    prelaunch.proof(self.backend, self.directory, self.start)

    def test_existing_generation_directory_or_intent_always_rejects(self):
        self.output.mkdir()
        with self.context(), self.assertRaises(ValueError):
            prelaunch.proof(self.backend, self.directory, self.start)
        self.write(self.output / "intent.json", {"kind": "runtime-intent"})
        before = self.files()
        with self.context(), self.assertRaises(ValueError):
            prelaunch.proof(self.backend, self.directory, self.start)
        self.assertEqual(self.files(), before)

    def test_dangling_generation_link_is_not_an_absent_directory(self):
        original = Path.is_symlink
        def linked(path):
            return path == self.output or original(path)
        with self.context(), patch.object(Path, "is_symlink", linked), self.assertRaises(ValueError):
            prelaunch.proof(self.backend, self.directory, self.start)

    def test_live_or_reused_controller_pid_cannot_be_certified_stopped(self):
        for identity in (self.identity, {**self.identity, "started": "another-created"}):
            with self.subTest(identity=identity), self.context(identity=identity), self.assertRaises(ValueError):
                prelaunch.proof(self.backend, self.directory, self.start)

    def test_generation_directory_created_during_observation_invalidates_proof(self):
        def observed(_pid):
            self.output.mkdir()
            return None
        with self.context(), patch.object(prelaunch, "process_identity", side_effect=observed), \
                self.assertRaises(ValueError):
            prelaunch.proof(self.backend, self.directory, self.start)

    def test_failure_or_controller_changed_during_observation_invalidates_proof(self):
        for filename in ("failure-0057.json", "controller-0057.json"):
            with self.subTest(filename=filename):
                self.save()
                def observed(_pid):
                    path = self.directory / filename
                    value = read_json(path)
                    value["extra"] = "changed"
                    self.write(path, value)
                    return None
                with self.context(), patch.object(prelaunch, "process_identity", side_effect=observed), \
                        self.assertRaises(ValueError):
                    prelaunch.proof(self.backend, self.directory, self.start)


if __name__ == "__main__":
    unittest.main()
