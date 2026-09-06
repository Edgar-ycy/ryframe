"""生产者 lineage 只读取显式文件证据，不访问 API、数据库或对象存储。"""

from __future__ import annotations

import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from tests.workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import devex_clone_producer_lineage as lineage
from devex_clone_producer_lineage import collect_producer_lineage
from devex_clone_run_state import binding
from restore_build import file_digest
from restore_reference_plan import plan_hash


class ProducerLineageTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary_root = self.backend / ".local-tests" / "python-unit"
        temporary_root.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=temporary_root)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.run = self.root / "run"
        self.run.mkdir()
        self.write(self.run / "manifest.json", {"format_version": 1, "kind": "fixture-run"})
        self.state = {"format_version": 1, "manifest": binding(self.run / "manifest.json"), "attempts": []}
        self.write(self.run / "state.json", self.state)
        self.artifacts = self.root / "artifacts"
        self.artifacts.mkdir()
        self.api = self.artifacts / "api.exe"
        self.worker = self.artifacts / "worker.exe"
        self.api.write_bytes(b"api-fixture")
        self.worker.write_bytes(b"worker-fixture")
        self.node = Path(sys.executable).resolve()
        self.config_sha = "a" * 64
        self.post_sequence = 0
        self.requests = {}
        registration = patch.object(
            lineage, "post_registration", side_effect=lambda *_args, **_kwargs: SimpleNamespace(
                sequence=self.post_sequence))
        registration.start()
        self.addCleanup(registration.stop)
        historical = patch.object(lineage, "historical_producer_request", side_effect=self.historical_request)
        historical.start()
        self.addCleanup(historical.stop)

    @staticmethod
    def write(path: Path, value: dict) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")

    def artifact(self, path: Path) -> dict:
        return {"path": str(path), "sha256": file_digest(path)["sha256"]}

    def historical_request(self, _backend, _run, _stage, descriptor):
        self.assertEqual(binding(Path(descriptor["path"])), descriptor)
        if descriptor["path"] not in self.requests:
            raise ValueError("fixture registration 不属于历史登记")
        return self.requests[descriptor["path"]]

    def runtime(self, name: str, *, pid: int, started: str, executable: Path | None = None) -> Path:
        directory = self.run / name
        directory.mkdir(parents=True)
        if name == "post-copy/runtime":
            sequence = 0
        else:
            sequence = int(name.removeprefix("post-copy/runtime-"))
        self.post_sequence = max(self.post_sequence, sequence)
        api = executable or self.api
        token = f"{pid:032x}"
        contract = {
            "format_version": 1,
            "backend_root": str(self.backend),
            "scope_id": "fixture-scope",
            "configuration_sha256": self.config_sha,
            "worker_ready_url": "http://127.0.0.1:19210/readyz",
            "artifacts": {"api": self.artifact(api), "worker": self.artifact(self.worker)},
        }
        self.write(directory / "runtime.json", contract)
        (directory / f"api-{token}.log").write_text("fixture log\n", encoding="utf-8")
        identity = {"pid": pid, "started": started, "executable": str(api)}
        self.write(directory / "producer-history.json", {
            "format_version": 1,
            "kind": "devex-clone-producer-history",
            "scope_id": "fixture-scope",
            "runtime_directory": str(directory),
            "events": [
                {"sequence": 0, "time_ns": 100, "event": "start-intent", "role": "api", "token": token,
                 "artifact": self.artifact(api), "ready_url": "http://127.0.0.1:18210/readyz",
                 "configuration_sha256": self.config_sha, "log": f"api-{token}.log"},
                {"sequence": 1, "time_ns": 101, "event": "started", "role": "api", "token": token,
                 "identity": identity},
                {"sequence": 2, "time_ns": 102, "event": "stopped", "role": "api", "identity": identity},
            ],
        })
        self.write(directory / "api.json", {
            "format_version": 1, "role": "api", "scope_id": "fixture-scope", "identity": identity,
        })
        return directory

    def node_attempt(self, number: int = 1) -> dict:
        stage, mode, kind = "seed-runtime", "identities-verify", "identity-verify"
        script = self.backend / "scripts" / "devex_clone_identity.mjs"
        script_binding = binding(script)
        sources = {
            "snapshot": {"head": "a" * 40, "patch_sha256": "b" * 64, "clean": False,
                         "files": [{"path": "scripts/devex_clone_identity.mjs",
                                    "sha256": script_binding["sha256"]}]},
            "worktree_fingerprint": "sha256:" + "c" * 64,
            "fingerprints": {name: {"sha256": character * 64, "files": 1}
                             for name, character in (("product", "d"), ("test_tools", "e"), ("support", "f"))},
        }
        attempt = {
            "number": number, "stage": stage, "mode": mode,
            "started_at": "2026-09-05T00:00:00+00:00",
            "finished_at": "2026-09-05T00:00:01+00:00",
            "status": "failed", "sources": sources, "result": None, "error_type": "ValueError",
        }
        original = {**attempt, "status": "running", "finished_at": None, "result": None, "error_type": None}
        owner = {
            "format_version": 1,
            "identity": {"pid": 9000 + number, "started": str(8000 + number),
                         "executable": str(self.node)},
            "directory": str(self.run),
            "manifest_sha256": binding(self.run / "manifest.json")["sha256"],
        }
        controller = self.run / f"controller-{number:04d}.json"
        self.write(controller, {"format_version": 1, "kind": "devex-stage-controller", "owner": owner,
                                "attempt": number, "attempt_sha256": plan_hash(original)})
        registration = self.run / "seed-runtime.json"
        if not registration.exists():
            self.write(registration, {"format_version": 1, "kind": "fixture-seed-registration"})
        output = self.run / stage / f"attempt-{number:04d}"
        output.mkdir(parents=True)
        request = {"node": {"path": str(self.node), "sha256": file_digest(self.node)["sha256"]},
                   "identity_plan": {"path": str(self.run / "identity-plan.json"), "sha256": "1" * 64},
                   "identity_state": str(self.run / "identity-state")}
        self.requests[str(registration)] = request
        launch = output / "session-launch.json"
        command = lineage.producer_command(self.backend, request, self.run, number, kind, output)
        self.write(launch, {
            "format_version": 1, "kind": "devex-post-producer-launch",
            "run_manifest": binding(self.run / "manifest.json"), "registration": binding(registration),
            "controller": binding(controller), "attempt": number, "producer_kind": kind, "command": command,
            "node": request["node"], "script": script_binding,
        })
        self.write(output / "session-process.json", {
            "format_version": 1, "kind": "devex-post-producer", "launch": binding(launch),
            "identity": {"pid": 7000 + number, "started": str(6000 + number),
                         "executable": str(self.node)},
        })
        self.state["attempts"].append(attempt)
        self.write(self.run / "state.json", self.state)
        return {"attempt": attempt, "controller": binding(controller)}

    def collect(self, runtimes: list[Path], attempts: list[dict] | None = None) -> dict:
        return collect_producer_lineage(self.backend, self.run, runtimes, attempts or [])

    def test_multiple_runtime_histories_pid_reuse_and_node_are_deterministic(self):
        later = self.runtime("post-copy/runtime-0001", pid=4242, started="200")
        earlier = self.runtime("post-copy/runtime", pid=4242, started="100")
        (later / f"api-{4242:032x}.log").write_bytes(b"")
        attempt = self.node_attempt()
        first = self.collect([later, earlier], [attempt])
        second = self.collect([earlier, later], [attempt])
        self.assertEqual(first, second)
        self.assertEqual([Path(item["runtime_directory"]).name for item in first["runtimes"]],
                         ["runtime", "runtime-0001"])
        identities = [(item["identity"]["pid"], item["identity"]["started"])
                      for item in first["identities"]]
        self.assertEqual(identities, [(4242, "100"), (4242, "200"), (7001, "6001")])
        node = first["node_producers"][0]
        self.assertEqual(node["attempt"], 1)
        self.assertEqual(node["producer_kind"], "identity-verify")
        self.assertEqual(node["receipt"], binding(self.run / "seed-runtime/attempt-0001/session-process.json"))
        self.assertEqual(node["launch"], binding(self.run / "seed-runtime/attempt-0001/session-launch.json"))
        self.assertEqual(node["controller"], attempt["controller"])

    def test_identity_node_accepts_optional_historical_lock_owner(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        process_path = self.run / "seed-runtime/attempt-0001/session-process.json"
        process = json.loads(process_path.read_text(encoding="utf-8"))
        process["identity_lock_owner"] = "12345678-1234-4abc-8def-1234567890ab"
        self.write(process_path, process)

        result = self.collect([runtime], [attempt])

        self.assertEqual(result["node_producers"][0]["receipt"], binding(process_path))

    def test_node_lock_owner_rejects_wrong_kind_format_and_unknown_fields(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        output = self.run / "seed-runtime/attempt-0001"
        launch_path, process_path = output / "session-launch.json", output / "session-process.json"
        original_launch = json.loads(launch_path.read_text(encoding="utf-8"))
        original_process = json.loads(process_path.read_text(encoding="utf-8"))

        for label, launch_change, process_change, message in (
            ("wrong-kind", {"producer_kind": "quota-plan"},
             {"identity_lock_owner": "12345678-1234-4abc-8def-1234567890ab"}, "非身份"),
            ("wrong-format", {}, {"identity_lock_owner": "not-a-uuid"}, "格式无效"),
            ("unknown-field", {}, {"unexpected": True}, "字段"),
        ):
            with self.subTest(label=label):
                launch = {**original_launch, **launch_change}
                self.write(launch_path, launch)
                process = {**original_process, **process_change, "launch": binding(launch_path)}
                self.write(process_path, process)
                with self.assertRaisesRegex(ValueError, message):
                    self.collect([runtime], [attempt])

    def test_duplicate_creation_identity_is_one_record_with_all_occurrences(self):
        first = self.runtime("post-copy/runtime", pid=4242, started="100")
        second = self.runtime("post-copy/runtime-0001", pid=4242, started="100")
        result = self.collect([first, second])
        self.assertEqual(len(result["identities"]), 1)
        self.assertEqual(len(result["identities"][0]["occurrences"]), 2)

    def test_same_pid_generation_with_different_executable_is_rejected(self):
        other = self.artifacts / "other-api.exe"
        other.write_bytes(b"other-api")
        first = self.runtime("post-copy/runtime", pid=4242, started="100")
        second = self.runtime("post-copy/runtime-0001", pid=4242, started="100", executable=other)
        with self.assertRaisesRegex(ValueError, "不同可执行文件"):
            self.collect([first, second])

    def test_missing_runtime_role_receipt_is_rejected(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        (runtime / "api.json").unlink()
        with self.assertRaisesRegex(ValueError, "普通文件"):
            self.collect([runtime])

    def test_missing_node_process_or_launch_is_rejected(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        process = self.run / "seed-runtime/attempt-0001/session-process.json"
        process.unlink()
        with self.assertRaisesRegex(ValueError, "缺少收据"):
            self.collect([runtime], [attempt])

    def test_runtime_outside_run_and_changed_controller_are_rejected(self):
        outside = self.root / "outside-runtime"
        outside.mkdir()
        with self.assertRaisesRegex(ValueError, "完整登记代次"):
            self.collect([outside])
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        controller = self.run / "controller-0001.json"
        value = json.loads(controller.read_text(encoding="utf-8"))
        value["attempt"] = 2
        self.write(controller, value)
        with self.assertRaisesRegex(ValueError, "固定控制器"):
            self.collect([runtime], [attempt])

    def test_node_launch_must_bind_attempt_controller_and_process(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        launch_path = self.run / "seed-runtime/attempt-0001/session-launch.json"
        process_path = self.run / "seed-runtime/attempt-0001/session-process.json"
        launch = json.loads(launch_path.read_text(encoding="utf-8"))
        launch["controller"] = binding(self.run / "manifest.json")
        self.write(launch_path, launch)
        process = json.loads(process_path.read_text(encoding="utf-8"))
        process["launch"] = binding(launch_path)
        self.write(process_path, process)
        with self.assertRaisesRegex(ValueError, "固定 attempt"):
            self.collect([runtime], [attempt])

    def test_node_full_command_and_historical_registration_are_required(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        launch_path = self.run / "seed-runtime/attempt-0001/session-launch.json"
        process_path = self.run / "seed-runtime/attempt-0001/session-process.json"
        launch = json.loads(launch_path.read_text(encoding="utf-8"))
        launch["command"].extend(["--run-dir", str(self.run)])
        self.write(launch_path, launch)
        process = json.loads(process_path.read_text(encoding="utf-8"))
        process["launch"] = binding(launch_path)
        self.write(process_path, process)
        with self.assertRaisesRegex(ValueError, "Node 命令"):
            self.collect([runtime], [attempt])
        launch["command"] = lineage.producer_command(
            self.backend, self.requests[str(self.run / "seed-runtime.json")], self.run, 1,
            "identity-verify", self.run / "seed-runtime/attempt-0001")
        launch["registration"] = binding(self.run / "manifest.json")
        self.write(launch_path, launch)
        process["launch"] = binding(launch_path)
        self.write(process_path, process)
        with self.assertRaisesRegex(ValueError, "历史登记"):
            self.collect([runtime], [attempt])

    def test_duplicate_explicit_runtime_and_attempt_are_rejected(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        attempt = self.node_attempt()
        with self.assertRaisesRegex(ValueError, "完整登记代次"):
            self.collect([runtime, runtime])
        with self.assertRaisesRegex(ValueError, "固定 state"):
            self.collect([runtime], [attempt, attempt])

    def test_omitted_registered_runtime_or_node_attempt_is_rejected(self):
        base = self.runtime("post-copy/runtime", pid=4242, started="100")
        amended = self.runtime("post-copy/runtime-0001", pid=4243, started="200")
        with self.assertRaisesRegex(ValueError, "完整登记代次"):
            self.collect([base])
        self.node_attempt()
        with self.assertRaisesRegex(ValueError, "固定 state"):
            self.collect([base, amended])

    def test_history_sequence_and_latest_role_receipt_tampering_are_rejected(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        history_path = runtime / "producer-history.json"
        history = json.loads(history_path.read_text(encoding="utf-8"))
        history["events"][1]["sequence"] = 9
        self.write(history_path, history)
        with self.assertRaisesRegex(ValueError, "序号"):
            self.collect([runtime])
        history["events"][1]["sequence"] = 1
        self.write(history_path, history)
        receipt = json.loads((runtime / "api.json").read_text(encoding="utf-8"))
        receipt["identity"]["started"] = "101"
        self.write(runtime / "api.json", receipt)
        with self.assertRaisesRegex(ValueError, "最新启动代次"):
            self.collect([runtime])

    def test_history_rejects_failed_then_started_duplicate_identity_and_second_close(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        path = runtime / "producer-history.json"
        original = json.loads(path.read_text(encoding="utf-8"))
        intent, started, stopped = original["events"]
        failed = {"sequence": 1, "time_ns": 101, "event": "start-failed", "role": "api",
                  "token": intent["token"], "error_type": "RuntimeError", "log": intent["log"]}
        started_after = {**started, "sequence": 2, "time_ns": 102}
        self.write(path, {**original, "events": [intent, failed, started_after]})
        with self.assertRaisesRegex(ValueError, "started"):
            self.collect([runtime])
        second_token = "f" * 32
        second_log = runtime / f"api-{second_token}.log"
        second_log.write_text("fixture\n", encoding="utf-8")
        second_intent = {**intent, "sequence": 3, "time_ns": 103, "token": second_token,
                         "log": second_log.name}
        second_started = {**started, "sequence": 4, "time_ns": 104, "token": second_token}
        self.write(path, {**original, "events": [intent, started, stopped, second_intent, second_started]})
        with self.assertRaisesRegex(ValueError, "多个启动 token"):
            self.collect([runtime])
        failed_after = {**failed, "sequence": 2, "time_ns": 102}
        stopped_after = {**stopped, "sequence": 3, "time_ns": 103}
        self.write(path, {**original, "events": [intent, started, failed_after, stopped_after]})
        with self.assertRaisesRegex(ValueError, "停止或回滚"):
            self.collect([runtime])

    def test_orphan_start_intent_and_forged_script_snapshot_are_rejected(self):
        runtime = self.runtime("post-copy/runtime", pid=4242, started="100")
        history_path = runtime / "producer-history.json"
        history = json.loads(history_path.read_text(encoding="utf-8"))
        self.write(history_path, {**history, "events": history["events"][:1]})
        (runtime / "api.json").unlink()
        with self.assertRaisesRegex(ValueError, "没有 started"):
            self.collect([runtime])
        self.write(history_path, history)
        self.write(runtime / "api.json", {
            "format_version": 1, "role": "api", "scope_id": "fixture-scope",
            "identity": history["events"][1]["identity"],
        })
        attempt = self.node_attempt()
        attempt["attempt"]["sources"]["snapshot"]["files"][0]["sha256"] = "0" * 64
        self.state["attempts"][0] = attempt["attempt"]
        self.write(self.run / "state.json", self.state)
        controller = self.run / "controller-0001.json"
        value = json.loads(controller.read_text(encoding="utf-8"))
        value["attempt_sha256"] = plan_hash({**attempt["attempt"], "status": "running",
                                              "finished_at": None, "result": None, "error_type": None})
        self.write(controller, value)
        attempt["controller"] = binding(controller)
        launch_path = self.run / "seed-runtime/attempt-0001/session-launch.json"
        launch = json.loads(launch_path.read_text(encoding="utf-8"))
        launch["controller"] = attempt["controller"]
        self.write(launch_path, launch)
        process_path = self.run / "seed-runtime/attempt-0001/session-process.json"
        process = json.loads(process_path.read_text(encoding="utf-8"))
        process["launch"] = binding(launch_path)
        self.write(process_path, process)
        with self.assertRaisesRegex(ValueError, "来源快照"):
            self.collect([runtime], [attempt])


if __name__ == "__main__":
    unittest.main()
