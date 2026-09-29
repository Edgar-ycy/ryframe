"""封存 seed 的 RustFS 启动失败证据必须完整闭合后才能重试。"""
import copy
import hashlib
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_segment as segment
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash


class FailedStorageRestartEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/tmp", prefix="failed-storage-restart-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.data = self.directory / "data"
        self.data.mkdir(parents=True)
        write_json(self.directory / "manifest.json", {"fixed": True})
        self.root = self.directory / "storage-target"
        self.output = self.root / "a0008"
        self.output.mkdir(parents=True)
        executable = str((self.directory / "rustfs.exe").resolve())
        self.request = {
            "manifest": binding(self.directory / "manifest.json"), "side": "target",
            "scope_id": "fixed-scope", "executable": {"path": executable, "sha256": "a" * 64},
            "data_directory": {"path": str(self.data), "device": self.data.stat().st_dev,
                               "inode": self.data.stat().st_ino},
            "api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201",
            "credential_files": {},
        }
        write_json(self.root / "request.json", self.request)
        self.request_binding = binding(self.root / "request.json")
        write_json(self.root / "registration.json", {
            "request": self.request_binding, "manifest": self.request["manifest"], "side": "target"})
        self.source = {"snapshot": {"head": "a" * 40, "clean": True, "files": [],
                                    "patch_sha256": hashlib.sha256(b"").hexdigest()},
                       "fingerprints": {"product": {"sha256": "b" * 64}}}
        self.attempt = {"number": 8, "stage": "storage-target", "mode": "restart",
                        "status": "failed", "result": None, "error_type": "CalledProcessError",
                        "sources": self.source, "started_at": "before", "finished_at": "after"}
        owner_identity = {"pid": 991001, "started": "12345",
                          "executable": str((self.backend / "python.exe").resolve())}
        owner = {"format_version": 1, "identity": owner_identity, "directory": str(self.directory),
                 "manifest_sha256": self.request["manifest"]["sha256"]}
        initial = {**copy.deepcopy(self.attempt), "status": "running", "finished_at": None,
                   "result": None, "error_type": None}
        self.controller_path = self.directory / "controller-0008.json"
        write_json(self.controller_path, {"format_version": 1, "kind": "devex-stage-controller",
                                          "owner": owner, "attempt": 8,
                                          "attempt_sha256": plan_hash(initial)})
        self.controller = binding(self.controller_path)
        self.process = {"pid": 24580, "started": "134339570871974524", "executable": executable}
        arguments = [executable, "server", "--address", "127.0.0.1:29200",
                     "--console-address", "127.0.0.1:29201", str(self.data)]
        environment = {"RUSTFS_CONSOLE_ENABLE": "true"}
        write_json(self.output / "intent.json", {
            "format_version": 1, "kind": "devex-clone-storage-intent", "request": self.request_binding,
            "controller": self.controller, "attempt": 8, "arguments": arguments,
            "environment": environment})
        intent = binding(self.output / "intent.json")
        write_json(self.output / "spawned.json", {"pid": self.process["pid"], "identity_pending": True})
        write_json(self.output / "process.json", {
            "format_version": 1, "role": "rustfs", "scope_id": "fixed-scope", "lifecycle": "running",
            "identity": self.process, "api_url": self.request["api_url"],
            "console_url": self.request["console_url"], "data_dir": str(self.data), "intent": intent})
        write_json(self.output / "launch.json", {
            "format_version": 1, "scope_id": "fixed-scope", "identity": self.process,
            "arguments": arguments, "environment": environment, "credential_files": {}})
        write_json(self.output / "failure-cleanup.json", {
            "intent": intent, "identity": self.process, "pid": self.process["pid"], "returncode": 0})
        (self.output / "stdout.log").write_bytes(b"closed")
        (self.output / "stderr.log").write_bytes(b"")
        self.failure_path = self.directory / "failure-0008.json"
        write_json(self.failure_path, {
            "format_version": 1, "kind": "devex-stage-failure", "attempt": 8,
            "stage": "storage-target", "mode": "restart", "error_type": "CalledProcessError",
            "frames": [{"file": file, "function": function, "line": index + 10}
                       for index, (file, function) in enumerate(segment._FAILED_RESTART_CALLS)],
            "controller": self.controller})

    def verify(self, *, idle=True):
        with patch.object(segment, "validate_request", return_value={"fixed": True}) as request, \
                patch.object(segment, "require_recorded_producer_stopped") as stopped, \
                patch.object(segment, "require_closed_port") as port:
            result = segment._verify_failed_storage_restart(
                self.backend, self.directory, self.attempt, self.request_binding, require_idle=idle)
        return result, request, stopped, port

    def test_complete_failure_binds_every_receipt_and_checks_idle_boundary(self):
        result, request, stopped, port = self.verify()
        self.assertEqual(result, (binding(self.failure_path), self.controller))
        self.assertEqual(request.call_count, 2)
        self.assertEqual([call.args[0] for call in stopped.call_args_list],
                         [read_json(self.controller_path)["owner"]["identity"], self.process])
        self.assertTrue(all(call.kwargs == {"run": subprocess.run, "counter_run": subprocess.run}
                            for call in stopped.call_args_list))
        self.assertEqual([call.args[0] for call in port.call_args_list],
                         [self.request["api_url"], self.request["console_url"]])
        self.verify(idle=False)[3].assert_not_called()

    def test_missing_changed_or_unknown_evidence_remains_blocked(self):
        original = {path: path.read_bytes() for path in self.output.iterdir() if path.is_file()}
        original[self.failure_path] = self.failure_path.read_bytes()
        cases = {
            "unknown-output": lambda: (self.output / "other.json").write_text("{}", encoding="utf-8"),
            "spawned": lambda: self.replace_json(self.output / "spawned.json",
                                                  {"pid": self.process["pid"], "identity_pending": False}),
            "missing-launch": lambda: (self.output / "launch.json").unlink(),
            "process": lambda: self.replace_json(
                self.output / "process.json",
                {**read_json(self.output / "process.json"),
                 "identity": {**self.process, "started": "134339570871974525"}}),
            "cleanup": lambda: self.replace_json(self.output / "failure-cleanup.json", {
                "intent": binding(self.output / "intent.json"), "identity": self.process,
                "pid": self.process["pid"], "returncode": 1}),
            "failure": self.change_failure,
        }
        for name, mutate in cases.items():
            with self.subTest(name=name):
                mutate()
                with self.assertRaises(ValueError):
                    self.verify()
                extra = self.output / "other.json"
                if extra.exists():
                    extra.unlink()
                for path, contents in original.items():
                    path.write_bytes(contents)

    def change_failure(self):
        value = read_json(self.failure_path)
        value["frames"][-1]["function"] = "other"
        self.replace_json(self.failure_path, value)

    @staticmethod
    def replace_json(path, value):
        path.unlink()
        write_json(path, value)

    def test_live_identity_or_occupied_port_and_other_request_are_rejected(self):
        with patch.object(segment, "validate_request", return_value={}), \
                patch.object(segment, "require_recorded_producer_stopped",
                             side_effect=[None, ValueError("process alive")]), \
                patch.object(segment, "require_closed_port"):
            with self.assertRaisesRegex(ValueError, "process alive"):
                segment._verify_failed_storage_restart(
                    self.backend, self.directory, self.attempt, self.request_binding, require_idle=True)
        with patch.object(segment, "validate_request", return_value={}), \
                patch.object(segment, "require_recorded_producer_stopped"), \
                patch.object(segment, "require_closed_port", side_effect=ValueError("port busy")), \
                self.assertRaisesRegex(ValueError, "port busy"):
            segment._verify_failed_storage_restart(
                self.backend, self.directory, self.attempt, self.request_binding, require_idle=True)
        with self.assertRaisesRegex(ValueError, "相同固定请求"):
            segment._verify_failed_storage_restart(
                self.backend, self.directory, self.attempt,
                {**self.request_binding, "sha256": "c" * 64}, require_idle=True)


if __name__ == "__main__":
    unittest.main()
