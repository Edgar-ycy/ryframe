import json
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ci_full_stack_resources import BINARIES, read_binaries


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
