import json
import os
import signal
import socket
import subprocess
import sys
import time
import unittest
from pathlib import Path

from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
from full_stack_process import process_identity, terminate_owned_process
from full_stack_process_tree import (
    launch_supervised_process,
    read_process_tree,
    terminate_owned_process_tree,
    validate_process_tree_directory,
)

ROOT = Path(__file__).resolve().parents[2]


def _ignore_termination() -> None:
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM, lambda *_: None)


def _fixture_leaf(arguments: list[str]) -> None:
    pid_file, ready_file, port = Path(arguments[0]), Path(arguments[1]), int(arguments[2])
    _ignore_termination()
    listener = None
    if port:
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", port))
        listener.listen()
    pid_file.write_text(str(os.getpid()), encoding="utf-8")
    ready_file.write_text("ready", encoding="utf-8")
    while True:
        time.sleep(1)


def _fixture_child(arguments: list[str]) -> None:
    child_pid, leaf_pid, ready_file, port = arguments
    _ignore_termination()
    child = subprocess.Popen(
        [sys.executable, __file__, "__leaf", leaf_pid, ready_file, port],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    Path(child_pid).write_text(str(os.getpid()), encoding="utf-8")
    child.wait()


def _fixture_root(arguments: list[str]) -> None:
    mode, child_pid, leaf_pid, ready_file, port = arguments
    _ignore_termination()
    subprocess.Popen(
        [
            sys.executable,
            __file__,
            "__child",
            child_pid,
            leaf_pid,
            ready_file,
            port,
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and not Path(ready_file).is_file():
        time.sleep(0.02)
    if not Path(ready_file).is_file():
        raise RuntimeError("后代监听进程未就绪")
    if mode == "exit":
        raise SystemExit(17)
    while True:
        time.sleep(1)


class FullStackProcessTreeTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.directory = WorkspaceDirectory(dir=local)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.children = []

    def tearDown(self):
        tree_path = self.root / "api-tree.json"
        if tree_path.is_file():
            try:
                tree = read_process_tree(self.root, "api", "tree-test")
                terminate_owned_process_tree(tree, crash=True)
            except (OSError, ValueError):
                pass
        for identity in self.children:
            if process_identity(identity["pid"]) == identity:
                terminate_owned_process(identity, crash=True)

    @staticmethod
    def _free_port() -> int:
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            return listener.getsockname()[1]

    @staticmethod
    def _wait_for(condition, timeout: float = 10) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if condition():
                return
            time.sleep(0.02)
        raise AssertionError("等待真实进程树检查点超时")

    @staticmethod
    def _gone(identity: dict) -> bool:
        try:
            return process_identity(identity["pid"]) is None
        except PermissionError:
            return False

    def _launch(self, mode: str, role: str = "api", operation_id: str | None = None):
        port = 0 if mode == "no-port" else self._free_port()
        child_pid = self.root / "child.pid"
        leaf_pid = self.root / "leaf.pid"
        ready = self.root / "ready"
        log = (self.root / "tree.log").open("ab")
        self.addCleanup(log.close)
        process = launch_supervised_process(
            self.root,
            role,
            "tree-test",
            [
                sys.executable,
                __file__,
                "__root",
                mode,
                str(child_pid),
                str(leaf_pid),
                str(ready),
                str(port),
            ],
            self.root,
            os.environ.copy(),
            log,
            operation_id=operation_id,
        )
        self._wait_for(ready.is_file)
        return process, child_pid, leaf_pid, port

    def test_launch_uses_the_pre_registered_operation_id(self):
        operation_id = "a" * 32
        process, _child, _leaf, _port = self._launch("no-port", operation_id=operation_id)
        self.assertEqual(process.tree["operation_id"], operation_id)
        terminate_owned_process_tree(process.tree, crash=True)
        process.wait(timeout=5)

    def test_generation_directory_rejects_unknown_or_nested_files(self):
        operation = "a" * 32
        (self.root / "api.log").write_text("diagnostic", encoding="utf-8")
        (self.root / "runtime.json").write_text("{}", encoding="utf-8")
        self.assertEqual(
            validate_process_tree_directory(
                self.root, {"api": operation}, extra_files=("runtime.json",)
            ),
            ("api.log", "runtime.json"),
        )
        unknown = self.root / "foreign.json"
        unknown.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "未登记文件"):
            validate_process_tree_directory(
                self.root, {"api": operation}, extra_files=("runtime.json",)
            )
        unknown.unlink()
        nested = self.root / "nested"
        nested.mkdir()
        with self.assertRaisesRegex(ValueError, "目录、链接"):
            validate_process_tree_directory(
                self.root, {"api": operation}, extra_files=("runtime.json",)
            )

    def _identities(self, *pid_files: Path) -> list[dict]:
        identities = [process_identity(int(path.read_text(encoding="utf-8"))) for path in pid_files]
        self.assertTrue(all(identity is not None for identity in identities))
        return identities

    def test_normal_stop_escalates_and_reaps_descendants_without_touching_foreign_process(self):
        foreign = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        foreign_identity = process_identity(foreign.pid)
        self.assertIsNotNone(foreign_identity)
        self.children.append(foreign_identity)
        self.addCleanup(foreign.wait, 5)
        process, child_pid, leaf_pid, port = self._launch("wait")
        descendants = self._identities(child_pid, leaf_pid)

        self.assertTrue(terminate_owned_process_tree(process.tree))
        self.assertNotEqual(process.wait(timeout=5), 17)
        self.assertTrue(all(self._gone(item) for item in descendants))
        result_path = self.root / f"api-tree-{process.tree['operation_id']}-result.json"
        result = json.loads(result_path.read_text(encoding="utf-8"))
        self.assertEqual(result["termination"], "forced")
        self.assertEqual(process_identity(foreign.pid), foreign_identity)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", port))

    def test_product_parent_exit_closes_the_tree_and_reaps_rapid_descendants(self):
        process, child_pid, leaf_pid, port = self._launch("exit")
        descendants = self._identities(child_pid, leaf_pid)
        self.assertEqual(process.wait(timeout=10), 17)
        result_path = self.root / f"api-tree-{process.tree['operation_id']}-result.json"
        result = json.loads(result_path.read_text(encoding="utf-8"))
        self.assertEqual(result["exit_code"], 17)
        self.assertEqual(result["termination"], "natural")
        self.assertTrue(all(self._gone(item) for item in descendants))
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", port))

    def test_tampered_identity_fails_closed_and_valid_receipt_remains_recoverable(self):
        process, child_pid, leaf_pid, _ = self._launch("wait")
        descendants = self._identities(child_pid, leaf_pid)
        changed = {
            **process.tree,
            "process": {**process.tree["process"], "started": "0"},
        }
        with self.assertRaisesRegex(ValueError, "身份已变化"):
            terminate_owned_process_tree(changed, crash=True)
        self.assertTrue(all(process_identity(item["pid"]) == item for item in descendants))
        self.assertTrue(terminate_owned_process_tree(process.tree, crash=True))
        process.wait(timeout=5)
        control_path = self.root / f"api-tree-{process.tree['operation_id']}-control.json"
        control = json.loads(control_path.read_text(encoding="utf-8"))
        self.assertEqual(control["mode"], "crash")

    def test_launch_failure_does_not_publish_a_process_tree(self):
        log = (self.root / "failure.log").open("ab")
        self.addCleanup(log.close)
        with self.assertRaisesRegex(RuntimeError, "登记产品进程前退出"):
            launch_supervised_process(
                self.root,
                "api",
                "tree-test",
                [str(self.root / "missing-product")],
                self.root,
                os.environ.copy(),
                log,
            )
        self.assertFalse((self.root / "api-tree.json").exists())
        self.assertFalse((self.root / "api.json").exists())

    def test_rustfs_role_uses_the_same_container_and_reaps_listener_descendants(self):
        process, child_pid, leaf_pid, port = self._launch("wait", "rustfs")
        descendants = self._identities(child_pid, leaf_pid)
        self.children.extend(descendants)
        self.addCleanup(terminate_owned_process_tree, process.tree, crash=True)
        self.assertEqual(read_process_tree(self.root, "rustfs", "tree-test"), process.tree)
        self.assertTrue(terminate_owned_process_tree(process.tree))
        process.wait(timeout=5)
        self.assertTrue(all(self._gone(item) for item in descendants))
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", port))

    def test_monitoring_roles_use_the_same_container_and_reap_listener_descendants(self):
        for role in ("prometheus", "alertmanager", "webhook"):
            with self.subTest(role=role):
                process, child_pid, leaf_pid, port = self._launch("wait", role)
                descendants = self._identities(child_pid, leaf_pid)
                self.children.extend(descendants)
                self.addCleanup(terminate_owned_process_tree, process.tree, crash=True)
                self.assertEqual(read_process_tree(self.root, role, "tree-test"), process.tree)
                self.assertTrue(terminate_owned_process_tree(process.tree))
                process.wait(timeout=5)
                self.assertTrue(all(self._gone(item) for item in descendants))
                with socket.socket() as listener:
                    listener.bind(("127.0.0.1", port))
                for path in (child_pid, leaf_pid, self.root / "ready"):
                    path.unlink(missing_ok=True)

    def test_no_port_grandchild_is_confirmed_before_return_in_unicode_space_path(self):
        self.root = self.root / "中文 空格"
        self.root.mkdir()
        process, child_pid, leaf_pid, _ = self._launch("no-port")
        descendants = self._identities(child_pid, leaf_pid)
        self.children.extend(descendants)
        self.assertTrue(terminate_owned_process_tree(process.tree, crash=True))
        self.assertTrue(all(self._gone(item) for item in descendants))
        self.assertTrue(self._gone(process.tree["monitor"]))
        from full_stack_process_monitor import wait_members
        proof = wait_members(process.tree, timeout=0)
        self.assertTrue(all(item in proof["members"] for item in descendants))
        process.wait(timeout=5)


if __name__ == "__main__":
    if sys.argv[1:2] == ["__leaf"]:
        _fixture_leaf(sys.argv[2:])
    elif sys.argv[1:2] == ["__child"]:
        _fixture_child(sys.argv[2:])
    elif sys.argv[1:2] == ["__root"]:
        _fixture_root(sys.argv[2:])
    else:
        unittest.main()
