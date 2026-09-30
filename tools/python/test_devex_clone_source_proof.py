"""历史 producer 的只读停止证明；不启动、终止进程或访问业务服务。"""
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_source_proof as proof
import full_stack_process as process


class HistoricalProducerTests(unittest.TestCase):
    def setUp(self):
        self.expected = {"pid": 45940, "started": "134329416910833259", "executable": str(Path(__file__).resolve())}
        self.cim = {"count": 1, "pid": 45940, "kind": "Local", "started": "134330077346827900", "precision_ticks": 10}
        self.run = Mock(return_value=self.response(self.cim))
        self.system_root = Path(__file__).resolve().parent / "system-root"
        flag = patch.object(subprocess, "CREATE_NO_WINDOW", 0x08000000, create=True)
        flag.start()
        self.addCleanup(flag.stop)

    @staticmethod
    def response(value, code=0):
        return subprocess.CompletedProcess([], code, json.dumps(value).encode(), b"")

    @staticmethod
    def denied(code=5):
        error = PermissionError("fixture access denied")
        error.winerror = code
        return error

    def check_cim(self, value=None):
        if value is not None:
            self.run.return_value = self.response(value)
        with patch.object(proof, "process_identity", side_effect=self.denied()), \
                patch.object(proof, "os", SimpleNamespace(name="nt", environ={"SystemRoot": str(self.system_root)})), \
                patch.object(proof.Path, "is_file", return_value=True):
            proof.require_recorded_producer_stopped(self.expected, run=self.run)

    def test_absent_or_strictly_newer_native_creation_proves_old_generation_stopped(self):
        for actual in (None, {**self.expected, "started": str(int(self.expected["started"]) + 1)}):
            with self.subTest(actual=actual), patch.object(proof, "process_identity", return_value=actual):
                proof.require_recorded_producer_stopped(self.expected, run=self.run)
        self.run.assert_not_called()

    def test_same_creation_is_alive_even_if_executable_changed(self):
        for executable in (self.expected["executable"], str(Path(__file__).resolve().parent / "other.exe")):
            with self.subTest(executable=executable), patch.object(proof, "process_identity", return_value={**self.expected, "executable": executable}):
                with self.assertRaises(ValueError):
                    proof.require_recorded_producer_stopped(self.expected, run=self.run)
        self.run.assert_not_called()

    def test_earlier_or_malformed_native_identity_cannot_prove_stopped(self):
        for change in ({"started": "1"}, {"started": "unknown"}, {"started": 134330077346827900},
                       {"pid": 45941}, {"pid": True}, {"executable": "relative"}, {"executable": None}):
            with self.subTest(change=change), patch.object(proof, "process_identity", return_value={**self.expected, **change}):
                with self.assertRaises(ValueError):
                    proof.require_recorded_producer_stopped(self.expected, run=self.run)
        self.run.assert_not_called()

    def test_invalid_original_identity_rejected_before_query(self):
        for change in ({"pid": True}, {"pid": 1}, {"started": "0"}, {"started": "123;exit"}, {"executable": "relative"}):
            with self.subTest(change=change), patch.object(proof, "process_identity") as native:
                with self.assertRaises(ValueError):
                    proof.require_recorded_producer_stopped({**self.expected, **change}, run=self.run)
                native.assert_not_called()

    def test_permission_fallback_requires_windows_access_denied_and_explicit_runner(self):
        cases = (("nt", self.denied(87), self.run), ("posix", self.denied(), self.run),
                 ("nt", PermissionError("no Windows code"), self.run), ("nt", self.denied(), None),
                 ("nt", OSError("unknown error"), self.run))
        for platform, error, runner in cases:
            with self.subTest(platform=platform, error=type(error).__name__), patch.object(proof, "os", SimpleNamespace(name=platform)), \
                    patch.object(proof, "process_identity", side_effect=error):
                with self.assertRaises(type(error)) as raised:
                    proof.require_recorded_producer_stopped(self.expected, run=runner)
                self.assertIs(raised.exception, error)
        self.run.assert_not_called()

    def test_cim_uses_exact_pid_system_powershell_no_window_and_one_recorded_command(self):
        self.check_cim()
        self.run.assert_called_once()
        command = self.run.call_args.args[0]
        options = self.run.call_args.kwargs
        self.assertEqual(Path(command[0]), self.system_root / "System32/WindowsPowerShell/v1.0/powershell.exe")
        self.assertEqual(command[1:4], ["-NoProfile", "-NonInteractive", "-Command"])
        self.assertIn("-Filter 'ProcessId = 45940' -Property ProcessId,CreationDate", command[4])
        self.assertIn("$items.Count -ne 1", command[4])
        self.assertIn("[DateTimeKind]::Local,[DateTimeKind]::Utc", command[4])
        self.assertIn(".ToUniversalTime().ToFileTimeUtc()", command[4])
        for forbidden in ("CommandLine", "ExecutablePath", "Terminate", "Stop-Process", "Invoke-CimMethod"):
            self.assertNotIn(forbidden, command[4])
        self.assertEqual(options, {"stdin": subprocess.DEVNULL, "stdout": subprocess.PIPE, "stderr": subprocess.PIPE,
                                  "check": True, "timeout": 15, "creationflags": subprocess.CREATE_NO_WINDOW})

    def test_cim_microsecond_boundary_is_conservative(self):
        self.expected["started"] = "134330077346827900"
        for delta in (-20, -10, 0, 10, 20):
            with self.subTest(delta=delta):
                value = {**self.cim, "started": str(int(self.expected["started"]) + delta)}
                if delta <= 10:
                    with self.assertRaises(ValueError):
                        self.check_cim(value)
                else:
                    self.check_cim(value)

    def test_access_denied_cim_can_use_same_snapshot_process_counters(self):
        self.run.side_effect = subprocess.CalledProcessError(1, "cim")
        counter = Mock(return_value=self.response({"count": 1, "elapsed_count": 1,
            "pid": self.expected["pid"], "started": str(int(self.expected["started"]) + 2),
            "precision_ticks": 1}))
        with patch.object(proof, "process_identity", side_effect=self.denied()), \
                patch.object(proof, "os", SimpleNamespace(name="nt", environ={"SystemRoot": str(self.system_root)})), \
                patch.object(proof.Path, "is_file", return_value=True):
            proof.require_recorded_producer_stopped(self.expected, run=self.run, counter_run=counter)
        self.run.assert_called_once()
        counter.assert_called_once()
        command = counter.call_args.args[0]
        script = command[4]
        self.assertEqual(Path(command[0]), self.system_root / "System32/WindowsPowerShell/v1.0/powershell.exe")
        self.assertEqual(command[1:4], ["-NoProfile", "-NonInteractive", "-Command"])
        self.assertIn("Get-Counter '\\Process(*)\\ID Process','\\Process(*)\\Elapsed Time'", script)
        self.assertIn("-ErrorAction SilentlyContinue", script)
        self.assertIn("$ids.Count -ne 1", script)
        self.assertIn("$elapsed.Count -ne 1", script)
        self.assertIn("[string]::Equals($_.Path,$elapsedPath", script)
        self.assertIn("$now=[DateTime]::UtcNow.ToFileTimeUtc()", script)
        for forbidden in ("CommandLine", "ExecutablePath", "Terminate", "Stop-Process", "Invoke-CimMethod"):
            self.assertNotIn(forbidden, script)
        self.assertEqual(counter.call_args.kwargs, {
            "stdin": subprocess.DEVNULL, "stdout": subprocess.PIPE, "stderr": subprocess.PIPE,
            "check": True, "timeout": 15, "creationflags": subprocess.CREATE_NO_WINDOW,
        })

    def test_counter_fallback_keeps_one_tick_boundary_conservative(self):
        self.run.side_effect = subprocess.CalledProcessError(1, "cim")
        with patch.object(proof, "process_identity", side_effect=self.denied()), \
                patch.object(proof, "os", SimpleNamespace(name="nt", environ={"SystemRoot": str(self.system_root)})), \
                patch.object(proof.Path, "is_file", return_value=True):
            for delta in (0, 1, 2):
                counter = Mock(return_value=self.response({"count": 1, "elapsed_count": 1,
                    "pid": self.expected["pid"], "started": str(int(self.expected["started"]) + delta),
                    "precision_ticks": 1}))
                if delta <= 1:
                    with self.subTest(delta=delta), self.assertRaises(ValueError):
                        proof.require_recorded_producer_stopped(self.expected, run=self.run,
                                                                counter_run=counter)
                else:
                    proof.require_recorded_producer_stopped(self.expected, run=self.run,
                                                            counter_run=counter)

    @unittest.skipUnless(os.name == "nt", "Windows 性能计数器创建代次测试")
    def test_windows_counter_query_matches_current_kernel_creation_time(self):
        expected = process.process_identity(os.getpid())
        self.assertIsNotNone(expected)
        self.assertEqual(proof._counter_creation_time(os.getpid(), subprocess.run),
                         int(expected["started"]))

    def test_cim_rejects_nonunique_wrong_pid_unknown_timezone_or_precision(self):
        for change in ({"count": 0}, {"count": 2}, {"count": True}, {"pid": 45941}, {"pid": True},
                       {"kind": "Unspecified"}, {"kind": None}, {"started": None}, {"started": "134330077346827901"},
                       {"started": 134330077346827900}, {"precision_ticks": 1}, {"precision_ticks": True}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.check_cim({**self.cim, **change})
        for value in ({}, [], [self.cim, self.cim], {key: value for key, value in self.cim.items() if key != "started"}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.check_cim(value)

    def test_cim_local_and_utc_results_use_same_explicit_utc_filetime(self):
        for kind in ("Local", "Utc"):
            with self.subTest(kind=kind):
                self.check_cim({**self.cim, "kind": kind})

    def test_cim_failure_empty_and_invalid_output_never_retry_or_return_stopped(self):
        failures = (subprocess.TimeoutExpired("query", 15), subprocess.CalledProcessError(1, "query"), OSError("query failed"))
        for error in failures:
            with self.subTest(error=type(error).__name__):
                self.run.reset_mock()
                self.run.side_effect = error
                with self.assertRaises(type(error)) as raised:
                    self.check_cim()
                self.assertIs(raised.exception, error)
                self.run.assert_called_once()
        self.run.side_effect = None
        for raw, code in ((b"", 0), (b"null", 0), (b"invalid", 0), (b"\xff", 0), (json.dumps(self.cim).encode(), 1)):
            with self.subTest(raw=raw, code=code):
                self.run.reset_mock()
                self.run.return_value = subprocess.CompletedProcess([], code, raw, b"")
                with self.assertRaises((ValueError, UnicodeError)):
                    self.check_cim()
                self.run.assert_called_once()

    def test_registry_returns_original_identity_after_reuse_and_passes_explicit_runner(self):
        original = [{"name": role, "identity": {**self.expected, "pid": 45940 + index}} for index, role in enumerate(("api", "worker", "dataset"))]
        registry = {"format_version": 1, "scope_id": "source-scope", "runtime_sha256": "a" * 64, "processes": original}
        request = {"source": {"scope_id": "source-scope"}, "runtime": {"sha256": "a" * 64}}
        with patch.object(proof, "require_recorded_producer_stopped") as stopped:
            observed = proof.producer_identities(request, registry, {item["name"]: item["identity"] for item in original[:2]}, run=self.run)
        self.assertEqual(observed, sorted(original, key=lambda item: item["name"]))
        self.assertEqual(stopped.call_count, 3)
        self.assertTrue(all(call.kwargs == {"run": self.run} for call in stopped.call_args_list))

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
