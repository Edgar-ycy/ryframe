"""seed runtime 分阶段关闭、稳定排空与可续作收据。"""
from contextlib import nullcontext
import json
from pathlib import Path
import shutil
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import uuid

import devex_clone_seed_close as close
import devex_clone_seed_runtime as runtime
from devex_clone_capture import write_json
from devex_clone_run_state import begin, binding, finish, initialize_state, load_state
from rate_limit_client import ROOT
from restore_reference_plan import plan_hash


ACTIVE_FIELDS = (
    "background_job_status",
    "background_job_lease",
    "background_job_completion",
    "background_job_attempt",
    "outbox_event_status",
    "outbox_event_lease",
    "export_job_status",
    "export_job_request",
    "export_job_delete",
    "user_import_job",
    "enabled_schedule",
    "schedule_execution",
    "config_bundle",
    "config_transfer",
    "retention_run",
    "password_reset",
    "tenant_operation_lease",
    "tenant_data_migration",
    "tenant_data_migration_item",
    "file_upload",
    "tenant_status",
)

TERMINAL_FIELDS = (
    "background_job_dead",
    "background_job_attempt_failed",
    "background_job_attempt_dead",
    "background_job_attempt_deferred",
    "background_job_attempt_lease_expired",
    "outbox_event_dead",
    "export_job_failed",
    "export_job_cancelled",
    "export_job_expired",
    "user_import_partial",
    "user_import_failed",
    "user_import_cancelled",
    "schedule_skipped_misfire",
    "schedule_skipped_concurrency",
    "schedule_target_unavailable",
    "schedule_invalid_configuration",
    "config_bundle_failed",
    "config_bundle_expired",
    "config_transfer_failed",
    "retention_run_failed",
    "password_reset_expired",
)


def snapshot(*, active=None, terminal=None, moment="2026-09-05T00:00:00.000000Z"):
    active_counts = {key: 0 for key in ACTIVE_FIELDS}
    terminal_counts = {key: 0 for key in TERMINAL_FIELDS}
    active_counts.update(active or {})
    terminal_counts.update(terminal or {})
    return {"database_time": moment, "active": active_counts,
            "terminal_non_success": terminal_counts}


def local_test_directory(label):
    root = ROOT / ".local-tests/test-python"
    root.mkdir(parents=True, exist_ok=True)
    directory = root / f"{label}-{uuid.uuid4().hex}"
    directory.mkdir()
    return directory


def remove_local_test_directory(directory):
    root = ROOT / ".local-tests/test-python"
    if directory.resolve().parent != root.resolve():
        raise AssertionError("测试清理目录越界")
    shutil.rmtree(directory)


class Clock:
    def __init__(self):
        self.value = 0.0

    def now(self):
        return self.value

    def wait(self, seconds):
        self.value += seconds


