"""运行 Node 内建测试，所有 HTTP 测试只访问自身创建的 loopback 服务。"""
import subprocess
import unittest
from pathlib import Path


class RuntimeDriverTests(unittest.TestCase):
    def test_runtime_driver(self):
        root = Path(__file__).resolve().parents[2]
        result = subprocess.run(["node", "--test", "--test-isolation=none", "--test-concurrency=1",
                                 "scripts/tests/devex-runtime.test.mjs",
                                 "scripts/tests/devex-telemetry.test.mjs",
                                 "scripts/tests/devex-homepage.test.mjs",
                                 "scripts/tests/devex-job-timing.test.mjs",
                                 "scripts/tests/devex-job-wait.test.mjs",
                                 "scripts/tests/devex-import-samples.test.mjs",
                                 "scripts/tests/devex-provenance.test.mjs",
                                 "scripts/tests/devex-selection.test.mjs",
                                 "scripts/tests/devex-config.test.mjs",
                                 "scripts/tests/devex-pacing.test.mjs",
                                 "scripts/tests/devex-cycle-evidence.test.mjs"],
                                cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
