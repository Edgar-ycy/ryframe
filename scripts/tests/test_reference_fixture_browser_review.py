"""Device 浏览器只读消费者逐项复核成功结果，不能只信路径。"""

from __future__ import annotations

from pathlib import Path
import sys
import time
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_browser_evidence as evidence
import reference_fixture_browser_review as review
from devex_clone_capture import write_json
from restore_build import file_digest
from workspace_directory import WorkspaceDirectory


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


class ReferenceFixtureBrowserReviewTests(unittest.TestCase):
    def setUp(self):
        backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=backend / ".local-tests", prefix="browser-review-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir()
        self.frontend = self.root / "frontend"
        self.frontend.mkdir()

    def outputs(self, server: str, run_id: str) -> dict[str, Path]:
        prefix = f"browser-{run_id}"
        outputs = {
            "login_budget": self.runtime / f"{prefix}-login-budget.json",
            "intent": self.runtime / f"{prefix}-intent.json",
            "result": self.runtime / f"{prefix}-result.json",
            "failure": self.runtime / f"{prefix}-failure.json",
            "browser_log": self.runtime / f"{prefix}-check.log",
            "browser_process": self.runtime / f"{prefix}-check-process",
            "report": self.frontend / f".local-tests/playwright-real/report/device/{server}/{run_id}",
            "results": self.frontend / f".local-tests/playwright-real/results/device/{server}/{run_id}",
        }
        if server == "preview":
            for name in ("build", "build_verify_before", "build_verify_after"):
                outputs[name + "_log"] = self.runtime / f"{prefix}-{name}.log"
                outputs[name + "_process"] = self.runtime / f"{prefix}-{name}-process"
            outputs["build_receipt"] = self.frontend / "dist/.vite/restore-build.json"
        return outputs

    def process(self, directory: Path, operation: str) -> dict:
        directory.mkdir()
        process = directory / "frontend.json"
        tree = directory / "frontend-tree.json"
        completion = directory / f"frontend-members-{operation}-stopped.json"
        product = {"pid": 123, "started": "1", "executable": str(process)}
        supervisor = {"pid": 124, "started": "2", "executable": str(process)}
        monitor = {"pid": 125, "started": "3", "executable": str(process)}
        write_json(process, {"format_version": 1, "role": "frontend",
                             "scope_id": "fixture-source", "identity": product})
        write_json(tree, {
            "format_version": 2, "kind": "full-stack-process-tree",
            "runtime_directory": str(directory.resolve()), "role": "frontend",
            "scope_id": "fixture-source", "operation_id": operation,
            "supervisor": supervisor, "process": product, "monitor": monitor,
            "group_id": supervisor["pid"],
        })
        write_json(completion, {"status": "stopped"})
        return {"directory": str(directory), "process": bound(process), "tree": bound(tree),
                "completion": bound(completion)}

    def fixture_tests_receipt(self, server: str, run_id: str) -> dict:
        return {
            "format_version": 1, "kind": "device-browser-tests", "fixture": "device",
            "server": server, "run_id": run_id, "status": "passed",
            "runs": [
                {"title": ["真实 Device 数据从 shared-control 复制校验并切换到 shared"],
                 "status": "passed", "retry": 0, "scenarios": ["shared-migration"]},
                {"title": ["真实 Device 数据从 dedicated-a 复制校验并切换到 dedicated-b"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["dedicated-migration", "retention"]},
                {"title": ["真实排队 Device 迁移取消恢复源数据，并允许再次迁移"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["cancellation"]},
                {"title": ["真实 Device 复制阻塞时 Worker 崩溃，重启后同一迁移恢复并完成校验"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["crash-recovery"]},
            ],
        }

    def prepare(self, server="preview", run_id="r24-device-preview"):
        outputs = self.outputs(server, run_id)
        binding_path = self.runtime / f"browser-binding-{run_id}.json"
        commands = (["build", "verify", "browser", "verify"] if server == "preview" else ["browser"])
        binding = {
            "run_id": run_id, "server": server, "scope_id": "fixture-source",
            "commands": commands, "rate_limits": {"login": {"capacity": 7, "window_secs": 30}},
        }
        write_json(binding_path, binding)
        write_json(outputs["intent"], {
            "format_version": 1, "kind": "reference-fixture-browser-intent",
            "binding": bound(binding_path), "commands": commands,
            "login_budget": {"path": str(outputs["login_budget"]), "existed_before": False},
        })
        outputs["report"].mkdir(parents=True)
        (outputs["report"] / "index.html").write_text("report", encoding="utf-8")
        outputs["results"].mkdir(parents=True)
        write_json(outputs["results"] / "device-tests.json", self.fixture_tests_receipt(server, run_id))
        now = int(time.time() * 1_000)
        write_json(outputs["login_budget"], {
            "version": 1,
            "binding": {"scope": "fixture-source", "capacity": 7, "windowMs": 30_000},
            "observedAt": now,
            "buckets": {"principal:" + "a" * 64: {
                "generation": "00000000-0000-0000-0000-000000000000", "count": 1,
                "reservedAt": now, "completedAt": now,
            }},
        })
        keys = ["browser"] if server == "dev" else [
            "build", "build_verify_before", "browser", "build_verify_after"
        ]
        logs, processes = {}, {}
        for index, name in enumerate(keys):
            outputs[name + "_log"].write_text(name, encoding="utf-8")
            logs[name] = bound(outputs[name + "_log"])
            processes[name] = self.process(outputs[name + "_process"], f"{index + 1:032x}")
        build = None
        if server == "preview":
            outputs["build_receipt"].parent.mkdir(parents=True)
            outputs["build_receipt"].write_text("{}\n", encoding="utf-8")
            (self.frontend / "dist/index.html").write_text("built", encoding="utf-8")
            build = {"receipt": bound(outputs["build_receipt"]),
                     "dist": evidence.artifact_manifest(self.frontend / "dist", self.frontend,
                                                        "Device 前端生产产物")}
        artifacts = {
            "report": evidence.artifact_manifest(
                outputs["report"], self.frontend / ".local-tests/playwright-real/report",
                "Device 浏览器 HTML 报告"),
            "results": evidence.artifact_manifest(
                outputs["results"], self.frontend / ".local-tests/playwright-real/results",
                "Device 浏览器结果"),
            "tests": evidence.device_tests(outputs["results"] / "device-tests.json", server, run_id),
        }
        result = {
            "format_version": 1, "kind": "reference-fixture-browser-result", "status": "passed",
            "run_id": run_id, "server": server, "binding": bound(binding_path),
            "intent": bound(outputs["intent"]), "build": build, "logs": logs,
            "processes": processes, "artifacts": artifacts,
            "login_budget": evidence.login_budget(outputs["login_budget"], binding),
            "remote_writes": {"business_data": True},
        }
        write_json(outputs["result"], result)
        return binding, binding_path, outputs, result

    def test_preview_consumer_checks_every_artifact_and_detects_tamper(self):
        binding, path, outputs, result = self.prepare()
        context = {"outputs": outputs, "frontend": self.frontend, "secrets": ("AdminSecret",)}
        receipt = Mock(path=outputs["build_receipt"])
        with patch.object(review, "validate_frontend_build", return_value=({}, receipt)), \
                patch.object(review, "wait_members", return_value={"status": "stopped"}) as waited:
            self.assertEqual(review.verify_browser_result(binding, context, path), result)
            self.assertEqual(waited.call_count, 4)
            (outputs["report"] / "index.html").write_text("changed", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "完整清单"):
                review.verify_browser_result(binding, context, path)

    def test_dev_consumer_rejects_an_unregistered_build(self):
        binding, path, outputs, _ = self.prepare("dev", "r24-device-dev")
        value = __import__("json").loads(outputs["result"].read_text(encoding="utf-8"))
        value["build"] = {"unexpected": True}
        outputs["result"].unlink()
        write_json(outputs["result"], value)
        context = {"outputs": outputs, "frontend": self.frontend, "secrets": ("AdminSecret",)}
        with patch.object(review, "wait_members", return_value={"status": "stopped"}), \
                self.assertRaisesRegex(ValueError, "不得绑定"):
            review.verify_browser_result(binding, context, path)

    def test_consumer_rechecks_logs_even_when_the_result_descriptor_matches(self):
        binding, path, outputs, _ = self.prepare("dev", "r24-device-dev")
        outputs["browser_log"].write_text("request AdminSecret\n", encoding="utf-8")
        value = __import__("json").loads(outputs["result"].read_text(encoding="utf-8"))
        value["logs"]["browser"] = bound(outputs["browser_log"])
        outputs["result"].unlink()
        write_json(outputs["result"], value)
        context = {"outputs": outputs, "frontend": self.frontend, "secrets": ("AdminSecret",)}
        with self.assertRaisesRegex(ValueError, "未脱敏"):
            review.verify_browser_result(binding, context, path)

    def test_consumer_rejects_process_receipt_from_an_unrelated_tree(self):
        binding, path, outputs, _ = self.prepare("dev", "r24-device-dev")
        process = __import__("json").loads(
            (outputs["browser_process"] / "frontend.json").read_text(encoding="utf-8")
        )
        process["identity"]["started"] = "unrelated"
        (outputs["browser_process"] / "frontend.json").unlink()
        write_json(outputs["browser_process"] / "frontend.json", process)
        value = __import__("json").loads(outputs["result"].read_text(encoding="utf-8"))
        value["processes"]["browser"]["process"] = bound(
            outputs["browser_process"] / "frontend.json"
        )
        outputs["result"].unlink()
        write_json(outputs["result"], value)
        context = {"outputs": outputs, "frontend": self.frontend, "secrets": ()}
        with patch.object(review, "wait_members", return_value={"status": "stopped"}), \
                self.assertRaisesRegex(ValueError, "产品进程身份"):
            review.verify_browser_result(binding, context, path)


if __name__ == "__main__":
    unittest.main()