class SnapshotTests(unittest.TestCase):
    def test_drain_observation_binds_current_close_attempt_to_the_target_lock(self):
        directory = Path("run")
        context = SimpleNamespace(backend=ROOT, directory_root=directory, owned_lock_identity=419)
        value = snapshot()
        handoff = {"sha256": "1" * 64}
        with patch.object(close, "load_state", return_value={"attempts": [{"number": 12}]}), \
                patch.object(close, "binding", return_value=handoff) as bind, \
                patch("devex_clone_seed.invoke_seed_tool", return_value={"snapshot": value}) as invoke:
            self.assertEqual(close.observe_snapshot(context), value)
            bind.assert_called_once_with(directory / "seed-runtime/runtime/handoff.json")
            invoke.assert_called_once_with(ROOT, "__seed-runtime-control", {
                "format_version": 1, "kind": "ryframe-seed-runtime-control", "run_dir": str(directory),
                "attempt": 12, "operation": "drain", "roles": ["worker"], "handoff": handoff,
                "target_lock_inode": 419,
            }, timeout=180)
            invoke.return_value = {"snapshot": value, "runtime": {}}
            with self.assertRaisesRegex(ValueError, "排空读取结果无效"):
                close.observe_snapshot(context)

    def test_stability_resets_after_active_snapshot_and_requires_elapsed_interval(self):
        busy = snapshot(active={"outbox_event_status": 1})
        zero_one = snapshot(moment="2026-09-05T00:00:01.000000Z")
        zero_two = snapshot(moment="2026-09-05T00:00:02.000000Z")
        observed = iter([zero_one, busy, zero_one, zero_two, zero_two])
        clock, statuses = Clock(), []
        result = close.drain_until_stable(lambda: next(observed), lambda: statuses.append("status"),
            timeout_seconds=10, stable_seconds=2, poll_seconds=1, clock=clock.now, wait=clock.wait)
        self.assertEqual(result["observations"], 5)
        self.assertEqual(result["first_snapshot"], zero_one)
        self.assertEqual(result["last_snapshot"], zero_two)
        self.assertEqual(len(statuses), 10)
        self.assertEqual(clock.value, 4)

    def test_timeout_keeps_last_snapshot_for_reconciliation(self):
        busy = snapshot(active={"background_job_status": 1})
        clock = Clock()
        with self.assertRaises(close.DrainTimeout) as raised:
            close.drain_until_stable(lambda: busy, lambda: None, timeout_seconds=2,
                stable_seconds=1, poll_seconds=1, clock=clock.now, wait=clock.wait)
        self.assertEqual(raised.exception.last_snapshot, busy)
        self.assertEqual(raised.exception.observations, 3)


