import json
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ci_full_stack_resources import (
    BINARIES, build_binaries, read_binaries,
)
from full_stack_provenance import BUILD_EVIDENCE, verify_build_evidence


class CiResourceManifestTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.output = self.root / "runtime"
        self.output.mkdir()
        artifacts = self.root / "中文构建 产物"
        artifacts.mkdir()
        self.binaries = {}
        for _, name in BINARIES:
            path = artifacts / (name + ".exe")
            path.write_bytes(name.encode())
            self.binaries[name] = str(path)

    def cargo_result(self, name):
        events = [
            {"reason": "compiler-message", "message": {"rendered": "检查输出"}},
            {"reason": "compiler-artifact", "target": {"name": "other-target"},
             "executable": str(self.root / "ignored.exe")},
            {"reason": "compiler-artifact", "target": {"name": name},
             "executable": self.binaries[name]},
        ]
        return SimpleNamespace(stdout="\n".join(json.dumps(event) for event in events))

    def test_manifest_uses_each_declared_cargo_artifact_without_guessing_target_paths(self):
        run = Mock(side_effect=lambda command, **_: self.cargo_result(command[command.index("--bin") + 1]))
        result = build_binaries(run, self.root, self.output)
        self.assertEqual(result, self.binaries)
        self.assertEqual(read_binaries(self.output), self.binaries)
        evidence = verify_build_evidence(self.root, self.output)
        self.assertIsNone(evidence["source"])
        self.assertEqual(set(evidence["artifacts"]), set(self.binaries))
        for name, value in evidence["artifacts"].items():
            self.assertEqual(value["path"], self.binaries[name])
        self.assertEqual(run.call_count, len(BINARIES))
        for invocation, (feature, name) in zip(run.call_args_list, BINARIES):
            self.assertEqual(invocation.args, ([
                "cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                "--features", feature, "--bin", name, "--message-format=json",
            ],))
            self.assertEqual(invocation.kwargs, {"cwd": self.root, "capture_output": True})

    def test_missing_artifact_and_command_failure_do_not_publish_partial_manifest(self):
        for failure in ("missing", "failed"):
            with self.subTest(failure=failure):
                count = 0

                def run(command, **_):
                    nonlocal count
                    count += 1
                    if count == 2:
                        if failure == "failed":
                            raise subprocess.CalledProcessError(1, command)
                        return SimpleNamespace(stdout='{"reason":"build-finished"}')
                    return self.cargo_result(command[command.index("--bin") + 1])

                error = subprocess.CalledProcessError if failure == "failed" else ValueError
                with self.assertRaises(error):
                    build_binaries(run, self.root, self.output)
                self.assertEqual(count, 2)
                self.assertFalse((self.output / "binaries.json").exists())
                self.assertFalse((self.output / BUILD_EVIDENCE).exists())

    def test_manifest_rejects_wrong_roles_relative_paths_and_missing_files_without_rewriting(self):
        first = BINARIES[0][1]
        cases = [
            {key: value for key, value in self.binaries.items() if key != first},
            self.binaries | {"unexpected": self.binaries[first]},
            self.binaries | {first: "relative.exe"},
            self.binaries | {first: str(self.root / "missing.exe")},
        ]
        path = self.output / "binaries.json"
        for value in cases:
            with self.subTest(value=value):
                raw = json.dumps(value).encode()
                path.write_bytes(raw)
                with self.assertRaises(ValueError):
                    read_binaries(self.output)
                self.assertEqual(path.read_bytes(), raw)

    def test_build_evidence_rejects_replaced_artifact_without_rewriting(self):
        run = Mock(side_effect=lambda command, **_: self.cargo_result(command[command.index("--bin") + 1]))
        build_binaries(run, self.root, self.output)
        receipt = self.output / BUILD_EVIDENCE
        original = receipt.read_bytes()
        Path(self.binaries["ryframe"]).write_bytes(b"replaced")
        with self.assertRaisesRegex(ValueError, "不匹配"):
            verify_build_evidence(self.root, self.output)
        self.assertEqual(receipt.read_bytes(), original)
