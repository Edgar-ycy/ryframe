import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_release_ci import requirements

ROOT = Path(__file__).resolve().parents[2]


class DeviceWorkflowTests(unittest.TestCase):
    def setUp(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/extended-ci.yml").read_text(encoding="utf-8"))
        self.job = workflow["jobs"]["full-stack-e2e"]
        self.matrix = self.job["strategy"]["matrix"]["include"]
        self.steps = {step["name"]: step for step in self.job["steps"]}

    def test_release_requires_every_full_stack_fixture_job(self):
        args = SimpleNamespace(backend_repository="owner/backend", frontend_repository="owner/frontend",
                               backend_sha="a" * 40, frontend_sha="b" * 40, tag="v0.13.0")
        required = requirements(args)[2]
        self.assertEqual({entry["fixture"] for entry in self.matrix}, {"core", "device"})
        self.assertEqual(set(required.jobs), {entry["name"] for entry in self.matrix} | {"Linux DevEx Cgroup Memory"})
        self.assertFalse(self.job["strategy"]["fail-fast"])

    def test_fixture_generation_is_between_source_receipt_and_service_preparation(self):
        names = list(self.steps)
        record = names.index("记录本次全栈源码组合")
        generate = names.index("在隔离工作树生成 Device 并校验幂等性")
        prepare = names.index("构建并安全初始化临时全栈环境")
        self.assertLess(record, generate)
        self.assertLess(generate, prepare)
        record_command = self.steps["记录本次全栈源码组合"]["run"]
        self.assertIn("cargo xtask check release ci record-pair", record_command)
        self.assertIn("--output", record_command)
        self.assertIn('--frontend-dir "$GITHUB_WORKSPACE/frontend"', record_command)
        self.assertEqual(
            self.steps["记录本次全栈源码组合"]["working-directory"], "backend"
        )
        verify_command = self.steps["复核全栈源码组合未变化"]["run"]
        self.assertIn("cargo xtask check release ci verify-pair", verify_command)
        self.assertIn("--input", verify_command)
        self.assertIn('--frontend-dir "$GITHUB_WORKSPACE/frontend"', verify_command)
        self.assertEqual(
            self.steps["复核全栈源码组合未变化"]["working-directory"], "backend"
        )
        self.assertLess(
            names.index("构建前端生产产物"),
            names.index("复核全栈源码组合未变化"),
        )
        self.assertLess(
            names.index("复核全栈源码组合未变化"),
            names.index("保存全栈诊断产物"),
        )
        for name in ("构建并安全初始化临时全栈环境", "启动真实 API、Worker 并等待就绪"):
            self.assertEqual(self.steps[name]["working-directory"], "${{ matrix.backend }}")
        production = self.steps["构建前端生产产物"]
        self.assertNotIn("if", production)
        self.assertIn("corepack pnpm build", production["run"])
        self.assertNotIn("pnpm check", production["run"])
        self.assertEqual(production["working-directory"], "${{ matrix.frontend }}")

    def test_both_attempt_scoped_artifacts_keep_hidden_browser_evidence(self):
        upload = self.steps["保存全栈诊断产物"]
        self.assertEqual(upload["if"], "${{ always() }}")
        self.assertTrue(upload["with"]["include-hidden-files"])
        template = upload["with"]["name"]
        names = set()
        for entry in self.matrix:
            name = template.replace("${{ github.run_id }}", "123").replace("${{ github.run_attempt }}", "2")
            names.add(name.replace("${{ matrix.artifact_suffix }}", entry["artifact_suffix"]))
        self.assertEqual(names, {"ryframe-full-stack-123-2", "ryframe-full-stack-123-2-device"})
        self.assertIn("device-fixture/fixture.json", upload["with"]["path"])
        self.assertNotIn("playwright-real", upload["with"]["path"])


if __name__ == "__main__":
    unittest.main()
