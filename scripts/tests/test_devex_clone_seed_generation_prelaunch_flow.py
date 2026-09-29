"""原失败封存、新当前像和唯一实际启动的完整控制链；无业务资源访问。"""
from contextlib import ExitStack, nullcontext
import copy
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

import devex_clone_seed_generation as generation
import devex_clone_seed_generation_control as control
import devex_clone_seed_generation_prelaunch as prelaunch
import devex_clone_seed_rebind as rebind
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash
import test_devex_clone_seed_generation_prelaunch as fixtures


class PrelaunchFlowTests(unittest.TestCase):
    def setUp(self):
        self.p = fixtures.PrelaunchProofTests()
        self.p.setUp()
        self.addCleanup(self.p.doCleanups)
        self.backend, self.directory = self.p.backend, self.p.directory
        self.directory.joinpath("results").mkdir()
        self.directory.joinpath("seed-runtime").mkdir()
        self.source_request = {"source": {"scope_id": "original-c52", "api_url": "http://127.0.0.1:28000"}}
        registration = self.file("results/0052.json", {"source_request": self.file("source.json", self.source_request)})
        self.successor = self.file("successor.json", {"same": True})
        self.storage = {"generation": "same"}
        rebound = self.file("results/0056.json", {"source_registration": registration,
                "review_successor": self.successor, "current_storage": self.storage})
        self.prefix = [self.record(52, "source-register", registration), self.record(56, "source-rebind", rebound), self.p.start]
        self.recovery = self.record(58, generation.RECOVER, status="running")
        self.recovery["sources"] = self.p.start["sources"]
        self.state = {"attempts": [*self.prefix, self.recovery]}
        self.request = {"format_version": 1, "kind": "devex-clone-seed-source-generation", "id": "current-generation",
            "source_registration": registration, "source_rebind": rebound, "review_successor": self.successor,
            "current_storage": self.storage, "execution_backend": str(self.backend), "expected_backend_sha": "a" * 40,
            "adapter_contract": None, "product_backend": None, "backend_build": self.file("build.json", {}),
            "maintenance_build": self.file("maintenance.json", {"artifacts": {
                role: {"executable": str(self.directory / f"{role}.exe")} for role in ("reset", "migrate")}}),
            "source_environment": self.file("env.json", {"environment": {}})}
        self.request_path = self.directory / "request.json"
        write_json(self.request_path, self.request)
        self.source = {"directory": self.directory, "request": self.source_request,
            "review_successor": {"source_result": registration}, "review_successor_binding": self.successor,
            "source_rebind": rebound, "storage": {"storage": self.storage}}
        self.output = self.directory / "seed-runtime/attempt-0058"
        self.runtime = Mock(output=self.output, runtime=self.output / "runtime", execution=self.backend,
            selected={**self.source_request["source"], "runtime_dir": str(self.output / "runtime")},
            build={"artifacts": {role: {"executable": str(self.directory / f"{role}.exe")} for role in ("api", "worker")}})
        self.runtime.environment.side_effect = lambda: nullcontext({})
        self.runtime.control_environment.side_effect = lambda: nullcontext({})
        self.image = {"database": "all-tables", "objects": "all-bytes", "redis": "same", "storage": self.storage}
        self.reads = []
        self.files_before = self.p.files()

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return binding(path)

    @staticmethod
    def record(number, mode, result=None, status="passed"):
        return {"number": number, "stage": "seed-runtime", "mode": mode, "result": result, "status": status}

    def contexts(self):
        stack = ExitStack()
        stack.enter_context(self.p.context())
        stack.enter_context(patch("devex_clone_run_state.load_state", return_value=self.state))
        stack.enter_context(patch.object(generation, "load_state", return_value=self.state))
        stack.enter_context(patch.object(control, "load_state", return_value=self.state))
        stack.enter_context(patch("source_fingerprints.current_execution_source", return_value=self.p.start["sources"]))
        stack.enter_context(patch("devex_clone_run._require_owned_run"))
        stack.enter_context(patch.object(control, "_active", return_value=self.prefix))
        stack.enter_context(patch.object(generation, "predecessor", return_value=self.source))
        stack.enter_context(patch("reference_fixture_successor_generation.rebuild", side_effect=lambda _b, value: value))
        stack.enter_context(patch("devex_clone_seed_generation_runtime.GenerationRuntime", return_value=self.runtime))
        stack.enter_context(patch.object(rebind, "quiet_producers"))
        stack.enter_context(patch("devex_clone_seed_generation_images.capture_image", side_effect=self.capture))
        stack.enter_context(patch("devex_clone_seed_generation_images.verify_image", side_effect=self.verify_image))
        stack.enter_context(patch.object(control, "verify_image", side_effect=self.verify_image))
        stack.enter_context(patch("full_stack_runtime.register_runtime", side_effect=self.register_runtime))
        stack.enter_context(patch("full_stack_runtime.verify_runtime", return_value={}))
        return stack

    def register_runtime(self, backend, directory):
        self.assertEqual(set(read_json(directory / "binaries.json")), {"ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"})
        write_json(directory / "runtime.json", {"backend_root": str(backend)})
        return {}

    def capture(self, *_args, **_kwargs):
        output = _args[-2]
        output.mkdir()
        image = copy.deepcopy(self.image)
        if output.name == "after" and getattr(self, "drift", False):
            image["database"] = "unknown-write"
        write_json(output / "image.json", {"image": image})
        return binding(output / "image.json")

    def verify_image(self, _backend, descriptor, *_args, **_kwargs):
        self.reads.append(descriptor)
        return read_json(Path(descriptor["path"]))

    def execute(self):
        return prelaunch.execute(self.backend, self.directory, self.request_path, self.recovery["number"], self.prefix)

    def publish(self):
        value = self.execute()
        self.recovery.update(status="passed", result=self.file(f"results/{self.recovery['number']:04d}.json", value))
        return value

    def test_recover_preserves_failure_and_captures_current_baseline_without_start_or_intent(self):
        with self.contexts():
            value = self.publish()
            archive = prelaunch.closed(self.backend, self.directory, self.state["attempts"])
        self.assertEqual(archive["image"], self.image)
        self.assertEqual(value["status"], prelaunch.STATUS)
        self.assertFalse(value["historical_image_compared"])
        self.assertFalse(value["restore_qualified"])
        self.assertEqual(value["remote_writes"], 0)
        self.assertFalse(self.p.output.exists())
        self.assertFalse(list(self.directory.rglob("intent.json")))
        for name, raw in self.files_before.items():
            self.assertEqual((self.directory / name).read_bytes(), raw)
        for method in ("prepare", "start", "stop", "retain"):
            getattr(self.runtime, method).assert_not_called()

    def test_preflight_checks_request_failure_and_resources_before_new_attempt_files(self):
        self.state["attempts"] = self.prefix
        with self.contexts():
            control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path)
            self.runtime.preflight.assert_called_once()
            self.assertFalse(self.output.exists())
            self.runtime.preflight.side_effect = ValueError("producer or resource drift")
            with self.assertRaisesRegex(ValueError, "resource drift"):
                control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path)
            self.assertFalse(self.output.exists())

    def test_recover_current_image_drift_never_closes_or_allows_start(self):
        self.drift = True
        with self.contexts(), self.assertRaisesRegex(ValueError, "当前像变化"):
            self.execute()
        self.assertTrue((self.output / "before/image.json").is_file())
        self.assertTrue((self.output / "after/image.json").is_file())
        self.assertFalse((self.directory / "results/0058.json").exists())
        self.runtime.start.assert_not_called()

    def test_sealed_prelaunch_pair_allows_only_one_actual_start_and_ignores_no_other_history(self):
        with self.contexts():
            self.publish()
            generation.preflight(self.directory, backend=self.backend)
            self.assertEqual(prelaunch.starts(self.backend, self.directory, self.state["attempts"]), [])
            rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
            actual = self.record(59, generation.START, status="running")
            self.state["attempts"].append(actual)
            self.assertEqual(prelaunch.starts(self.backend, self.directory, self.state["attempts"]), [actual])
            rebind.history(self.directory, self.state, self.request["source_registration"], current=59, backend=self.backend)
            with self.assertRaisesRegex(ValueError, "只执行一次"):
                generation.preflight(self.directory, backend=self.backend)
            self.state["attempts"].append(self.record(60, generation.START, status="running"))
            with self.assertRaises(ValueError):
                rebind.history(self.directory, self.state, self.request["source_registration"], current=60, backend=self.backend)

    def test_modified_seal_or_inserted_history_cannot_restore_start_allowance(self):
        with self.contexts():
            value = self.publish()
            original = copy.deepcopy(value)
            for field, changed in (("historical_image_compared", True), ("history_sha256", "a" * 64),
                                   ("failed_start", 56), ("before", value["after"]),
                                   ("failure_proof", {**value["failure_proof"], "tree": "a" * 40})):
                with self.subTest(field=field):
                    self.p.write(Path(self.recovery["result"]["path"]), {**original, field: changed})
                    self.recovery["result"] = binding(Path(self.recovery["result"]["path"]))
                    with self.assertRaises(ValueError):
                        prelaunch.closed(self.backend, self.directory, self.state["attempts"])
            self.p.write(Path(self.recovery["result"]["path"]), original)
            self.recovery["result"] = binding(Path(self.recovery["result"]["path"]))
            self.state["attempts"].insert(-1, self.record(57, "unknown-write"))
            with self.assertRaises(ValueError):
                prelaunch.closed(self.backend, self.directory, self.state["attempts"])

    def test_closed_prelaunch_does_not_consume_later_stop_or_recover_allowance(self):
        with self.contexts():
            self.publish()
            actual = self.record(59, generation.START, self.file("results/0059.json", {
                "source_registration": self.request["source_registration"],
                "running": self.file("running.json", {"image": self.image})}))
            self.state["attempts"].append(actual)
            control.preflight(self.directory, generation.STOP, backend=self.backend)
            output = self.directory / "g0059"
            output.mkdir()
            value, _ = control._recovery_baseline(self.backend, output, self.state["attempts"], {"status": "not_started"}, self.source, {})
            self.assertEqual(value, read_json(Path(actual["result"]["path"]))["running"])
            with patch("restore_source_runtime_producer.require_source_verifier_stopped", return_value={"status": "verified_stopped"}) as verify:
                control._verifier_stopped(self.backend, self.directory, self.state["attempts"])
            verify.assert_called_once_with(self.backend, actual["result"])
            self.state["attempts"].append(self.record(60, generation.RECOVER, status="failed"))
            with self.assertRaisesRegex(ValueError, "不能重放"):
                control.preflight(self.directory, generation.RECOVER, backend=self.backend)

    def test_current_baseline_drift_is_rejected_before_unique_actual_process_start(self):
        with self.contexts():
            self.publish()
            prefix = list(self.state["attempts"])
            actual = self.record(59, generation.START, status="running")
            actual["sources"] = self.p.start["sources"]
            self.state["attempts"].append(actual)
            self.image["database"] = "write-after-recover"
            with patch.object(generation, "inputs", return_value=(self.request, self.source, prefix)), \
                    self.assertRaisesRegex(ValueError, "基线之后存在未知写入"):
                generation.execute_generation(self.backend, self.directory, self.request_path, 59)
            self.runtime.start.assert_not_called()
            self.assertTrue((self.directory / "g0059/before/image.json").exists())
            self.assertFalse(self.p.output.exists())

    def test_status_uses_not_started_only_after_strict_seal_then_selects_actual_attempt(self):
        with self.contexts():
            self.publish()
            self.file("state.json", {"fixture": "bound"})
            observed = control.status(self.backend, self.directory)
            self.assertEqual(observed["attempt"], 57)
            self.assertEqual({role["state"] for role in observed["roles"].values()}, {"not_started"})
            self.state["attempts"].append(self.record(59, generation.START, status="failed"))
            observed = control.status(self.backend, self.directory)
            self.assertEqual(observed["attempt"], 59)
            self.assertEqual({role["state"] for role in observed["roles"].values()}, {"unknown"})

    def test_duplicate_recovery_or_new_storage_operation_cannot_hide_in_sealed_prefix(self):
        with self.contexts():
            self.publish()
            self.state["attempts"].append({**self.recovery, "number": 59})
            with self.assertRaisesRegex(ValueError, "重复恢复"):
                prelaunch.closed(self.backend, self.directory, self.state["attempts"])
            self.state["attempts"].pop()
            self.state["attempts"].append({"number": 59, "stage": "storage-target", "mode": "restart", "status": "passed"})
            with self.assertRaisesRegex(ValueError, "未知写入"):
                rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)

    def failed_collection(self):
        failed = {**self.recovery, "status": "failed", "error_type": "CalledProcessError"}
        self.prefix.append(failed)
        self.recovery = {**self.recovery, "number": 59}
        self.state["attempts"] = [*self.prefix, self.recovery]
        self.output = self.directory / "seed-runtime/attempt-0059"
        self.runtime.output = self.output
        self.runtime.runtime = self.output / "runtime"
        self.runtime.selected = {**self.source_request["source"], "runtime_dir": str(self.runtime.runtime)}
        return failed

    def test_audited_failed_collection_can_prepare_again_without_replaying_start(self):
        failed = self.failed_collection()
        self.state["attempts"] = self.prefix
        with self.contexts(), patch.object(prelaunch, "collection_failure", return_value={"original": 58}):
            control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path)
            rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
            with self.assertRaisesRegex(ValueError, "只执行一次"):
                generation.preflight(self.directory, backend=self.backend)
        self.assertIn(failed, self.state["attempts"])
        self.assertFalse(self.output.exists())
        self.runtime.start.assert_not_called()

    def test_failed_collection_is_preserved_and_bound_in_successful_second_collection(self):
        failed = self.failed_collection()
        old = self.directory / "seed-runtime/attempt-0058"
        old.mkdir()
        write_json(old / "diagnostic.json", {"status": "original failure"})
        before = self.p.files()
        with self.contexts(), patch.object(prelaunch, "collection_failure", return_value={"original": 58}) as proof:
            rebind.history(self.directory, self.state, self.request["source_registration"], current=59, backend=self.backend)
            value = self.publish()
            self.assertEqual(value["collection_failure"], {"original": 58})
            self.assertEqual(value["history_sha256"], plan_hash(self.prefix))
            archive = prelaunch.closed(self.backend, self.directory, self.state["attempts"])
            self.assertEqual(archive["records"], (self.p.start, failed, self.recovery))
            generation.preflight(self.directory, backend=self.backend)
            rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
            actual = self.record(60, generation.START, status="running")
            self.state["attempts"].append(actual)
            rebind.history(self.directory, self.state, self.request["source_registration"], current=60, backend=self.backend)
            with self.assertRaisesRegex(ValueError, "只执行一次"):
                generation.preflight(self.directory, backend=self.backend)
            self.assertEqual(prelaunch.starts(self.backend, self.directory, self.state["attempts"]), [actual])
            proof.return_value = {"original": "changed after seal"}
            with self.assertRaises(ValueError):
                prelaunch.closed(self.backend, self.directory, self.state["attempts"])
        for path, raw in before.items():
            self.assertEqual((self.directory / path).read_bytes(), raw)
        self.runtime.start.assert_not_called()
        self.assertFalse(value["historical_image_compared"])

    def test_unverified_failed_collection_or_extra_history_never_allows_recovery(self):
        self.failed_collection()
        self.state["attempts"] = self.prefix
        with self.contexts(), patch.object(prelaunch, "collection_failure", side_effect=ValueError("unverified collection")):
            for action in (
                lambda: control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path),
                lambda: rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend),
            ):
                with self.assertRaisesRegex(ValueError, "unverified collection"):
                    action()
        with self.contexts(), patch.object(prelaunch, "collection_failure", return_value={"original": 58}):
            for mode in (generation.RECOVER, generation.STOP, generation.START, "unknown-write"):
                with self.subTest(mode=mode):
                    extra = self.record(59, mode, status="failed")
                    self.state["attempts"] = [*self.prefix, extra]
                    with self.assertRaises(ValueError):
                        control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path)
                    with self.assertRaises(ValueError):
                        rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
        self.assertFalse(self.output.exists())

    def test_second_audited_parser_failure_can_be_sealed_without_losing_either_collection(self):
        first = self.failed_collection()
        second = {**self.recovery, "status": "failed", "error_type": "CalledProcessError"}
        self.prefix.append(second)
        self.recovery = {**self.recovery, "number": 60}
        self.state["attempts"] = self.prefix
        self.output = self.directory / "seed-runtime/attempt-0060"
        self.runtime.output, self.runtime.runtime = self.output, self.output / "runtime"
        self.runtime.selected = {**self.source_request["source"], "runtime_dir": str(self.runtime.runtime)}
        with self.contexts(), patch.object(prelaunch, "collection_failure", return_value={"original": [58, 59]}):
            self.state["attempts"] = [*self.prefix, {**second, "number": 60}]
            with self.assertRaises(ValueError):
                control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path)
            with self.assertRaises(ValueError):
                rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
            self.state["attempts"] = self.prefix
            control.preflight(self.directory, generation.RECOVER, backend=self.backend, request_path=self.request_path)
            rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
            with self.assertRaises(ValueError):
                generation.preflight(self.directory, backend=self.backend)
            self.state["attempts"] = [*self.prefix, self.recovery]
            rebind.history(self.directory, self.state, self.request["source_registration"], current=60, backend=self.backend)
            value = self.publish()
            archive = prelaunch.closed(self.backend, self.directory, self.state["attempts"])
            self.assertEqual(archive["records"], (self.p.start, first, second, self.recovery))
            self.assertEqual(value["collection_failure"], {"original": [58, 59]})
            generation.preflight(self.directory, backend=self.backend)
            rebind.history(self.directory, self.state, self.request["source_registration"], backend=self.backend)
            self.state["attempts"].append(self.record(61, generation.START, status="running"))
            rebind.history(self.directory, self.state, self.request["source_registration"], current=61, backend=self.backend)
            with self.assertRaises(ValueError):
                generation.preflight(self.directory, backend=self.backend)
        self.runtime.start.assert_not_called()


if __name__ == "__main__":
    unittest.main()
