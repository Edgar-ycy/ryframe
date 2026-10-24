"""历史 producer 停止证明的受管原生桥接；不运行真实进程或服务。"""
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_source_proof as proof
import full_stack_process as process
import native_s3_client
from rate_limit_tool import tool_binding


class HistoricalProducerTests(unittest.TestCase):
    def setUp(self):
        self.expected = {"pid": 45940, "started": "134329416910833259", "executable": str(Path(__file__).resolve())}
        self.run = Mock()
        if tool_binding() is None:
            raise RuntimeError("本测试必须通过正式 run_checks --rate-limit-tool-binding 入口安装协调器绑定")

    @staticmethod
    def response(value):
        return subprocess.CompletedProcess([], 0, json.dumps({"ok": True, "result": {"value": value}}).encode(), b"")

    def test_bridge_sends_original_identity_once_using_bound_native_tool(self):
        with patch.object(proof.subprocess, "run", return_value=self.response({"stopped": True, "signals_sent": 0})) as run:
            proof.require_recorded_producer_stopped(self.expected)
        run.assert_called_once()
        self.assertEqual(run.call_args.args[0][1:], ["__source-evidence"])
        self.assertEqual(json.loads(run.call_args.kwargs["input"]), {
            "operation": "recorded-process-stopped", "write": False,
            "arguments": {"identity": self.expected}})

    def test_bridge_rejects_nonexact_or_mistyped_success(self):
        for value in ({}, {"stopped": True}, {"stopped": 1, "signals_sent": 0},
                      {"stopped": False, "signals_sent": 0}, {"stopped": True, "signals_sent": False},
                      {"stopped": True, "signals_sent": 0.0}, {"stopped": True, "signals_sent": 1}, {"stopped": True, "signals_sent": 0, "extra": True}):
            with self.subTest(value=value), patch.object(proof.subprocess, "run", return_value=self.response(value)) as run:
                with self.assertRaises(ValueError):
                    proof.require_recorded_producer_stopped(self.expected)
                run.assert_called_once()

    def test_native_failure_propagates_without_retry_or_unbound_fallback(self):
        with patch.object(proof.subprocess, "run", side_effect=OSError("native unavailable")) as run:
            with self.assertRaises(native_s3_client.NativeS3InvocationError):
                proof.require_recorded_producer_stopped(self.expected)
        run.assert_called_once()
        with patch.object(native_s3_client, "tool_binding", return_value=None), patch.object(proof.subprocess, "run") as run:
            with self.assertRaises(native_s3_client.NativeS3InvocationError):
                proof.require_recorded_producer_stopped(self.expected)
        run.assert_not_called()

    def test_registry_returns_original_identity_and_passes_no_legacy_kwargs(self):
        original = [{"name": role, "identity": {**self.expected, "pid": 45940 + index}} for index, role in enumerate(("api", "worker", "dataset"))]
        registry = {"format_version": 1, "scope_id": "source-scope", "runtime_sha256": "a" * 64, "processes": original}
        request = {"source": {"scope_id": "source-scope"}, "runtime": {"sha256": "a" * 64}}
        with patch.object(proof, "require_recorded_producer_stopped") as stopped:
            observed = proof.producer_identities(request, registry, {item["name"]: item["identity"] for item in original[:2]}, run=self.run)
        self.assertEqual(observed, sorted(original, key=lambda item: item["name"]))
        self.assertEqual(stopped.call_count, 3)
        self.assertEqual([call.args[0] for call in stopped.call_args_list], [item["identity"] for item in original])
        self.assertTrue(all(not call.kwargs for call in stopped.call_args_list))

    def test_generation_forwards_its_recorded_runner_to_producer_proof(self):
        from devex_clone_source_fixture import SourceFixture

        fixture = SourceFixture(self)
        with patch.object(proof, "producer_identities", wraps=proof.producer_identities) as producers:
            proof.verify_generation(fixture.backend, fixture.request, fixture)
        producers.assert_called_once()
        self.assertIs(producers.call_args.kwargs["run"], fixture)

    def test_termination_still_rejects_reused_identity_without_calling_terminate(self):
        kernel = Mock()
        opened = Mock()
        opened.__enter__ = Mock(return_value=(kernel, 123))
        opened.__exit__ = Mock(return_value=False)
        current = {**self.expected, "started": str(int(self.expected["started"]) + 100)}
        with patch.object(process.os, "name", "nt"), patch.object(process, "_windows_handle", return_value=opened), \
                patch.object(process, "_windows_identity", return_value=current), self.assertRaises(ValueError):
            process.terminate_owned_process(self.expected)
        kernel.TerminateProcess.assert_not_called()


if __name__ == "__main__":
    unittest.main()
