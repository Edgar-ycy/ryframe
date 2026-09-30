"""阶段发布的真实控制器退出及外层来源失败回归；不连接业务服务。"""
from contextlib import contextmanager
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_run as run
import devex_clone_run_state as state
from devex_clone_capture import read_json, write_json
import test_devex_clone_run as fixtures


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.RunTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.backend, self.directory, self.value = self.case.backend, self.case.directory, self.case.value

    def copy_result(self):
        output = Path(self.value["copy_directory"])
        output.mkdir()
        saved = {"initialized": self.value["initialized"], "source_export": self.value["source_export"],
                 "plan_sha256": "a" * 64, "generation_sha256": "b" * 64}
        result = {"status": "data_steps_verified", "plan_sha256": saved["plan_sha256"],
                  "generation_sha256": saved["generation_sha256"], "written_steps": 1,
                  "source_export_sha256": self.value["source_export"]["sha256"],
                  "fresh_target_sha256": self.value["initialized"]["sha256"], "pending_target_actions": []}
        write_json(output / "session.json", saved)
        write_json(output / "result.json", result)
        return result

    def test_real_exit_after_directory_lock_cleanup_is_recoverable_in_same_run(self):
        child_code = (
            "from pathlib import Path\nfrom contextlib import nullcontext\nfrom unittest.mock import patch\n"
            "import os,sys\nimport devex_clone_run as run\n"
            "with patch('source_fingerprints.current_execution_source', return_value={'fixture':'child'}), "
            "patch('source_fingerprints.artifact_sources', return_value=nullcontext()), "
            "patch.object(run,'run_export', return_value={'status':'export_verified'}), "
            "patch.object(run,'finish', side_effect=lambda *a, **k: os._exit(73)):\n"
            " run.execute(Path(sys.argv[1]), Path(sys.argv[2]), 'export', 'run')\n"
        )
        environment = dict(os.environ, PYTHONPATH=os.pathsep.join([str(Path(run.__file__).parent),
                           str(Path(sys.modules["process_guard"].__file__).parent)]))
        child = subprocess.run([sys.executable, "-c", child_code, str(self.backend), str(self.directory)],
                               env=environment, capture_output=True, timeout=15,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        self.assertEqual(child.returncode, 73, child.stderr.decode(errors="replace"))
        self.assertFalse((self.directory / "run.lock").exists())
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "running")
        descriptor = state.pending_controller(self.directory)
        self.assertEqual(Path(descriptor["path"]).name, "controller-0001.json")
        status = run.status(self.backend, self.directory)
        self.assertTrue(status["controller"]["process_missing"])
        self.assertEqual(status["controller"]["phase"], "publication_pending")
        result = state.recover_lock(self.backend, self.directory, descriptor)
        self.assertTrue(result["copy_requires_reconciliation"])
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["error_type"], "ControllerInterrupted")
        with patch.object(run, "run_export", return_value={"status": "export_verified"}):
            self.assertEqual(run.execute(self.backend, self.directory, "export", "run")["attempt"], 2)

    def test_finish_still_holds_kernel_guard_after_directory_lock_is_removed(self):
        real_finish = run.finish
        checked = []

        def publish(directory, number, **options):
            self.assertFalse((directory / "run.lock").exists())
            descriptor = state.pending_controller(directory)
            with self.assertRaisesRegex(ValueError, "并发"), state.run_lock(directory):
                self.fail("new controller entered publication")
            with self.assertRaisesRegex(ValueError, "并发"):
                state.recover_lock(self.backend, directory, descriptor)
            checked.append(True)
            return real_finish(directory, number, **options)

        with patch.object(run, "finish", side_effect=publish), \
                patch.object(run, "run_export", return_value={"status": "export_verified"}):
            run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(checked, [True])

    def test_result_file_survives_state_write_failure_and_error_finalization(self):
        real_write = state.write_receipt
        failures = []

        def write(path, value):
            if path.name == "state.json" and value["attempts"][-1]["status"] == "passed" and not failures:
                failures.append(True)
                raise OSError("fixture state publication failed after result write")
            return real_write(path, value)

        with patch.object(state, "write_receipt", side_effect=write), \
                patch.object(run, "run_export", return_value={"status": "export_verified"}):
            with self.assertRaisesRegex(OSError, "publication failed"):
                run.execute(self.backend, self.directory, "export", "run")
        attempt = state.load_state(self.directory)["attempts"][0]
        self.assertEqual(attempt["status"], "failed")
        self.assertEqual(attempt["error_type"], "OSError")
        self.assertEqual(read_json(Path(attempt["result"]["path"])), {"status": "export_verified"})

    def test_lock_directory_removal_failure_restores_owner_for_explicit_recovery(self):
        real_rmdir = Path.rmdir
        lock = self.directory / "run.lock"

        def remove(path):
            if path == lock:
                raise OSError("fixture directory removal failed")
            return real_rmdir(path)

        with patch.object(Path, "rmdir", remove), patch.object(run, "run_export", return_value={"status": "export_verified"}):
            with self.assertRaisesRegex(OSError, "directory removal failed"):
                run.execute(self.backend, self.directory, "export", "run")
        owner_binding = state.binding(lock / "owner.json")
        self.assertEqual(read_json(Path(owner_binding["path"])), read_json(self.directory / "controller-0001.json")["owner"])
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")
        with patch.object(state, "process_identity", return_value=None):
            state.recover_lock(self.backend, self.directory, owner_binding)
        self.assertFalse(lock.exists())

    def test_recovery_directory_removal_failure_keeps_exact_owner_and_can_be_retried(self):
        with state.run_lock(self.directory) as owner:
            number = state.begin(self.directory, "copy", "resume", self.case.sources)
            state.bind_controller_attempt(self.directory, number, owner)
        lock = self.directory / "run.lock"
        lock.mkdir()
        write_json(lock / "owner.json", owner)
        owner_binding = state.binding(lock / "owner.json")
        real_rmdir = Path.rmdir

        def remove(path):
            if path == lock:
                raise OSError("fixture recovery removal failed")
            return real_rmdir(path)

        with patch.object(state, "process_identity", return_value=None):
            with patch.object(Path, "rmdir", remove), self.assertRaises(OSError):
                state.recover_lock(self.backend, self.directory, owner_binding)
            self.assertEqual(state.binding(lock / "owner.json"), owner_binding)
            state.recover_lock(self.backend, self.directory, owner_binding)
        self.assertFalse(lock.exists())

    def test_different_existing_result_is_never_overwritten(self):
        number = state.begin(self.directory, "export", "run", self.case.sources)
        path = self.directory / "results/0001.json"
        write_json(path, {"original": "failure evidence"})
        before = path.read_bytes()
        with self.assertRaisesRegex(ValueError, "拒绝覆盖"):
            state.finish(self.directory, number, result={"status": "export_verified"})
        self.assertEqual(path.read_bytes(), before)

    def test_old_persistent_controller_cannot_recover_a_new_running_attempt(self):
        with patch.object(run, "run_export", return_value={"status": "export_verified"}):
            run.execute(self.backend, self.directory, "export", "run")
        descriptor = state.binding(self.directory / "controller-0001.json")
        with state.run_lock(self.directory) as owner:
            number = state.begin(self.directory, "copy", "resume", self.case.sources)
            state.bind_controller_attempt(self.directory, number, owner)
        before = state.binding(self.directory / "state.json")
        with patch.object(state, "process_identity", return_value=None), self.assertRaisesRegex(ValueError, "旧控制器"):
            state.recover_lock(self.backend, self.directory, descriptor)
        self.assertEqual(state.binding(self.directory / "state.json"), before)

    def test_persistent_owner_change_before_recovery_state_write_is_rejected(self):
        with state.run_lock(self.directory) as owner:
            number = state.begin(self.directory, "copy", "resume", self.case.sources)
            descriptor = state.bind_controller_attempt(self.directory, number, owner)
        before = state.binding(self.directory / "state.json")
        real_write = state.write_json

        def changed(path, value):
            real_write(path, value)
            if path.name.endswith(".intent.json"):
                Path(descriptor["path"]).write_text("{}", encoding="utf-8")

        with patch.object(state, "process_identity", return_value=None), patch.object(state, "write_json", side_effect=changed):
            with self.assertRaisesRegex(ValueError, "控制锁发生变化"):
                state.recover_lock(self.backend, self.directory, descriptor)
        self.assertEqual(state.binding(self.directory / "state.json"), before)
        self.assertEqual(list(self.directory.glob("recovered-*.json")), [])

    def test_persistent_owner_change_after_state_write_cannot_publish_recovered(self):
        with state.run_lock(self.directory) as owner:
            number = state.begin(self.directory, "copy", "resume", self.case.sources)
            descriptor = state.bind_controller_attempt(self.directory, number, owner)
        real_write = state.write_receipt

        def changed(path, value):
            real_write(path, value)
            if path.name == "state.json":
                Path(descriptor["path"]).write_text("{}", encoding="utf-8")

        with patch.object(state, "process_identity", return_value=None), patch.object(state, "write_receipt", side_effect=changed):
            with self.assertRaisesRegex(ValueError, "控制锁发生变化"):
                state.recover_lock(self.backend, self.directory, descriptor)
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")
        self.assertEqual(list(self.directory.glob("recovered-*.json")), [])

    def test_internal_copy_success_cannot_override_latest_outer_source_failure(self):
        result = self.copy_result()
        number = state.begin(self.directory, "copy", "resume", self.case.sources)
        state.finish(self.directory, number, result=result)

        @contextmanager
        def bad_final_source(*_args):
            yield
            raise ValueError("final complete source changed")

        with patch("source_fingerprints.artifact_sources", side_effect=bad_final_source), \
                patch.object(run, "run_copy", return_value=result):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "copy", "resume")
        self.assertEqual([item["status"] for item in state.load_state(self.directory)["attempts"]], ["passed", "failed"])
        with patch("devex_clone_ledger.CloneLedger") as ledger, self.assertRaisesRegex(ValueError, "最新复制阶段"):
            run.require_target_copy(self.backend, self.directory, self.value, ("api",))
        ledger.assert_not_called()

    def test_latest_running_copy_and_reconciliation_are_not_data_completion(self):
        result = self.copy_result()
        number = state.begin(self.directory, "copy", "resume", self.case.sources)
        with self.assertRaisesRegex(ValueError, "最新复制阶段"):
            run.require_target_copy(self.backend, self.directory, self.value, ("api",))
        state.finish(self.directory, number, result={"status": "copy_reconciled", "remote_writes": 0})
        with self.assertRaisesRegex(ValueError, "最新复制阶段结果"):
            run.require_target_copy(self.backend, self.directory, self.value, ("api",))
        self.assertEqual(read_json(Path(self.value["copy_directory"]) / "result.json"), result)

    def test_copy_result_for_another_source_is_rejected(self):
        result = self.copy_result()
        published = dict(result, generation_sha256="f" * 64)
        number = state.begin(self.directory, "copy", "resume", self.case.sources)
        state.finish(self.directory, number, result=published)
        with self.assertRaisesRegex(ValueError, "最新复制阶段结果"):
            run.require_target_copy(self.backend, self.directory, self.value, ("api",))


if __name__ == "__main__":
    unittest.main()