class CloseTests(unittest.TestCase):
    def setUp(self):
        self.backend = ROOT
        self.test_root = local_test_directory("seed-close")
        self.addCleanup(remove_local_test_directory, self.test_root)
        self.directory = self.test_root / "run"
        self.runtime = self.directory / "seed-runtime/runtime"
        self.runtime.mkdir(parents=True)
        self.handoff = {
            "scope_id": "seed-scope", "api_url": "http://127.0.0.1:18210",
            "identity": {"proof": True}, "capacity": {"capacity": True}, "schedules": ["schedule"],
        }
        write_json(self.runtime / "handoff.json", self.handoff)
        (self.runtime / "producer-history.json").write_text("{}\n", encoding="utf-8")
        self.api_identity = {"pid": 101, "started": "1001", "executable": "api.exe"}
        self.worker_identity = {"pid": 102, "started": "1002", "executable": "worker.exe"}
        self.context = SimpleNamespace(backend=self.backend, directory_root=self.directory)
        self.private = {"FIXTURE": "value"}

    def begin_attempt(self, mode="close"):
        if not (self.directory / "manifest.json").exists():
            write_json(self.directory / "manifest.json", {"fixture": True})
            initialize_state(self.directory)
        number = begin(self.directory, "seed-runtime", mode, {"fixture": "source"})
        attempt = load_state(self.directory)["attempts"][-1]
        owner = {
            "format_version": 1,
            "identity": {"pid": 501, "started": "9001", "executable": str(self.backend / "controller.exe")},
            "directory": str(self.directory),
            "manifest_sha256": binding(self.directory / "manifest.json")["sha256"],
        }
        write_json(self.directory / f"controller-{number:04d}.json", {
            "format_version": 1, "kind": "devex-stage-controller", "owner": owner,
            "attempt": number, "attempt_sha256": plan_hash(attempt),
        })
        output = self.directory / "seed-runtime" / f"attempt-{number:04d}"
        output.mkdir()
        return number, SimpleNamespace(backend=self.backend, directory_root=self.directory, output=output)

    def control(self, operation, processes):
        return {"format_version": 1, "kind": "devex-clone-runtime-control", "operation": operation,
                "scope_id": self.handoff["scope_id"], "runtime_directory": str(self.runtime),
                "processes": processes, "producer_history": str(self.runtime / "producer-history.json")}

    def test_native_close_stop_is_bound_to_current_attempt_and_one_role(self):
        number, _ = self.begin_attempt()
        with patch("devex_clone_seed_runtime.pair_control", return_value={"runtime": self.worker_stop, "authority": None}) as control:
            result = close._control(self.private, self.backend, self.runtime, "stop", ("worker",), self.handoff)
        self.assertEqual(result, self.worker_stop)
        control.assert_called_once_with(self.backend, self.runtime, "stop", ("worker",), self.handoff, number)

    @property
    def running(self):
        return self.control("status", {
            "api": {"state": "running", "identity": self.api_identity, "ready": True},
            "worker": {"state": "running", "identity": self.worker_identity, "ready": True},
        })

    @property
    def draining(self):
        return self.control("status", {
            "api": {"state": "stopped", "identity": None, "ready": False},
            "worker": {"state": "running", "identity": self.worker_identity, "ready": True},
        })

    @property
    def closed(self):
        stopped = {"state": "stopped", "identity": None, "ready": False}
        return self.control("status", {"api": stopped, "worker": stopped})

    @property
    def api_stop(self):
        return self.control("stop", {"api": {"state": "stopped", "identity": None, "ready": False}})

    @property
    def worker_stop(self):
        return self.control("stop", {"worker": {"state": "stopped", "identity": None, "ready": False}})

    def patches(self, prior, controls, observations):
        def next_control(_private, _backend, _runtime, operation, roles, _handoff):
            if not controls:
                self.fail(f"unexpected control {operation} {roles}")
            value = controls.pop(0)
            if isinstance(value, BaseException):
                raise value
            return value
        return (
            patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)),
            patch.object(close, "fixed_guard"),
            patch.object(close, "previous_result", return_value=(prior, None)),
            patch.object(close, "recorded_identities", return_value={
                "api": self.api_identity, "worker": self.worker_identity}),
            patch.object(close, "_control", side_effect=next_control),
            patch.object(close, "observe_snapshot", side_effect=lambda _context: observations.pop(0)),
            patch.object(runtime, "stopped_evidence", return_value={"history": "closed"}),
            patch.object(close, "publish_checkpoint", return_value={"path": "checkpoint"}),
        )

    def run_with(self, prior, controls, observations, drain=None):
        contexts = self.patches(prior, controls, observations)
        with (contexts[0], contexts[1], contexts[2], contexts[3], contexts[4], contexts[5], contexts[6],
              contexts[7]):
            if drain is None:
                return close.execute_close(self.context, {"request": True}, 7)
            with patch.object(close, "drain_until_stable", side_effect=drain):
                return close.execute_close(self.context, {"request": True}, 7)

    def evidence(self, first=None, last=None):
        first, last = first or snapshot(), last or snapshot(moment="2026-09-05T00:00:02.000000Z")
        return {"stable_seconds": close.DRAIN_STABLE_SECONDS, "observations": 2,
                "first_snapshot": first, "last_snapshot": last}

    def test_close_stops_api_drains_then_stops_worker_and_only_success_marks_outbox(self):
        calls = []
        controls = [self.running, self.api_stop, self.draining, self.draining, self.worker_stop, self.closed]
        def control(private, backend, runtime_path, operation, roles, handoff):
            calls.append((operation, roles))
            return controls.pop(0)
        with patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)), \
                patch.object(close, "fixed_guard") as guard, patch.object(close, "previous_result", return_value=(None, None)), \
                patch.object(close, "recorded_identities", return_value={
                    "api": self.api_identity, "worker": self.worker_identity}), \
                patch.object(close, "_control", side_effect=control), \
                patch.object(close, "drain_until_stable", return_value=self.evidence()), \
                patch.object(close, "observe_snapshot", return_value=snapshot(terminal={"outbox_event_dead": 1})), \
                patch.object(runtime, "stopped_evidence", return_value={"history": "closed"}), \
                patch.object(close, "publish_checkpoint", return_value={"path": "checkpoint"}):
            result = close.execute_close(self.context, {"request": True}, 7)
        self.assertEqual(calls, [
            ("status", ("api", "worker")), ("stop", ("api",)), ("status", ("api", "worker")),
            ("status", ("api", "worker")), ("stop", ("worker",)), ("status", ("api", "worker")),
        ])
        self.assertEqual(guard.call_count, 3)
        self.assertEqual(result["status"], "seed_runtime_closed")
        self.assertTrue(result["outbox_drained"])
        self.assertEqual(result["direct_remote_writes"], 0)
        self.assertTrue(result["worker_drain_writes"])
        self.assertEqual(result["terminal_non_success"]["outbox_event_dead"], 1)
        self.assertFalse(controls)

    def test_timeout_preserves_worker_and_next_close_resumes_without_stopping_api_again(self):
        busy = snapshot(active={"outbox_event_status": 1})
        timeout = close.DrainTimeout(busy, 3)
        first_controls = [self.running, self.api_stop, self.draining, self.draining]
        partial = self.run_with(None, first_controls, [], drain=timeout)
        self.assertEqual((partial["phase"], partial["reason"]), ("api_stopped", "drain_timeout"))
        self.assertTrue(partial["worker_preserved"])
        self.assertFalse(partial["outbox_drained"])
        self.assertFalse(first_controls)

        second_controls = [self.draining, self.draining, self.worker_stop, self.closed]
        complete = self.run_with(partial, second_controls, [snapshot()], drain=lambda *_args, **_kwargs: self.evidence())
        self.assertEqual(complete["status"], "seed_runtime_closed")
        self.assertTrue(complete["outbox_drained"])
        self.assertFalse(second_controls)

    def test_unknown_api_stop_reconciles_running_generation_then_retries_only_api(self):
        first_controls = [self.running, RuntimeError("stop result lost"), self.running]
        partial = self.run_with(None, first_controls, [], drain=lambda *_args, **_kwargs: self.fail("must not drain"))
        self.assertEqual((partial["phase"], partial["reason"]), ("running", "api_stop_failed"))
        self.assertTrue(partial["worker_preserved"])
        self.assertFalse(partial["outbox_drained"])
        second_controls = [self.running, self.api_stop, self.draining, self.draining, self.worker_stop, self.closed]
        result = self.run_with(partial, second_controls, [snapshot()],
                               drain=lambda *_args, **_kwargs: self.evidence())
        self.assertEqual(result["status"], "seed_runtime_closed")
        self.assertFalse(second_controls)

    def test_api_stop_receipt_survives_failed_followup_observation(self):
        controls = [self.running, self.api_stop, RuntimeError("status unavailable")]
        partial = self.run_with(None, controls, [], drain=lambda *_args, **_kwargs: self.fail("must not drain"))
        self.assertEqual((partial["phase"], partial["reason"]),
                         ("api_stopped", "post_api_stop_observation_failed"))
        self.assertIsNotNone(partial["api_stop"])
        self.assertFalse(partial["outbox_drained"])

    def test_final_active_state_is_resumable_after_worker_stop(self):
        busy = snapshot(active={"export_job_status": 1})
        first_controls = [self.running, self.api_stop, self.draining, self.draining, self.worker_stop, self.closed]
        partial = self.run_with(None, first_controls, [busy], drain=lambda *_args, **_kwargs: self.evidence())
        self.assertEqual((partial["phase"], partial["reason"]),
                         ("worker_stopped", "final_state_not_drained"))
        self.assertFalse(partial["worker_preserved"])
        self.assertFalse(partial["outbox_drained"])

        second_controls = [self.closed, self.closed]
        complete = self.run_with(partial, second_controls, [snapshot()])
        self.assertEqual(complete["status"], "seed_runtime_closed")
        self.assertFalse(second_controls)

    def test_fresh_close_rejects_already_stopped_api_instead_of_guessing(self):
        with patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)), \
                patch.object(close, "fixed_guard"), patch.object(close, "previous_result", return_value=(None, None)), \
                patch.object(close, "_control", return_value=self.draining) as control:
            with self.assertRaisesRegex(ValueError, "首次 close"):
                close.execute_close(self.context, {}, 7)
        self.assertEqual(control.call_count, 1)

    def test_prior_result_rejects_other_handoff(self):
        identities = {"api": self.api_identity, "worker": self.worker_identity}
        value = close._base_result(self.runtime, self.handoff, self.draining, identities,
                                   phase="api_stopped", api_stop=self.api_stop)
        value = close._partial(value, "api_stopped", "drain_timeout", close.DrainTimeout(None, 0))
        value["scope_id"] = "other"
        with self.assertRaisesRegex(ValueError, "固定 handoff"):
            close.validate_result(value, self.runtime, self.handoff)

    def test_failed_outer_attempt_reloads_hash_bound_close_phase(self):
        write_json(self.directory / "manifest.json", {"fixture": True})
        initialize_state(self.directory)
        identities = {"api": self.api_identity, "worker": self.worker_identity}
        partial = close._base_result(self.runtime, self.handoff, self.draining, identities,
                                     phase="api_stopped", api_stop=self.api_stop)
        partial = close._partial(partial, "api_stopped", "drain_timeout",
                                 close.DrainTimeout(snapshot(active={"outbox_event_status": 1}), 3),
                                 snapshot=snapshot(active={"outbox_event_status": 1}))
        previous = begin(self.directory, "seed-runtime", "close", {})
        finish(self.directory, previous, result=partial, error=ValueError("outer reconciliation"))
        current = begin(self.directory, "seed-runtime", "close", {})
        self.assertEqual(close.previous_result(
            self.backend, self.directory, current, self.runtime, self.handoff)[0], partial)

    def test_missing_prior_result_is_reconciled_instead_of_permanently_blocked(self):
        write_json(self.directory / "manifest.json", {"fixture": True})
        initialize_state(self.directory)
        previous = begin(self.directory, "seed-runtime", "close", {})
        finish(self.directory, previous, error=RuntimeError("controller lost after API stop"))
        current = begin(self.directory, "seed-runtime", "close", {})
        self.assertIs(close.previous_result(
            self.backend, self.directory, current, self.runtime, self.handoff)[0], close.MISSING_RESULT)
        identities = {"api": self.api_identity, "worker": self.worker_identity}
        with patch.object(close, "recorded_identities", return_value=identities), \
                patch.object(close, "_status", return_value=self.draining):
            result, worker = close._reconcile_missing(self.backend, self.runtime, self.private, self.handoff)
        self.assertEqual(result["phase"], "api_stopped")
        self.assertEqual(worker, self.worker_identity)

    def test_drained_checkpoint_recovers_worker_stop_crash_without_repeating_stop(self):
        number, context = self.begin_attempt()
        first_calls = []
        first_controls = [self.running, self.api_stop, self.draining, self.draining, self.worker_stop]

        def first_control(_private, _backend, _runtime, operation, roles, _handoff):
            first_calls.append((operation, roles))
            return first_controls.pop(0)

        original_write = close._write_checkpoint

        def crash_before_worker_checkpoint(path, value):
            if path.name == close.CHECKPOINT_FILES["worker_stopped"]:
                raise KeyboardInterrupt("controller interrupted")
            return original_write(path, value)

        with patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)), \
                patch.object(close, "fixed_guard"), \
                patch.object(close, "recorded_identities", return_value={
                    "api": self.api_identity, "worker": self.worker_identity}), \
                patch.object(close, "_control", side_effect=first_control), \
                patch.object(close, "drain_until_stable", return_value=self.evidence()), \
                patch.object(close, "_write_checkpoint", side_effect=crash_before_worker_checkpoint), \
                self.assertRaises(KeyboardInterrupt):
            close.execute_close(context, {"request": True}, number)
        self.assertFalse(first_controls)
        self.assertEqual(first_calls[-1], ("stop", ("worker",)))
        self.assertTrue(close._checkpoint_path(self.directory, number, "drained").exists())
        self.assertFalse(close._checkpoint_path(self.directory, number, "worker_stopped").exists())
        finish(self.directory, number, error=KeyboardInterrupt("controller interrupted"))

        resumed_number, resumed_context = self.begin_attempt()
        resumed_calls = []
        resumed_controls = [self.closed, self.closed]

        def resumed_control(_private, _backend, _runtime, operation, roles, _handoff):
            resumed_calls.append((operation, roles))
            return resumed_controls.pop(0)

        with patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)), \
                patch.object(close, "fixed_guard"), patch.object(close, "_control", side_effect=resumed_control), \
                patch.object(close, "observe_snapshot", return_value=snapshot()), \
                patch.object(runtime, "stopped_evidence", return_value={"history": "closed"}):
            result = close.execute_close(resumed_context, {"request": True}, resumed_number)
        self.assertEqual(result["status"], "seed_runtime_closed")
        self.assertTrue(result["outbox_drained"])
        self.assertFalse(resumed_controls)
        self.assertEqual(resumed_calls, [("status", ("api", "worker")),
                                         ("status", ("api", "worker"))])

    def test_running_checkpoint_recovers_api_stop_crash_without_repeating_stop(self):
        number, context = self.begin_attempt()
        first_calls = []
        first_controls = [self.running, self.api_stop, self.draining]

        def first_control(_private, _backend, _runtime, operation, roles, _handoff):
            first_calls.append((operation, roles))
            return first_controls.pop(0)

        original_write = close._write_checkpoint

        def crash_before_api_checkpoint(path, value):
            if path.name == close.CHECKPOINT_FILES["api_stopped"]:
                raise KeyboardInterrupt("controller interrupted after API stop")
            return original_write(path, value)

        with patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)), \
                patch.object(close, "fixed_guard"), \
                patch.object(close, "recorded_identities", return_value={
                    "api": self.api_identity, "worker": self.worker_identity}), \
                patch.object(close, "_control", side_effect=first_control), \
                patch.object(close, "_write_checkpoint", side_effect=crash_before_api_checkpoint), \
                self.assertRaises(KeyboardInterrupt):
            close.execute_close(context, {"request": True}, number)
        self.assertFalse(first_controls)
        self.assertEqual(first_calls[-1], ("status", ("api", "worker")))
        self.assertTrue(close._checkpoint_path(self.directory, number, "running").exists())
        self.assertFalse(close._checkpoint_path(self.directory, number, "api_stopped").exists())
        finish(self.directory, number, error=KeyboardInterrupt("controller interrupted after API stop"))

        resumed_number, resumed_context = self.begin_attempt()
        resumed_calls = []
        resumed_controls = [self.draining, self.draining, self.worker_stop, self.closed]

        def resumed_control(_private, _backend, _runtime, operation, roles, _handoff):
            resumed_calls.append((operation, roles))
            return resumed_controls.pop(0)

        with patch.object(runtime, "runtime_inputs", return_value=(self.runtime, self.handoff, self.private)), \
                patch.object(close, "fixed_guard"), patch.object(close, "_control", side_effect=resumed_control), \
                patch.object(close, "drain_until_stable", return_value=self.evidence()), \
                patch.object(close, "observe_snapshot", return_value=snapshot()), \
                patch.object(runtime, "stopped_evidence", return_value={"history": "closed"}):
            result = close.execute_close(resumed_context, {"request": True}, resumed_number)
        self.assertEqual(result["status"], "seed_runtime_closed")
        self.assertTrue(result["outbox_drained"])
        self.assertFalse(resumed_controls)
        self.assertEqual(resumed_calls, [("status", ("api", "worker")),
                                         ("status", ("api", "worker")),
                                         ("stop", ("worker",)),
                                         ("status", ("api", "worker"))])

    def test_checkpoint_chain_rejects_self_consistent_rewrite_of_earlier_phase(self):
        number, context = self.begin_attempt()
        identities = {"api": self.api_identity, "worker": self.worker_identity}
        running = close._base_result(self.runtime, self.handoff, self.running, identities)
        close.publish_checkpoint(context, number, running, self.runtime, self.handoff)
        api_stopped = dict(running)
        api_stopped.update(phase="api_stopped", runtime_status=self.draining, api_stop=self.api_stop)
        close.publish_checkpoint(context, number, api_stopped, self.runtime, self.handoff)
        drained = dict(api_stopped)
        evidence = self.evidence()
        drained.update(phase="drained", drain=evidence, last_snapshot=evidence["last_snapshot"],
                       terminal_non_success=evidence["last_snapshot"]["terminal_non_success"])
        close.publish_checkpoint(context, number, drained, self.runtime, self.handoff)

        path = close._checkpoint_path(self.directory, number, "running")
        changed = json.loads(path.read_text(encoding="utf-8"))
        changed["result"]["reason"] = "api_stop_failed"
        changed["sha256"] = plan_hash({key: value for key, value in changed.items() if key != "sha256"})
        path.write_text(json.dumps(changed, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "摘要链"):
            close._load_checkpoint_chain(
                self.backend, self.directory, load_state(self.directory),
                load_state(self.directory)["attempts"][-1], self.runtime, self.handoff)

    def test_missing_result_checks_intervening_stage_before_checkpoint_recovery(self):
        first, _context = self.begin_attempt()
        finish(self.directory, first, error=KeyboardInterrupt("controller interrupted"))
        intervening, _context = self.begin_attempt(mode="stop")
        finish(self.directory, intervening, result={"status": "stopped"})
        current, _context = self.begin_attempt()
        with patch.object(close, "_load_checkpoint_chain") as load, self.assertRaisesRegex(
                ValueError, "已有其他阶段"):
            close.previous_result(self.backend, self.directory, current, self.runtime, self.handoff)
        load.assert_not_called()

    def test_missing_result_does_not_infer_drain_from_closed_processes(self):
        with patch.object(close, "recorded_identities", return_value={
                "api": self.api_identity, "worker": self.worker_identity}), \
                patch.object(close, "_status", return_value=self.closed), \
                self.assertRaisesRegex(ValueError, "排空期间"):
            close._reconcile_missing(self.backend, self.runtime, self.private, self.handoff)


class ProcessHistoryTests(unittest.TestCase):
    def setUp(self):
        self.directory = local_test_directory("seed-history")
        self.addCleanup(remove_local_test_directory, self.directory)
        self.identity = {"pid": 101, "started": "1000", "executable": "worker.exe"}
        self.handoff = {"scope_id": "seed-scope"}
        write_json(self.directory / "producer-history.json", {
            "format_version": 1, "kind": "devex-clone-producer-history", "scope_id": "seed-scope",
            "runtime_directory": str(self.directory), "events": [
                {"sequence": 0, "event": "start-intent", "role": "worker", "token": "one"},
                {"sequence": 1, "event": "started", "role": "worker", "token": "one",
                 "identity": self.identity},
            ],
        })

    def test_same_generation_is_alive_but_newer_pid_generation_is_closed(self):
        with patch.object(runtime, "process_identity", return_value=self.identity), self.assertRaisesRegex(
                ValueError, "代次"):
            runtime.stopped_evidence(self.directory, self.handoff)
        newer = {**self.identity, "started": "1001", "executable": "other.exe"}
        with patch.object(runtime, "process_identity", return_value=newer):
            self.assertIn("history", runtime.stopped_evidence(self.directory, self.handoff))
        with patch.object(runtime, "process_identity", return_value=None):
            self.assertIn("history", runtime.stopped_evidence(self.directory, self.handoff))

    def test_older_or_malformed_pid_generation_is_rejected(self):
        for current in ({**self.identity, "started": "999"}, {"pid": 101, "started": "new", "executable": "x"}):
            with self.subTest(current=current), patch.object(runtime, "process_identity", return_value=current), \
                    self.assertRaises(ValueError):
                runtime.stopped_evidence(self.directory, self.handoff)


class CliTests(unittest.TestCase):
    def test_seed_runtime_dispatches_close_inside_existing_target_lock(self):
        backend, directory, request, context = Path.cwd(), Path.cwd() / ".local-tests/run", {}, object()
        expected = {"status": "seed_runtime_closed"}
        with patch.object(runtime, "require_quiet") as quiet, \
                patch.object(runtime, "target_lock", return_value=nullcontext()) as lock, \
                patch.object(runtime, "registered", return_value=request), \
                patch.object(runtime, "Context", return_value=context), \
                patch.object(close, "execute_close", return_value=expected) as execute:
            self.assertEqual(runtime.execute_seed(backend, directory, {"manifest": True}, "close", 9), expected)
        quiet.assert_called_once_with(backend, directory, 9)
        lock.assert_called_once_with(backend, {"manifest": True})
        execute.assert_called_once_with(context, request, 9)


if __name__ == "__main__":
    unittest.main()
