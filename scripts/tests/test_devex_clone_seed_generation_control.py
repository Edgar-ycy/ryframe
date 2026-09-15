"""同一已启动源的停止和恢复失败关闭；外部验证器与采集器使用明确替身。"""
from contextlib import ExitStack, contextmanager
import copy
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone_seed_generation as generation
import devex_clone_seed_generation_control as control
import test_devex_clone_seed_generation as fixtures
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding


class GenerationControlTests(unittest.TestCase):
    def setUp(self):
        self.f = fixtures.GenerationTests()
        self.f.setUp()
        self.addCleanup(self.f.doCleanups)
        self.start = self.f.execute()
        (self.f.directory / "results").mkdir()
        write_json(self.f.directory / "results/0054.json", self.start)
        self.descriptor = binding(self.f.directory / "results/0054.json")
        self.f.runtime.reset_mock()
        self.f.image["database"]["preserved"] = "explicit-session-audit-only"
        self.verified = {"receipt": {"source_generation": self.descriptor,
                                    "dataset_lineage": self.start["dataset_lineage"]},
                         "after": {"image": copy.deepcopy(self.f.image)}}
        self.source_runtime = self.f.file("source-runtime.json", self.verified["receipt"])
        self.facts = {"directory": self.f.directory, "output": self.f.output, "request": self.f.request,
                      "source": self.f.source, "runtime": {}, "receipt": self.start, "execution": self.f.backend}
        self.prefix = self.f.prefix + [self.f.record(54, generation.START, self.descriptor)]

    @contextmanager
    def execution(self):
        with ExitStack() as stack:
            verify = Mock(return_value=self.verified)
            stack.enter_context(patch.dict("sys.modules", {"restore_source_runtime": SimpleNamespace(verify_source_runtime=verify)}))
            stack.enter_context(patch.object(control, "_active", return_value=self.prefix))
            stack.enter_context(patch.object(control, "verify_running_source", return_value=self.facts))
            stack.enter_context(patch.object(control, "existing_runtime", return_value=self.f.runtime))
            stack.enter_context(patch.object(control, "capture_image", side_effect=self.f.capture))
            stack.enter_context(patch("devex_clone_source_proof.verify_generation", side_effect=self.f.generation_evidence))
            yield verify

    def stop(self):
        return control.execute_stop(self.f.backend, self.f.directory, Path(self.source_runtime["path"]), 55)

    def test_stop_only_accepts_same_generation_verified_image_and_never_starts_again(self):
        with self.execution() as verify:
            result = self.stop()
        self.assertEqual(result["status"], "seed_source_generation_published")
        self.assertEqual(result["source_runtime"], self.source_runtime)
        self.assertEqual(result["start"], self.descriptor)
        self.assertEqual(result["remote_writes"], 0)
        self.assertFalse(result["restore_qualified"])
        self.assertEqual([call.kwargs["live"] for call in verify.call_args_list], [True, True, False])
        self.f.runtime.stop.assert_called_once()
        self.f.runtime.start.assert_not_called()
        self.assertNotEqual(read_json(Path(result["before"]["path"]))["image"], read_json(Path(result["after"]["path"]))["image"])
        self.assertEqual(read_json(Path(result["stop_before"]["path"]))["image"], read_json(Path(result["after"]["path"]))["image"])

    def test_resolve_is_readonly_and_binds_quiesced_generation_to_same_start_and_source_verify(self):
        with self.execution():
            result = self.stop()
            stop_path = self.f.directory / "results/0055.json"
            write_json(stop_path, result)
            descriptor = binding(stop_path)
            state = {"attempts": self.prefix + [self.f.record(55, generation.STOP, descriptor)]}
            before = {str(path): path.read_bytes() for path in self.f.directory.rglob("*") if path.is_file()}
            with patch.object(generation, "verify_running_source", return_value=self.facts), \
                    patch.object(generation, "validate_request"), \
                    patch("devex_clone_seed_generation_images.verify_image",
                          side_effect=lambda _b, document, *_a, **_kw: read_json(Path(document["path"]))), \
                    patch("devex_clone_seed_generation_runtime.verify_runtime_evidence", return_value=self.f.source["generation"]) as runtime:
                resolved = generation.resolve_generation(self.f.backend, self.f.registration, self.f.source, state, live=False)
                self.assertEqual(resolved["source_generation"], descriptor)
                self.assertEqual(resolved["source_request"], result["source_request"])
                self.assertFalse(runtime.call_args.kwargs["live"])
                generation.resolve_generation(self.f.backend, self.f.registration, self.f.source, state, live=True)
                self.assertTrue(runtime.call_args.kwargs["live"])
                self.assertEqual(before, {str(path): path.read_bytes() for path in self.f.directory.rglob("*") if path.is_file()})
                Path(result["after"]["path"]).write_text("{}", encoding="utf-8")
                with self.assertRaises(ValueError):
                    generation.resolve_generation(self.f.backend, self.f.registration, self.f.source, state, live=False)

    def test_external_write_after_source_verify_stops_but_never_publishes_effective_source(self):
        self.f.image["objects"]["bytes"] = "external-write"
        with self.execution(), self.assertRaisesRegex(ValueError, "未披露写入"):
            self.stop()
        self.f.runtime.stop.assert_called_once()
        self.f.runtime.finish.assert_not_called()
        self.assertTrue((self.f.output / "after/image.json").is_file())
        self.assertFalse((self.f.output / "source-request.json").exists())

    def test_shutdown_drift_preserves_full_before_after_and_rejects_publication(self):
        self.f.drift = "redis", "state"
        with self.execution(), self.assertRaisesRegex(ValueError, "停止前后"):
            self.stop()
        self.assertTrue((self.f.output / "stop-before/image.json").is_file())
        self.assertTrue((self.f.output / "after/image.json").is_file())
        self.f.runtime.finish.assert_not_called()

    def test_stop_and_recover_cannot_replay_existing_control_attempt(self):
        for mode in (generation.STOP, generation.RECOVER):
            for status in ("failed", "running", "passed"):
                prior = self.prefix + [self.f.record(55, mode, self.source_runtime, status=status)]
                with self.subTest(mode=mode, status=status), patch.object(control, "load_state", return_value={"attempts": prior}):
                    with self.assertRaisesRegex(ValueError, "重放"):
                        control.preflight(self.f.directory, mode)

    def test_missing_tree_publication_window_is_unknown_and_recovery_performs_no_action(self):
        observed = {"roles": {"api": {"state": "unknown"}, "worker": {"state": "running"}}}
        with patch.object(control, "_active", return_value=self.prefix), \
                patch.object(control, "status", return_value=observed), \
                patch.object(control, "capture_image") as capture, patch.object(control, "existing_runtime") as runtime, \
                self.assertRaisesRegex(ValueError, "完整树发布前中断"):
            control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
        capture.assert_not_called()
        runtime.assert_not_called()

    @contextmanager
    def recovery(self):
        running = {"attempt": 54, "intent": self.start["intent"],
                   "roles": {role: {"state": "running"} for role in ("api", "worker")}}
        stopped = {**running, "roles": {role: {"state": "stopped"} for role in ("api", "worker")}}
        with patch.dict("sys.modules", {"restore_source_runtime_producer": SimpleNamespace(
                require_source_verifier_stopped=Mock(return_value={"status": "not_started", "process": None}))}), \
                patch.object(control, "_active", return_value=self.prefix), \
                patch.object(control, "status", side_effect=[running, stopped]), \
                patch.object(control, "load_state", return_value={"attempts": self.prefix}), \
                patch.object(control, "predecessor", return_value=self.f.source), \
                patch.object(control, "existing_runtime", return_value=self.f.runtime), \
                patch.object(control, "verify_image", side_effect=lambda _b, descriptor, *_a, **_kw: read_json(Path(descriptor["path"]))), \
                patch.object(control, "capture_image", side_effect=self.f.capture) as capture:
            yield capture

    def test_live_or_unknown_source_verifier_blocks_recovery_before_any_action(self):
        for message in ("来源验收 Node 仍在运行", "来源验收缺少已登记进程身份"):
            with self.subTest(message=message), self.recovery() as capture, \
                    patch("restore_source_runtime_producer.require_source_verifier_stopped", side_effect=ValueError(message)), \
                    self.assertRaisesRegex(ValueError, message):
                control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
            capture.assert_not_called()
            self.f.runtime.stop.assert_not_called()

    def test_failed_start_rejects_any_verification_directory(self):
        failed = copy.deepcopy(self.prefix)
        failed[-1].update(status="failed", result=None)
        (self.f.output / "verification").mkdir()
        with self.recovery(), self.assertRaisesRegex(ValueError, "未知验收生产者"):
            control._verifier_stopped(self.f.backend, self.f.directory, failed)

    def test_known_tree_recovery_only_stops_and_never_publishes_or_replays(self):
        self.f.image = read_json(Path(self.start["running"]["path"]))["image"]
        with self.recovery():
            result = control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
        self.assertEqual(result["status"], "seed_source_generation_abandoned")
        self.assertFalse(result["source_generation_published"])
        self.assertFalse(result["replay_allowed"])
        self.f.runtime.stop.assert_called_once()
        self.f.runtime.start.assert_not_called()
        self.f.runtime.finish.assert_not_called()

    def test_recovery_does_not_label_equal_current_images_as_zero_historical_writes(self):
        self.f.image["objects"]["bytes"] = "external-write-before-recover"
        with self.recovery(), self.assertRaisesRegex(ValueError, "未知写入"):
            control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
        self.f.runtime.stop.assert_called_once()
        before = read_json(self.f.output / "recover-before/image.json")["image"]
        after = read_json(self.f.output / "recover-after/image.json")["image"]
        self.assertEqual(before, after)
        self.assertNotEqual(before, read_json(Path(self.start["running"]["path"]))["image"])
        self.f.runtime.finish.assert_not_called()

    def test_failed_start_without_published_running_image_is_reclaimed_but_not_qualified(self):
        self.prefix[-1].update(status="failed", result=None)
        with self.recovery(), self.assertRaisesRegex(ValueError, "缺少不可变运行前像"):
            control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
        self.f.runtime.stop.assert_called_once()

    def test_verified_session_after_image_is_the_only_allowed_recovery_baseline(self):
        verification = self.f.output / "verification"
        verification.mkdir()
        write_json(verification / "source-runtime.json", self.verified["receipt"])
        self.verified["receipt"]["after"] = self.f.file("verified-after.json", self.verified["after"])
        with self.recovery(), patch("restore_source_runtime_producer.require_source_verifier_stopped",
                return_value={"status": "verified_stopped", "process": {"pid": 123}}), \
                patch.dict("sys.modules", {"restore_source_runtime": SimpleNamespace(
                    verify_source_runtime=Mock(return_value=self.verified))}):
            result = control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
        self.assertEqual(result["baseline"], self.verified["receipt"]["after"])
        self.assertEqual(result["remote_writes"], 0)

    def test_known_tree_recovery_still_stops_when_before_image_capture_fails(self):
        def capture(*args, **kwargs):
            if args[-2].name == "recover-before":
                raise TimeoutError("full image unavailable")
            return self.f.capture(*args)

        with self.recovery() as observed:
            observed.side_effect = capture
            with self.assertRaisesRegex(TimeoutError, "full image unavailable"):
                control.execute_recover(self.f.backend, self.f.directory, self.f.request_path, 55)
        self.f.runtime.stop.assert_called_once()
        self.assertTrue((self.f.output / "recover-after/image.json").is_file())
        self.f.runtime.finish.assert_not_called()


if __name__ == "__main__":
    unittest.main()
