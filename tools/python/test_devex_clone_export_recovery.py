"""源导出发布中断恢复的离线状态机回归；不连接真实服务。"""
from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_export_recovery as recovery
import devex_clone_run as run
import devex_clone_run_state as state
from devex_clone_capture import read_json, write_json
from restore_reference_plan import plan_hash
from process_environment import Environments


class ExportRecoveryTests(unittest.TestCase):
    def setUp(self):
        root = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        root.mkdir(parents=True, exist_ok=True)
        self.backend = (root / f"export-recovery-{uuid.uuid4().hex}").resolve()
        self.backend.mkdir()
        self.addCleanup(shutil.rmtree, self.backend, True)
        local = self.backend / ".local-tests"
        local.mkdir()
        self.directory = local / "run"
        self.directory.mkdir()
        write_json(self.directory / "manifest.json", {"kind": "fixture-run"})
        state.initialize_state(self.directory)
        request = local / "source-request.json"
        write_json(request, {"kind": "fixture-source-request"})
        self.request = state.binding(request)
        self.value = {"source_request": self.request, "source_export": None,
                      "copy_stage": "source_to_seed"}
        self.product = {"sha256": "a" * 64, "files": 7}
        self.sources = {"fingerprints": {"product": self.product}, "tools": "fixture"}
        self.storage = {"kind": "fixture-storage", "sha256": "b" * 64}
        self.generation = {"source": "fixture-generation"}
        self.environment = Environments(dict(os.environ), dict(os.environ))
        self.export_value = {"request": self.request,
                             "logical_inventory_sha256": "c" * 64,
                             "databases": [{"key": "control"}],
                             "objects": [{"bucket": "uploads", "entries": []}]}

    def start(self, mode, *, sources=None):
        sources = copy.deepcopy(self.sources if sources is None else sources)
        number = state.begin(self.directory, "export", mode, sources)
        attempt = state.load_state(self.directory)["attempts"][-1]
        owner = {"directory": str(self.directory),
                 "manifest_sha256": state.binding(self.directory / "manifest.json")["sha256"]}
        write_json(self.directory / f"controller-{number:04d}.json", {
            "format_version": 1, "kind": "devex-stage-controller", "owner": owner,
            "attempt": number, "attempt_sha256": plan_hash(attempt),
        })
        return number

    def verified(self, exported=None, *, generation=None):
        exported = exported or state.binding(self.directory / "e0001/export.json")
        return {"export": read_json(Path(exported["path"])), "binding": exported,
                "request": read_json(Path(self.request["path"])),
                "generation": copy.deepcopy(generation or self.generation), "proof_files": {}}

    def prepare_origin(self, *, seal=True):
        number = self.start("run")
        output = recovery.prepare_attempt(self.backend, self.directory, self.value, number,
                                          self.sources, self.storage)
        output.mkdir()
        write_json(output / "export.json", self.export_value)
        exported = state.binding(output / "export.json")
        if seal:
            intent = read_json(self.directory / f"export-{number:04d}.intent.json")
            recovery.record_verified(self.directory, number, intent, exported,
                                     self.verified(exported))
        result = {"status": "export_verified", "export": exported}
        state.finish(self.directory, number, result=result, error=RuntimeError("outer failure"))
        return exported

    def recover(self, mode, *, sources=None, storage=None, generation=None):
        sources = copy.deepcopy(self.sources if sources is None else sources)
        storage = copy.deepcopy(self.storage if storage is None else storage)
        generation = copy.deepcopy(self.generation if generation is None else generation)
        number = self.start(mode, sources=sources)
        operation = recovery.reconcile if mode == "reconcile" else recovery.resume
        with patch("devex_clone_export_verify.verify_source_export",
                   side_effect=lambda _, exported: self.verified(exported)), \
                patch("devex_clone_storage.current_storage_binding", return_value=storage), \
                patch.object(recovery, "verify_generation", return_value=generation):
            return number, operation(self.backend, self.directory, self.value,
                                     self.environment, number, sources)

    def fail_current(self, number, result=None):
        state.finish(self.directory, number, result=result, error=RuntimeError("outer failure"))

    def test_reconcile_and_resume_forward_control_environment_without_exposing_it_to_resources(self):
        self.prepare_origin()
        controller = {**dict(os.environ), "CARGO_BUILD_JOBS": "4"}
        service = {"APP_ENV": "fixture-service"}
        environment = Environments(service, controller)
        def control_environment():
            if dict(os.environ) not in (controller, service):
                raise ValueError("fixture ambient changed")
            return environment.use("target")
        def generation(*_args, control_environment):
            self.assertEqual(dict(os.environ), service)
            with control_environment():
                self.assertEqual(dict(os.environ), controller)
            self.assertEqual(dict(os.environ), service)
            return self.generation
        def storage(*_args):
            self.assertEqual(dict(os.environ), service)
            return self.storage
        def verify(_backend, exported):
            self.assertEqual(dict(os.environ), service)
            return self.verified(exported)
        with patch.dict(os.environ, controller, clear=True), \
                patch("devex_clone_export_verify.verify_source_export", side_effect=verify), \
                patch("devex_clone_storage.current_storage_binding", side_effect=storage), \
                patch.object(recovery, "verify_generation", side_effect=generation):
            for mode, operation in (("reconcile", recovery.reconcile), ("resume", recovery.resume)):
                number = self.start(mode)
                with patch.dict(os.environ, {"CARGO_BUILD_JOBS": "unregistered"}), \
                        self.assertRaisesRegex(ValueError, "fixture ambient changed"):
                    operation(self.backend, self.directory, self.value, environment, number,
                              self.sources, control_environment=control_environment)
                result = operation(self.backend, self.directory, self.value, environment, number,
                                   self.sources, control_environment=control_environment)
                self.assertEqual(dict(os.environ), controller)
                state.finish(self.directory, number, result=result)

    def test_complete_export_followed_by_outer_failure_forbids_reexport(self):
        number = self.start("run")

        def export_source(_backend, _request, output, *, control_environment=None):
            output.mkdir()
            write_json(output / "export.json", self.export_value)

        with patch("devex_clone_storage.current_storage_binding", return_value=self.storage), \
                patch("devex_clone_source.export_source", side_effect=export_source) as exported, \
                patch("devex_clone_export_verify.verify_source_export",
                      side_effect=lambda _, item: self.verified(item)):
            result = run.run_export(self.backend, self.directory, self.value, self.environment,
                                    number, "run", self.sources)
        self.fail_current(number, result)
        retry = self.start("run")
        with patch("devex_clone_source.export_source") as repeated, \
                self.assertRaisesRegex(ValueError, "禁止重新完整导出"):
            run.run_export(self.backend, self.directory, self.value, self.environment,
                           retry, "run", self.sources)
        exported.assert_called_once()
        repeated.assert_not_called()
        self.fail_current(retry)

    def test_reconcile_requires_exactly_one_failed_origin_candidate(self):
        self.prepare_origin()
        second = self.start("run")
        output = recovery.prepare_attempt(self.backend, self.directory, self.value, second,
                                          self.sources, self.storage)
        output.mkdir()
        write_json(output / "export.json", self.export_value | {"extra": True})
        self.fail_current(second)
        current = self.start("reconcile")
        with self.assertRaisesRegex(ValueError, "恰有一个.*failed attempt"):
            recovery.reconcile(self.backend, self.directory, self.value,
                               self.environment, current, self.sources)
        self.fail_current(current)

    def test_reconcile_rejects_a_complete_candidate_from_passed_run(self):
        number = self.start("run")
        output = recovery.prepare_attempt(self.backend, self.directory, self.value, number,
                                          self.sources, self.storage)
        output.mkdir()
        write_json(output / "export.json", self.export_value)
        state.finish(self.directory, number,
                     result={"status": "export_verified", "export": state.binding(
                         output / "export.json")})
        current = self.start("reconcile")
        with self.assertRaisesRegex(ValueError, "failed attempt"):
            recovery.reconcile(self.backend, self.directory, self.value,
                               self.environment, current, self.sources)
        self.fail_current(current)

    def test_binding_changes_fail_closed(self):
        cases = ("controller", "intent", "storage", "product", "generation", "export")
        for change in cases:
            with self.subTest(change=change):
                self.setUp()
                self.prepare_origin()
                sources, storage, generation = copy.deepcopy(self.sources), self.storage, self.generation
                if change == "controller":
                    path = self.directory / "controller-0001.json"
                    value = read_json(path)
                    value["owner"]["manifest_sha256"] = "d" * 64
                    path.write_text(json.dumps(value), encoding="utf-8")
                elif change == "intent":
                    path = self.directory / "export-0001.intent.json"
                    value = read_json(path)
                    value["remote_operations"] = "changed"
                    path.write_text(json.dumps(value), encoding="utf-8")
                elif change == "storage":
                    storage = self.storage | {"sha256": "d" * 64}
                elif change == "product":
                    sources["fingerprints"]["product"]["sha256"] = "d" * 64
                elif change == "generation":
                    generation = {"source": "changed"}
                else:
                    path = self.directory / "e0001/export.json"
                    path.write_text(json.dumps(self.export_value | {"changed": True}), encoding="utf-8")
                number = self.start("reconcile", sources=sources)
                with patch("devex_clone_export_verify.verify_source_export",
                           side_effect=lambda _, item: self.verified(item)), \
                        patch("devex_clone_storage.current_storage_binding", return_value=storage), \
                        patch.object(recovery, "verify_generation", return_value=generation), \
                        self.assertRaises(ValueError):
                    recovery.reconcile(self.backend, self.directory, self.value,
                                       self.environment, number, sources)
                self.fail_current(number)

    def test_failed_reconcile_can_repeat_then_resume_only_from_passed_receipt(self):
        self.prepare_origin()
        first, receipt = self.recover("reconcile")
        self.fail_current(first, receipt)
        blocked = self.start("resume")
        with self.assertRaisesRegex(ValueError, "必须先完成.*reconcile"):
            recovery.resume(self.backend, self.directory, self.value,
                            self.environment, blocked, self.sources)
        self.fail_current(blocked)
        second, repeated = self.recover("reconcile")
        self.assertEqual(repeated, receipt)
        state.finish(self.directory, second, result=repeated)
        resumed, result = self.recover("resume")
        self.assertEqual(result["status"], "export_verified")
        self.assertEqual(result["export"], receipt["export"])
        self.assertEqual(result["reconciliation"], state.binding(
            self.directory / f"results/{second:04d}.json"))
        state.finish(self.directory, resumed, result=result)

    def test_resume_rejects_changed_passed_reconcile_binding(self):
        self.prepare_origin()
        number, receipt = self.recover("reconcile")
        state.finish(self.directory, number, result=receipt)
        value = read_json(self.directory / "state.json")
        value["attempts"][number - 1]["sources"]["tools"] = "changed"
        (self.directory / "state.json").write_text(json.dumps(value), encoding="utf-8")
        resumed = self.start("resume")
        with patch("devex_clone_export_verify.verify_source_export",
                   side_effect=lambda _, item: self.verified(item)), \
                patch("devex_clone_storage.current_storage_binding", return_value=self.storage), \
                patch.object(recovery, "verify_generation", return_value=self.generation), \
                self.assertRaisesRegex(ValueError, "已发布 reconcile 收据"):
            recovery.resume(self.backend, self.directory, self.value,
                            self.environment, resumed, self.sources)
        self.fail_current(resumed)

    def test_different_adoption_results_are_rejected(self):
        self.prepare_origin()
        first, receipt = self.recover("reconcile")
        self.fail_current(first, receipt)
        sources = copy.deepcopy(self.sources)
        sources["tools"] = "changed"
        with self.assertRaisesRegex(ValueError, "本次 export 采用结果.*既有结果不同"):
            self.recover("reconcile", sources=sources)
        current = state.load_state(self.directory)["attempts"][-1]["number"]
        self.fail_current(current)


if __name__ == "__main__":
    unittest.main()
