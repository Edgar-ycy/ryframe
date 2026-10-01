"""浏览器首次会话前运行收据不能与已登记 launch、构建、进程分离。"""

import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import restore_runtime
import restore_runtime_static as static
import test_restore_runtime as runtime_tests
from restore_runtime_evidence import read_json_document


class StaticRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.fixture = runtime_tests.RestoreRuntimeTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.authority, self.receipt, bindings_path = self.fixture.bind_receipt()
        self.manifest = read_json_document(bindings_path).value["manifest"]
        self.target = {"product_plan": read_json_document(bindings_path).value["record"]["plan"],
                       "product_execution": {
                           "roots": {"source_backend": str(self.fixture.backend),
                                     "execution_backend": str(self.fixture.backend), "frontend": str(self.fixture.frontend)},
                           **{key: self.authority[key] for key in ("backend_product_sha", "backend_execution_sha", "frontend_sha")},
                           "adapter": None,
                           "builds": {role: restore_runtime._descriptor(read_json_document(Path(self.receipt["paths"][key])))
                                      for role, key in (("backend", "backend_build"), ("frontend", "frontend_build"))}}}
        self.target_path = self.fixture.write_json(self.fixture.root / "target.json", self.target)
        self.target_doc = read_json_document(self.target_path)
        self.registration = {"target_plan": restore_runtime._descriptor(self.target_doc)}
        self.reference = {"target": {"frontend_url": self.authority["frontend_endpoint"]}}
        launch = read_json_document(self.fixture.launch_path).value
        launch["request"]["artifacts"] = self.receipt["backend"]["artifacts"]
        for role in restore_runtime.ROLES:
            process = self.receipt["processes"][role]
            document = read_json_document(Path(process["receipt_path"]))
            launch["processes"][role]["process_receipt"] = {
                **restore_runtime._descriptor(document), "identity": process["identity"]}
        self.fixture.write_json(self.fixture.launch_path, launch)
        self.launch_doc = read_json_document(self.fixture.launch_path)
        self.receipt["digests"]["launch"] = self.launch_doc.sha256
        self.enterContext(patch.object(restore_runtime, "validate_launch", side_effect=lambda value, _path: value))
        self.control = self.enterContext(patch.object(static, "_bind_control_inputs", return_value=(
            self.fixture.root, self.registration, launch, (self.launch_doc,))))
        for name in ("_observe_processes", "_probe_api", "_probe_worker", "_verify_frontend_artifacts", "resolve_runtime_sources"):
            self.enterContext(patch.object(restore_runtime, name, side_effect=AssertionError("预检不得执行网络、探针或重建")))

    def verify(self, receipt=None):
        return static.verify_static_runtime(self.fixture.backend, receipt or self.receipt, self.target_doc,
                                            self.target, self.reference, self.manifest)

    def test_accepts_exact_static_chain_without_network_or_writes(self):
        before = {str(path): path.read_bytes() for path in self.fixture.root.rglob("*") if path.is_file()}
        self.assertEqual(self.verify(), self.authority)
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.fixture.root.rglob("*") if path.is_file()})
        self.control.assert_called_once()

    def test_launch_digest_and_canonical_path_are_required_before_control(self):
        for field, value in (("digests", "0" * 64), ("paths", str(self.fixture.root / "copy.json"))):
            changed = copy.deepcopy(self.receipt)
            changed[field]["launch"] = value
            if field == "paths":
                shutil.copyfile(self.fixture.launch_path, value)
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.verify(changed)
        self.control.assert_not_called()

    def test_self_consistent_runtime_substitutions_are_rejected(self):
        for mutate in (
            lambda value: value["restore"].update(plan_hash="0" * 64),
            lambda value: value["source"].update(backend_execution_sha="0" * 40),
            lambda value: value["endpoints"].update(api="http://127.0.0.1:18081/readyz"),
            lambda value: value["processes"]["api"]["identity"].update(pid=999),
            lambda value: value["processes"]["worker"].update(receipt_sha256="0" * 64),
            lambda value: value["paths"].update(frontend_root=str(self.fixture.backend)),
            lambda value: value["backend"]["artifacts"]["api"].update(sha256="0" * 64),
        ):
            changed = copy.deepcopy(self.receipt)
            mutate(changed)
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                self.verify(changed)

    def test_product_build_target_and_launch_snapshot_cannot_diverge(self):
        self.target["product_execution"]["builds"]["backend"]["bytes"] += 1
        with self.assertRaisesRegex(ValueError, "product execution"):
            self.verify()
        self.target["product_execution"]["builds"]["backend"]["bytes"] -= 1
        self.registration["target_plan"]["bytes"] += 1
        with self.assertRaisesRegex(ValueError, "同一文件快照"):
            self.verify()

    def test_cli_without_B_does_not_create_project_bytecode(self):
        tools_python = self.fixture.root / "tools" / "python"
        tools_python.mkdir(parents=True)
        for source in runtime_tests.ROOT.joinpath("tools", "python").glob("*.py"):
            shutil.copyfile(source, tools_python / source.name)
        environment = dict(os.environ)
        environment.pop("PYTHONDONTWRITEBYTECODE", None)
        environment.pop("PYTHONPYCACHEPREFIX", None)
        result = subprocess.run([sys.executable, "-X", "utf8", str(tools_python / "restore_business_proof.py"), "--help"],
                                cwd=self.fixture.root, env=environment, stdin=subprocess.DEVNULL,
                                capture_output=True, text=True, encoding="utf-8", timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--preflight", result.stdout)
        self.assertEqual(list(tools_python.rglob("*.pyc")), [])
        self.assertEqual(list(tools_python.rglob("__pycache__")), [])


if __name__ == "__main__":
    unittest.main()
