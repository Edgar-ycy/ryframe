"""统一阶段记录与进程互斥的本地回归；不连接外部资源。"""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from workspace_directory import WorkspaceDirectory
import devex_clone_run_state as state
from devex_clone_capture import read_json, write_json
from process_guard import process_guard


class RunStateTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(local, "devex-clone-run-state-")
        self.addCleanup(temporary.cleanup)
        self.backend = temporary.path
        self.directory = self.backend / ".local-tests/run"
        self.directory.mkdir(parents=True)
        write_json(self.directory / "manifest.json", {"kind": "isolated-state-fixture"})
        state.initialize_state(self.directory)

    def dead_lock(self):
        state.begin(self.directory, "copy", "resume", {"fixture": True})
        lock = self.directory / "run.lock"
        lock.mkdir()
        owner = {"format_version": 1, "identity": {"pid": 10001, "started": "123", "executable": "fixture"},
                 "directory": str(self.directory), "manifest_sha256": state.binding(self.directory / "manifest.json")["sha256"]}
        write_json(lock / "owner.json", owner)
        return state.binding(lock / "owner.json")

    def test_recovery_and_new_controller_share_one_kernel_mutex(self):
        owner = self.dead_lock()
        real_write = state.write_json
        checked = []

        def during_recovery(path, value):
            if path.name.endswith(".intent.json"):
                with self.assertRaisesRegex(ValueError, "并发"), state.run_lock(self.directory):
                    self.fail("new controller cannot race recovery")
                with self.assertRaisesRegex(ValueError, "并发"):
                    state.recover_lock(self.backend, self.directory, owner)
                checked.append(True)
            return real_write(path, value)

        with patch.object(state, "process_identity", return_value=None), patch.object(state, "write_json", side_effect=during_recovery):
            result = state.recover_lock(self.backend, self.directory, owner)
        self.assertEqual(checked, [True])
        self.assertTrue(result["copy_requires_reconciliation"])
        with state.run_lock(self.directory):
            self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")

    def test_controller_observation_distinguishes_live_dead_and_reused_pid_without_writes(self):
        descriptor = self.dead_lock()
        original = read_json(Path(descriptor["path"]))["identity"]
        before = {str(path): state.binding(path) for path in self.directory.rglob("*") if path.is_file()}
        for observed, matches, missing in ((original, True, False), (None, False, True),
                ({**original, "started": "456"}, False, False)):
            with self.subTest(observed=observed), patch.object(state, "process_identity", return_value=observed):
                result = state.controller_observation(self.directory)
            self.assertEqual(result, {"owner": descriptor, "process_matches": matches, "process_missing": missing})
            self.assertEqual({str(path): state.binding(path) for path in self.directory.rglob("*") if path.is_file()}, before)

    def test_controller_observation_rejects_missing_owner_and_changed_lock(self):
        self.dead_lock()
        with patch.object(state, "process_identity", side_effect=lambda _pid:
                (self.directory / "run.lock/unknown.txt").write_text("unknown", encoding="utf-8")), \
                self.assertRaisesRegex(ValueError, "只读观察期间变化"):
            state.controller_observation(self.directory)
        (self.directory / "run.lock/unknown.txt").unlink()
        (self.directory / "run.lock/owner.json").unlink()
        (self.directory / "run.lock").rmdir()
        with self.assertRaisesRegex(ValueError, "缺少控制器收据"):
            state.controller_observation(self.directory)

    def test_control_cleanup_preserves_changed_owner_and_unknown_files(self):
        with self.assertRaisesRegex(ValueError, "内容变化"):
            with state.run_lock(self.directory):
                (self.directory / "run.lock/unknown.txt").write_text("preserve", encoding="utf-8")
        self.assertTrue((self.directory / "run.lock/owner.json").exists())
        self.assertTrue((self.directory / "run.lock/unknown.txt").exists())

    def test_recovery_does_not_remove_owner_when_lock_has_unknown_contents(self):
        owner = self.dead_lock()
        original = state.binding(self.directory / "state.json")
        (self.directory / "run.lock/unknown.txt").write_text("preserve", encoding="utf-8")
        with patch.object(state, "process_identity", return_value=None), self.assertRaises(ValueError):
            state.recover_lock(self.backend, self.directory, owner)
        self.assertEqual(state.binding(self.directory / "state.json"), original)
        self.assertEqual(state.binding(self.directory / "run.lock/owner.json"), owner)
        self.assertEqual(list(self.directory.glob("recovered-*.json")), [])

    def test_failed_state_publication_keeps_intent_and_allows_explicit_retry(self):
        owner = self.dead_lock()
        with patch.object(state, "process_identity", return_value=None):
            with patch.object(state, "write_receipt", side_effect=OSError("fixture write failed")), self.assertRaises(OSError):
                state.recover_lock(self.backend, self.directory, owner)
            self.assertTrue((self.directory / "run.lock/owner.json").exists())
            self.assertEqual(list(self.directory.glob("recovered-*.json")), [])
            state.recover_lock(self.backend, self.directory, owner)
        self.assertEqual(len(list(self.directory.glob("recovery-*.intent.json"))), 2)
        self.assertEqual(len(list(self.directory.glob("recovered-*.json"))), 1)

    def test_cleanup_can_record_stop_when_unrelated_old_result_was_removed(self):
        number = state.begin(self.directory, "export", "run", {"fixture": True})
        state.finish(self.directory, number, result={"status": "export_verified"})
        original = read_json(self.directory / "state.json")["attempts"][0]
        (self.directory / "results/0001.json").unlink()
        with self.assertRaises(ValueError):
            state.load_state(self.directory)
        number = state.begin(self.directory, "runtime-source", "stop", {"fixture": True}, verify_results=False)
        state.finish(self.directory, number, result={"status": "stopped"}, verify_results=False)
        values = state.load_state(self.directory, verify_results=False)["attempts"]
        self.assertEqual(values[0], original)
        self.assertEqual(values[1]["status"], "passed")
        self.assertEqual(state.binding(Path(values[1]["result"]["path"])), values[1]["result"])

    def test_cleanup_still_rejects_result_path_or_digest_shape_changes(self):
        number = state.begin(self.directory, "export", "run", {"fixture": True})
        state.finish(self.directory, number, result={"status": "export_verified"})
        path = self.directory / "state.json"
        initial = read_json(path)
        for key, wrong in (("path", str(self.directory / "outside.json")), ("sha256", "not-a-digest"), ("bytes", True)):
            value = json.loads(json.dumps(initial))
            value["attempts"][0]["result"][key] = wrong
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.subTest(field=key), self.assertRaises(ValueError):
                state.load_state(self.directory, verify_results=False)

    def test_missing_process_identity_does_not_leave_an_empty_run_lock(self):
        with patch.object(state, "process_identity", return_value=None), self.assertRaises(ValueError):
            with state.run_lock(self.directory):
                self.fail("missing identity")
        self.assertFalse((self.directory / "run.lock").exists())

    def test_historical_state_requires_an_exact_unchanged_prefix(self):
        first = state.begin(self.directory, "storage-target", "restart", {"generation": 1})
        state.finish(self.directory, first, result={"status": "storage_restarted"})
        historical = state.binding(self.directory / "state.json")
        second = state.begin(self.directory, "cache-target", "restart", {"generation": 2})
        state.finish(self.directory, second, result={"status": "cache_ready"})

        proof = state.historical_state(self.directory, historical)
        self.assertEqual(proof["historical_attempts"], 1)
        self.assertEqual(proof["current"], state.binding(self.directory / "state.json"))

        value = read_json(self.directory / "state.json")
        value["attempts"][0]["sources"] = {"generation": "changed"}
        from full_stack_process import write_receipt
        write_receipt(self.directory / "state.json", value)
        with self.assertRaisesRegex(ValueError, "精确追加前缀"):
            state.historical_state(self.directory, historical)

    def test_os_guard_excludes_another_process_and_releases_on_exit(self):
        script = self.directory / "guard-child.py"
        script.write_text("from pathlib import Path\nimport sys\nfrom process_guard import process_guard\n"
                          "with process_guard(Path(sys.argv[1]), 'test.guard'):\n"
                          " print('locked', flush=True)\n sys.stdin.read(1)\n", encoding="utf-8")
        environment = dict(os.environ, PYTHONPATH=str(Path(sys.modules["process_guard"].__file__).resolve().parent))
        child = subprocess.Popen([sys.executable, str(script), str(self.directory)], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=environment,
                                 creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        try:
            self.assertEqual(child.stdout.readline().strip(), "locked")
            with self.assertRaisesRegex(ValueError, "并发"), process_guard(self.directory, "test.guard"):
                self.fail("a second process acquired the same guard")
            child.communicate("x", timeout=5)
            self.assertEqual(child.returncode, 0)
            with process_guard(self.directory, "test.guard"):
                self.assertTrue((self.directory / "test.guard").exists())
        finally:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
