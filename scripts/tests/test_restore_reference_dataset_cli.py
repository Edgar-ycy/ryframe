"""真实 Node 参数入口；ownership 替身在任何网络访问前返回确定结果。"""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

from workspace_directory import WorkspaceDirectory


BACKEND = Path(__file__).resolve().parents[2]
SCRIPT = BACKEND / "scripts/restore_reference_dataset.mjs"


class DatasetCliTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(BACKEND / ".local-tests/t", prefix="dc-")
        self.addCleanup(temporary.cleanup)
        self.root = temporary.path
        self.environment = {**os.environ, "RYFRAME_PYTHON": sys.executable}

    def invoke(self, *arguments):
        return subprocess.run(
            ["node", str(SCRIPT), *arguments], cwd=self.root, env=self.environment,
            capture_output=True, text=True, encoding="utf-8", timeout=30,
        )

    def test_existing_side_reaches_dataset_validation_after_exact_ownership_check(self):
        plan = {"source": {"scope_id": "source"}, "target": {"scope_id": "target"}}
        encoded = json.dumps(plan, sort_keys=True, separators=(",", ":")).encode()
        plan_path = self.root / "plan.json"
        plan_path.write_bytes(encoded)
        dataset = self.root / "dataset.json"
        dataset.write_text("{}", encoding="utf-8")
        scripts = self.root / "scripts"
        scripts.mkdir()
        (scripts / "restore_reference.py").write_text(
            "import json, sys\n"
            "side = sys.argv[sys.argv.index('--side') + 1]\n"
            f"print(json.dumps({{'plan_sha256': {hashlib.sha256(encoded).hexdigest()!r}, "
            "'side': side, 'scope_id': side}))\n", encoding="utf-8",
        )
        for side in (None, "source", "target"):
            with self.subTest(side=side):
                arguments = ["--plan", str(plan_path), "--backend-dir", str(self.root),
                             "--verify-existing", str(dataset), "--write"]
                if side is not None:
                    arguments.extend(("--side", side))
                result = self.invoke(*arguments)
                self.assertEqual(result.returncode, 1)
                self.assertIn("旧数据收据没有绑定本次参考环境计划", result.stderr)
                self.assertNotIn("ReferenceError", result.stderr)
                self.assertEqual(result.stdout, "")
        self.assertEqual(plan_path.read_bytes(), encoded)
        self.assertEqual(dataset.read_text(encoding="utf-8"), "{}")


if __name__ == "__main__":
    unittest.main()
