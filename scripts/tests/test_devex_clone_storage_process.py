"""原生隔离子进程的失败和父进程崩溃收据；不启动 RustFS 或连接服务。"""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

import devex_clone_storage_process as process
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from full_stack_process import process_identity, terminate_owned_process


class NativeProcessTests(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="sn-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.output = self.root / "attempt"
        self.output.mkdir()
        self.args = [sys.executable, "-c", "import time; time.sleep(90)"]
        self.request = {"scope_id": "native-stub", "api_url": "http://127.0.0.1:18290", "console_url": "http://127.0.0.1:18291",
                        "data_directory": {"path": str(self.root)}, "credential_files": {}, "timeout_seconds": 5,
                        "executable": {"path": str(Path(sys.executable).resolve()), "sha256": "1" * 64}}
        write_json(self.root / "request.json", self.request)
        write_json(self.root / "controller.json", {"identity": process_identity(os.getpid())})
        self.registration = binding(self.root / "request.json")
        self.controller = binding(self.root / "controller.json")
        self.addCleanup(self.cleanup_child)

    def cleanup_child(self):
        filename = self.output / "process.json"
        if filename.exists():
            expected = read_json(filename)["identity"]
            if process_identity(expected["pid"]) == expected:
                terminate_owned_process(expected)

    def test_native_child_receipt_failure_is_reaped_using_held_process(self):
        original_write = process.write_json
        child = {}
        def write(path, value):
            if path.name == "process.json":
                child.update(value["identity"])
                raise OSError("injected receipt failure")
            return original_write(path, value)
        with patch.object(process, "arguments", return_value=self.args), patch.object(process, "write_json", side_effect=write):
            with self.assertRaises(OSError):
                process.start(self.backend, self.request, dict(os.environ), self.output, self.registration, self.controller, 1, lambda: None)
        self.assertTrue(child)
        self.assertIsNone(process_identity(child["pid"]))
        cleanup = read_json(self.output / "failure-cleanup.json")
        self.assertEqual(cleanup["pid"], child["pid"])
        with patch.object(process, "arguments", return_value=self.args):
            observed = process.inspect_attempt(self.backend, self.output, self.registration, self.controller, 1)
        self.assertEqual(observed["state"], "not_started")

    def test_parent_crash_leaves_precise_child_receipt_for_explicit_recovery(self):
        launcher = self.root / "parent.py"
        code = "\n".join([
            "import json, os, sys", "from pathlib import Path",
            "sys.path[:0] = json.loads(sys.argv[1])", "import devex_clone_storage_process as p",
            "from devex_clone_capture import read_json", "from devex_clone_run_state import binding",
            "root=Path(sys.argv[2]); request=read_json(root/'request.json')",
            "p.arguments=lambda _: [sys.executable, '-c', 'import time; time.sleep(90)']",
            "p.actual_arguments=lambda *_: None", "p.wait_ready=lambda *_: os._exit(19)",
            "p.start(Path(sys.argv[3]), request, dict(os.environ), root/'attempt', binding(root/'request.json'), binding(root/'controller.json'), 1, lambda: None)",
        ])
        launcher.write_text(code, encoding="utf-8")
        paths = [str(Path(process.__file__).parent), str(self.backend / "scripts")]
        with (self.root / "parent.log").open("wb") as log:
            parent = subprocess.Popen([sys.executable, str(launcher), json.dumps(paths), str(self.root), str(self.backend)],
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                      creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            self.assertEqual(parent.wait(timeout=20), 19)
        expected = read_json(self.output / "process.json")["identity"]
        self.assertEqual(process_identity(expected["pid"]), expected)
        with patch.object(process, "arguments", return_value=self.args):
            observed = process.inspect_attempt(self.backend, self.output, self.registration, self.controller, 1)
            recovery = self.root / "recovery"
            recovery.mkdir()
            # 参数仍由本机真实 CIM 核验；测试仅隔离未使用的端口观察。
            with patch.object(process, "require_closed_port"):
                result = process.stop_record(self.request, observed, recovery)
        self.assertTrue(result["terminated"])
        self.assertIsNone(process_identity(expected["pid"]))


if __name__ == "__main__":
    unittest.main()
