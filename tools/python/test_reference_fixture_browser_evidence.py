"""业务 浏览器产物只接受有界完整清单和精确成功场景。"""

from __future__ import annotations

import os
from pathlib import Path
import sys
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import reference_fixture_browser_evidence as evidence
from devex_clone_capture import write_json
from workspace_directory import WorkspaceDirectory


class ReferenceFixtureBrowserEvidenceTests(unittest.TestCase):
    def setUp(self):
        backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=backend / ".local-tests", prefix="browser-evidence-")
        self.addCleanup(self.temporary.cleanup)
        self.parent = Path(self.temporary.name)
        self.root = self.parent / "results"
        self.root.mkdir()

    def test_manifest_is_complete_relative_bounded_and_detects_changes(self):
        (self.root / "nested").mkdir()
        (self.root / "index.html").write_text("index", encoding="utf-8")
        (self.root / "nested/trace.zip").write_bytes(b"trace")
        manifest = evidence.artifact_manifest(self.root, self.parent, "结果")
        self.assertEqual([item["path"] for item in manifest["files"]], [
            "index.html", "nested/trace.zip"
        ])
        self.assertEqual(manifest["total_bytes"], 10)
        evidence.verify_artifact_manifest(manifest, self.root, self.parent, "结果")
        (self.root / "extra.txt").write_text("extra", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "完整清单"):
            evidence.verify_artifact_manifest(manifest, self.root, self.parent, "结果")

    def test_manifest_rejects_limit_and_link_escape(self):
        (self.root / "a").write_bytes(b"a")
        (self.root / "b").write_bytes(b"b")
        with self.assertRaisesRegex(ValueError, "上限"):
            evidence.artifact_manifest(self.root, self.parent, "结果", maximum_files=1)
        outside = self.parent / "outside.txt"
        outside.write_text("outside", encoding="utf-8")
        link = self.root / "outside-link"
        try:
            link.symlink_to(outside)
        except OSError:
            self.skipTest("当前 Windows 权限不允许创建符号链接")
        with self.assertRaisesRegex(ValueError, "链接|重解析"):
            evidence.artifact_manifest(self.root, self.parent, "结果")

    def test_manifest_snapshot_detects_a_to_b_to_a(self):
        path = self.root / "asset.js"
        path.write_text("A", encoding="utf-8")
        snapshot = evidence.artifact_manifest_snapshot(self.root, self.parent, "结果")
        original = path.stat().st_mtime_ns
        path.write_text("B", encoding="utf-8")
        path.write_text("A", encoding="utf-8")
        os.utime(path, ns=(original + 1_000_000_000, original + 1_000_000_000))
        with self.assertRaisesRegex(ValueError, "替换|修改"):
            snapshot.assert_unchanged()

    def fixture_tests_receipt(self) -> dict:
        return {
            "format_version": 1, "kind": "business-browser-tests", "fixture": "business",
            "server": "dev", "run_id": "r24-business-dev", "status": "passed",
            "runs": [
                {"title": ["真实业务数据从 shared-control 复制校验并切换到 shared"],
                 "status": "passed", "retry": 0, "scenarios": ["shared-migration"]},
                {"title": ["真实业务数据从 dedicated-a 复制校验并切换到 dedicated-b"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["dedicated-migration", "retention"]},
                {"title": ["真实排队业务迁移取消恢复源数据，并允许再次迁移"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["cancellation"]},
                {"title": ["真实业务复制阻塞时 Worker 崩溃，重启后同一迁移恢复并完成校验"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["crash-recovery"]},
            ],
        }

    def test_business_receipt_requires_exact_four_runs_and_five_scenarios(self):
        path = self.root / "business-tests.json"
        write_json(path, self.fixture_tests_receipt())
        verified = evidence.business_tests(path, "dev", "r24-business-dev")
        self.assertEqual(verified["receipt"]["status"], "passed")
        changed = self.fixture_tests_receipt()
        changed["runs"].append({"title": ["extra"], "status": "passed", "retry": 0,
                                "scenarios": []})
        path.unlink()
        write_json(path, changed)
        with self.assertRaisesRegex(ValueError, "本次运行"):
            evidence.business_tests(path, "dev", "r24-business-dev")

        changed = self.fixture_tests_receipt()
        changed["runs"][0]["title"] = ["任意前缀", *changed["runs"][0]["title"]]
        path.unlink()
        write_json(path, changed)
        with self.assertRaisesRegex(ValueError, "无效标题"):
            evidence.business_tests(path, "dev", "r24-business-dev")

    def test_login_budget_binds_scope_capacity_and_nonempty_first_write(self):
        path = self.root / "login-budget.json"
        now = int(time.time() * 1_000)
        binding = {"scope_id": "fixture-source", "rate_limits": {
            "login": {"capacity": 7, "window_secs": 30}}}
        value = {
            "version": 1,
            "binding": {"scope": "fixture-source", "capacity": 7, "windowMs": 30_000},
            "observedAt": now,
            "buckets": {"principal:" + "a" * 64: {
                "generation": "00000000-0000-0000-0000-000000000000", "count": 1,
                "reservedAt": now, "completedAt": now,
            }},
        }
        write_json(path, value)
        self.assertEqual(evidence.login_budget(path, binding)["ledger"], value)
        value["buckets"] = {}
        path.unlink()
        write_json(path, value)
        with self.assertRaisesRegex(ValueError, "首次浏览器写入"):
            evidence.login_budget(path, binding)

    def test_log_consumer_rejects_a_secret_even_with_a_matching_descriptor(self):
        path = self.root / "browser.log"
        path.write_text("request AdminSecret\n", encoding="utf-8")
        descriptor = evidence.artifact_snapshot(path).descriptor()
        with self.assertRaisesRegex(ValueError, "未脱敏"):
            evidence.verify_redacted_log(path, descriptor, ("AdminSecret",))


if __name__ == "__main__":
    unittest.main()
