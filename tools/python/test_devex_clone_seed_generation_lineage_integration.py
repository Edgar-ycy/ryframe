"""C68、C69 与 C70 的生成续作集成测试。"""
from contextlib import ExitStack
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_generation as generation
import devex_clone_seed_generation_control as control
import devex_clone_seed_generation_lineage_recovery as recovery
import devex_clone_seed_generation_prelaunch as prelaunch
import devex_clone_seed_segment as segment
from devex_clone_capture import write_json
from devex_clone_run_state import binding


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/tmp", prefix="lineage-integration-"
        )
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.source = {"fingerprints": {"product": {"sha256": "a" * 64}}}

    def row(self, number, stage, mode, status="passed", result=None, error=None):
        return {
            "number": number,
            "stage": stage,
            "mode": mode,
            "status": status,
            "result": result,
            "error_type": error,
            "sources": self.source,
        }

    def test_generation_preflight_requires_c69_and_closes_after_c70(self):
        request_path = self.directory / "request.json"
        write_json(request_path, {"request": "current"})
        request_descriptor = binding(request_path)
        rebound = self.row(52, "seed-runtime", "source-rebind")
        closed = self.row(60, "seed-runtime", "source-generation-recover")
        c68 = self.row(
            68,
            "seed-runtime",
            "source-generation-start",
            "failed",
            error="ValueError",
        )
        c69 = self.row(69, "seed-runtime", "source-generation-recover")
        c70 = self.row(70, "seed-runtime", "source-generation-start", "failed", error="ValueError")
        archive = {
            "records": (closed,),
            "recovery": closed,
            "receipt": {"source_registration": {"path": "C52"}},
        }

        def check(attempts, segment_value, failure=False):
            with patch.object(generation, "load_state", return_value={"attempts": attempts}), \
                    patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                    patch("devex_clone_seed_segment.segmented_resume", return_value=segment_value):
                if failure:
                    with self.assertRaises(ValueError):
                        generation.preflight(
                            self.directory,
                            backend=self.backend,
                            request_path=request_path,
                        )
                else:
                    generation.preflight(
                        self.directory,
                        backend=self.backend,
                        request_path=request_path,
                    )

        ready = {"phase": "ready", "records": (), "storage": {"attempt": 65},
                 "cache": {"attempt": 66}, "replay": None}
        check([rebound, closed, c68], ready, failure=True)
        authorized = {
            **ready,
            "records": (c68, c69),
            "replay": {"record": c69, "request": request_descriptor},
        }
        check([rebound, closed, c68, c69], authorized)
        check([rebound, closed, c68, c69, c70], authorized, failure=True)

    def test_starts_only_hides_c68_after_valid_authority_and_keeps_c70(self):
        c60 = self.row(60, "seed-runtime", "source-generation-recover")
        c66 = self.row(66, "cache-target", "restart")
        c67 = self.row(
            67, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c68 = self.row(
            68, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c69 = self.row(69, "seed-runtime", "source-generation-recover")
        c70 = self.row(70, "seed-runtime", "source-generation-start")
        attempts = [c60, c66, c67, c68, c69, c70]
        archive = {"start": {"number": 1}}
        with patch.object(prelaunch, "closed", return_value=archive), \
                patch.object(prelaunch, "receipt_reregistration_failure") as old_failure, \
                patch.object(recovery, "replay_authority", return_value=None):
            self.assertEqual(prelaunch.starts(self.backend, self.directory, attempts), [c68, c70])
        old_failure.assert_called_once_with(self.backend, self.directory, c67, c66)
        replay = {"failed": c68, "record": c69, "records": (c68, c69)}
        with patch.object(prelaunch, "closed", return_value=archive), \
                patch.object(prelaunch, "receipt_reregistration_failure"), \
                patch.object(recovery, "replay_authority", return_value=replay):
            self.assertEqual(prelaunch.starts(self.backend, self.directory, attempts), [c70])

    def test_segment_consumes_c68_c69_and_exposes_replay_to_c70(self):
        recovery60 = self.row(60, "seed-runtime", "source-generation-recover")
        cache_stop = self.row(61, "cache-target", "stop")
        storage_failed = self.row(62, "storage-target", "stop", "failed", error="ValueError")
        storage_stop = self.row(63, "storage-target", "stop")
        storage_restart_failed = self.row(
            64, "storage-target", "restart", "failed", error="CalledProcessError"
        )
        storage_restart = self.row(65, "storage-target", "restart", result={"storage": 65})
        cache_restart = self.row(66, "cache-target", "restart", result={"cache": 66})
        c67 = self.row(
            67, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c68 = self.row(
            68, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c69 = self.row(69, "seed-runtime", "source-generation-recover", result={"replay": 69})
        attempts = [
            recovery60,
            cache_stop,
            storage_failed,
            storage_stop,
            storage_restart_failed,
            storage_restart,
            cache_restart,
            c67,
            c68,
            c69,
        ]
        (self.directory / "g0068").mkdir()
        evidence_a = self.file("failure.json", {"fixed": "failure"})
        evidence_b = self.file("controller.json", {"fixed": "controller"})
        storage = {"attempt": 65, "restart_result": storage_restart["result"]}
        cache = {
            "attempt": 66,
            "restart_result": cache_restart["result"],
            "request": {"cache": True},
        }
        archive = {
            "recovery": recovery60,
            "receipt": {
                "current_storage": {"attempt": 53},
                "review_successor": {"path": "successor"},
            },
        }
        replay = {
            "failed": c68,
            "record": c69,
            "records": (c68, c69),
            "receipt": {"request": {"path": "request"}},
            "image": {"frozen": True},
        }
        tail = Mock(return_value=None)
        with ExitStack() as stack:
            stack.enter_context(patch.object(segment, "_same_product_source"))
            stack.enter_context(patch.object(segment, "_require_clean_source"))
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_cache_stop",
                    return_value={"request": cache["request"], "result": {}},
                )
            )
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_storage_stop",
                    return_value={
                        "request": {"storage": True},
                        "result": {"processes": [{"attempt": 1}]},
                    },
                )
            )
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_failed_storage_stop",
                    return_value=(evidence_a, evidence_b),
                )
            )
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_failed_storage_restart",
                    return_value=(evidence_a, evidence_b),
                )
            )
            stack.enter_context(
                patch.object(segment, "registered_storage_binding", return_value=storage)
            )
            stack.enter_context(patch("devex_clone_seed_rebind.transition"))
            stack.enter_context(
                patch("devex_clone_cache.seed_history_proof", return_value={"path": "successor"})
            )
            stack.enter_context(
                patch("devex_clone_cache.registered_cache_binding", return_value=cache)
            )
            stack.enter_context(
                patch("devex_clone_seed_generation_prelaunch.receipt_reregistration_failure")
            )
            stack.enter_context(patch.object(recovery, "replay_authority", return_value=replay))
            stack.enter_context(patch.object(segment, "_verify_generation_tail", tail))
            value = segment.segmented_resume(
                self.backend,
                self.directory,
                attempts,
                archive,
                {"path": "C52"},
            )
        self.assertEqual(value["replay"], replay)
        self.assertIn(c68, value["records"])
        self.assertIn(c69, value["records"])
        tail.assert_called_once_with([], 69)

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return binding(path)

    def test_control_delegates_exact_c69_without_running_old_recovery(self):
        c68 = self.row(
            68,
            "seed-runtime",
            "source-generation-start",
            "failed",
            error="ValueError",
        )
        expected = {"status": recovery.STATUS}
        with patch.object(control, "_active", return_value=[c68]), \
                patch.object(recovery, "authorize", return_value=expected) as authorize, \
                patch.object(control, "status") as status:
            value = control.execute_recover(
                self.backend, self.directory, self.directory / "request.json", 69
            )
        self.assertEqual(value, expected)
        authorize.assert_called_once()
        status.assert_not_called()


if __name__ == "__main__":
    unittest.main()
