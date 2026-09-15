"""发布源重绑定的历史、存储身份和零写入边界。"""
from contextlib import ExitStack
import copy
import hashlib
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_rebind as rebind
import devex_clone_seed_segment as segment
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash


class SeedRebindTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="seed-rebind-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        (self.directory / "results").mkdir()
        self.data = self.directory / "data"
        self.data.mkdir()
        self.successor = self.file("successor.json", {"relationship": "frozen"})
        self.published = self.file("results/0052.json", {"published": "C52"})
        self.original = {
            "request": self.file("storage-request.json", {"credentials": "hashes-only"}),
            "restart_result": self.file("results/0007.json", {"restart": "original"}),
            "attempt": 7, "api_url": "http://127.0.0.1:29000", "console_url": "http://127.0.0.1:29001",
            "data_directory": {"path": str(self.data), "device": self.data.stat().st_dev, "inode": self.data.stat().st_ino},
            "storage": {"sha256": "a" * 64, "identity": {"pid": 619001, "started": "100",
                                                       "executable": str(self.directory / "rustfs.exe")}},
        }
        self.current = copy.deepcopy(self.original)
        self.current.update(attempt=53, restart_result=self.file("results/0053.json", {"restart": "current"}))
        self.current["storage"]["identity"].update(pid=619002, started="200")
        self.source = {"directory": self.directory, "storage": {"storage": self.original},
                       "review_successor": {"source_result": self.published},
                       "registration": {"producer_lineage": self.file("lineage.json", {
                           "identities": [{"identity": self.original["storage"]["identity"]}]})},
                       "request": {"runtime": self.file("runtime.json", {"worker_ready_url": "http://127.0.0.1:29003"}),
                                   "source": {"api_url": "http://127.0.0.1:29002"}}}
        self.prefix = [self.record(52, "seed-runtime", "source-register", self.published),
                       self.record(53, "storage-target", "restart", self.current["restart_result"])]

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return binding(path)

    @staticmethod
    def record(number, stage, mode, result=None, status="passed"):
        return {"number": number, "stage": stage, "mode": mode, "status": status, "result": result}

    def value(self):
        return {"status": "seed_source_rebound", "source_registration": self.published,
                "review_successor": self.successor, "original_storage": self.original,
                "current_storage": self.current, "history_length": len(self.prefix),
                "history_sha256": plan_hash(self.prefix), "remote_writes": 0, "restore_qualified": False}

    def published_rebind(self, value=None):
        receipt = self.file("results/0054.json", value or self.value())
        return receipt, {"attempts": self.prefix + [self.record(54, "seed-runtime", "source-rebind", receipt)]}

    def patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(rebind, "require_recorded_producer_stopped"))
        stack.enter_context(patch.object(rebind, "registered_storage_binding", return_value=self.current))
        stack.enter_context(patch.object(rebind, "quiet_producers"))
        return stack

    def test_explicit_rebind_is_append_only_result_and_rechecks_every_input(self):
        state = {"attempts": self.prefix + [self.record(54, "seed-runtime", "source-rebind", status="running")]}
        original = {path: path.read_bytes() for path in self.directory.rglob("*.json")}
        with self.patches(), patch("reference_fixture_successor._source_with_loader", return_value=self.source) as source, \
                patch.object(rebind, "load_state", return_value=state), \
                patch.object(rebind, "current_storage_binding", return_value=self.current) as current, \
                patch("devex_clone_run._require_owned_run") as owned:
            value = rebind.register_rebind(self.backend, self.directory, Path(self.successor["path"]), 54)
        self.assertEqual(value, self.value())
        self.assertEqual(source.call_count, 2)
        self.assertEqual(current.call_count, 2)
        self.assertEqual(owned.call_count, 2)
        self.assertEqual(original, {path: path.read_bytes() for path in self.directory.rglob("*.json")})

    def test_rebind_rejects_other_cache_successor_before_observing_current_storage(self):
        cache = self.record(54, "cache-target", "restart", self.successor)
        state = {"attempts": self.prefix + [cache, self.record(55, "seed-runtime", "source-rebind", status="running")]}
        with self.patches(), patch("reference_fixture_successor._source_with_loader", return_value=self.source), \
                patch.object(rebind, "load_state", return_value=state), \
                patch.object(rebind, "history", return_value=[cache]), \
                patch("devex_clone_cache.seed_history_proof", return_value={"other": "successor"}), \
                patch.object(rebind, "current_storage_binding") as current, self.assertRaisesRegex(ValueError, "同一缓存工具 successor"):
            rebind.register_rebind(self.backend, self.directory, Path(self.successor["path"]), 55)
        current.assert_not_called()

    def test_default_resolution_never_probes_current_listeners_or_writes(self):
        receipt, state = self.published_rebind()
        before = {path: path.read_bytes() for path in self.directory.rglob("*.json")}
        live = Mock(side_effect=AssertionError("unexpected live probe"))
        with self.patches():
            result = rebind.resolve_storage(self.backend, self.published, self.source, state,
                                             live_storage=False, current_storage=live)
        self.assertEqual(result["source_rebind"], receipt)
        self.assertEqual(result["storage"]["storage"], self.current)
        self.assertEqual(self.source["storage"]["storage"], self.original)
        self.assertEqual(before, {path: path.read_bytes() for path in self.directory.rglob("*.json")})

    def test_live_resolution_requires_exact_current_generation_and_quiet_producers(self):
        _, state = self.published_rebind()
        with self.patches(), patch.object(rebind, "quiet_producers") as quiet:
            live = Mock(return_value=self.current)
            rebind.resolve_storage(self.backend, self.published, self.source, state, live_storage=True, current_storage=live)
            live.assert_called_once_with(self.backend, self.directory, "target")
            quiet.assert_called_once_with(self.backend, self.source)
            for changed in (None, self.original, {**self.current, "attempt": 55}):
                with self.subTest(changed=changed), self.assertRaises(ValueError):
                    rebind.resolve_storage(self.backend, self.published, self.source, state,
                                             live_storage=True, current_storage=Mock(return_value=changed))

    def test_unbound_restart_cannot_be_consumed_by_live_reader(self):
        with self.patches(), self.assertRaises(ValueError):
            rebind.resolve_storage(self.backend, self.published, self.source, {"attempts": self.prefix},
                                     live_storage=True, current_storage=Mock(return_value=self.current))

    def test_frozen_history_result_and_storage_binding_are_verified(self):
        for field, changed in (("history_sha256", "0" * 64), ("history_length", 0), ("remote_writes", 1),
                               ("restore_qualified", True), ("status", "incomplete"),
                               ("source_registration", self.successor), ("original_storage", self.current)):
            value = {**self.value(), field: changed}
            with self.subTest(field=field), self.patches():
                receipt = self.file(f"invalid-{field}.json", value)
                state = {"attempts": self.prefix + [self.record(54, "seed-runtime", "source-rebind", receipt)]}
                # 固定路径检查自身也是必要门禁；此处只替换读取内容以定向覆盖各语义字段。
                expected = self.directory / "results/0054.json"
                with patch.object(rebind, "bound_file", side_effect=lambda root, item: expected if item == receipt else Path(item["path"])), \
                        patch.object(rebind, "read_json", return_value=value), self.assertRaises(ValueError):
                    rebind.resolve_storage(self.backend, self.published, self.source, state, live_storage=False)

    def test_unknown_failed_repeated_and_post_rebind_mutating_history_rejected(self):
        receipt, state = self.published_rebind()
        for stage, mode, status in (("copy", "run", "passed"), ("storage-target", "restart", "passed"),
                                   ("storage-target", "stop", "passed"), ("seed-runtime", "source-rebind", "passed"),
                                   ("seed-runtime", "arm-input", "failed"), ("seed-runtime", "start", "running")):
            changed = {"attempts": state["attempts"] + [self.record(55, stage, mode, receipt, status)]}
            with self.subTest(stage=stage, mode=mode, status=status), self.assertRaises(ValueError):
                rebind.history(self.directory, changed, self.published)
        unfinished = {"attempts": self.prefix + [self.record(54, "seed-runtime", "source-rebind", status="failed")]}
        with self.assertRaises(ValueError):
            rebind.history(self.directory, unfinished, self.published)

    def test_current_operation_requires_actual_owned_run_and_exact_last_attempt(self):
        state = {"attempts": self.prefix + [self.record(54, "seed-runtime", "source-rebind", status="running")]}
        with patch("devex_clone_run._require_owned_run", side_effect=ValueError("wrong owner")), self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published, current=54)
        with self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published, current=53)

    def test_transition_rejects_identity_data_directory_request_endpoint_and_binary_changes(self):
        variations = [None, self.original, {**self.current, "attempt": 6}]
        for field in ("request", "data_directory", "api_url", "console_url"):
            variations.append({**self.current, field: {"changed": True}})
        for field in ("sha256", "identity"):
            value = copy.deepcopy(self.current)
            value["storage"][field] = ("b" * 64 if field == "sha256" else
                                      {**value["storage"][field], "executable": "other.exe"})
            variations.append(value)
        for value in variations:
            with self.subTest(value=value), self.patches(), self.assertRaises(ValueError):
                rebind.transition(self.backend, self.original, value)
        with self.patches(), patch.object(rebind, "require_recorded_producer_stopped",
                                         side_effect=ValueError("same creation")), self.assertRaises(ValueError):
            rebind.transition(self.backend, self.original, self.current)
        with self.patches(), patch.object(rebind, "directory_identity", side_effect=ValueError("changed inode")), self.assertRaises(ValueError):
            rebind.transition(self.backend, self.original, self.current)

    def test_transition_proves_the_full_old_creation_identity_is_stopped(self):
        with patch.object(rebind, "require_recorded_producer_stopped") as stopped:
            rebind.transition(self.backend, self.original, self.current)
        stopped.assert_called_once_with(self.original["storage"]["identity"])

    def test_bound_request_bytes_cannot_change_credentials_after_restart(self):
        Path(self.original["request"]["path"]).write_text('{"credentials":"changed"}', encoding="utf-8")
        with self.patches(), self.assertRaises(ValueError):
            rebind.transition(self.backend, self.original, self.current)

    def test_producer_lineage_and_both_ports_are_rechecked_without_external_writes(self):
        with patch.object(rebind, "require_recorded_producer_stopped") as stopped, \
                patch.object(rebind, "require_closed_port") as ports:
            rebind.quiet_producers(self.backend, self.source)
        stopped.assert_called_once_with(self.original["storage"]["identity"])
        self.assertEqual([call.args[0] for call in ports.call_args_list],
                         ["http://127.0.0.1:29002", "http://127.0.0.1:29003"])

    def test_source_export_requires_rebind_and_is_allowed_once(self):
        receipt, state = self.published_rebind()
        state["attempts"] += [self.record(55, "seed-runtime", "source-generation-start", self.successor),
                              self.record(56, "seed-runtime", "source-generation-stop", self.successor)]
        export = self.record(57, "seed-runtime", "source-export", receipt)
        rebind.history(self.directory, {"attempts": state["attempts"] + [export]}, self.published)
        for attempts in (self.prefix + [export], state["attempts"] + [export, {**export, "number": 56}]):
            with self.subTest(attempts=attempts), self.assertRaises(ValueError):
                rebind.history(self.directory, {"attempts": attempts}, self.published)
        active = {**export, "status": "running", "result": None}
        with patch("devex_clone_run._require_owned_run"):
            rebind.history(self.directory, {"attempts": state["attempts"] + [active]}, self.published, current=57)

    def test_published_generation_cannot_be_recovered_then_reused_as_export_source(self):
        receipt, state = self.published_rebind()
        state["attempts"] += [self.record(55, "seed-runtime", "source-generation-start", receipt),
                              self.record(56, "seed-runtime", "source-generation-stop", receipt),
                              self.record(57, "seed-runtime", "source-generation-recover", receipt),
                              self.record(58, "seed-runtime", "source-export", receipt)]
        with self.assertRaisesRegex(ValueError, "不能重放"):
            rebind.history(self.directory, state, self.published)

    def test_changed_runtime_history_is_not_hidden_by_old_stopped_identities(self):
        runtime_history = self.file("producer-history.json", {"events": ["stopped"]})
        source = copy.deepcopy(self.source)
        source["registration"]["producer_lineage"] = self.file("updated-lineage.json", {
            "identities": [], "runtimes": [{"history": runtime_history}]})
        Path(runtime_history["path"]).write_text('{"events":["started"]}', encoding="utf-8")
        with patch.object(rebind, "require_closed_port") as ports, self.assertRaises(ValueError):
            rebind.quiet_producers(self.backend, source)
        ports.assert_not_called()

    def test_published_restart_guard_requires_published_quiet_source_and_owned_stage(self):
        state = {"attempts": [self.prefix[0], self.record(53, "storage-target", "restart", status="running")]}
        with self.patches(), patch.object(rebind, "load_state", return_value=state), \
                patch("devex_clone_seed_source._registered_source", return_value=self.source) as frozen, \
                patch("devex_clone_run._require_owned_run") as owned, \
                patch.object(rebind, "quiet_producers") as quiet:
            rebind.published_restart_guard(self.backend, self.directory, 53)
        frozen.assert_called_once()
        quiet.assert_called_once_with(self.backend, self.source)
        owned.assert_called_once_with(self.directory)
        changed = copy.deepcopy(state)
        changed["attempts"][-1]["stage"] = "storage-source"
        with patch.object(rebind, "load_state", return_value=changed), self.assertRaises(ValueError):
            rebind.published_restart_guard(self.backend, self.directory, 53)

    def test_rebind_cli_requires_request_and_explicit_write_and_dispatches_same_path(self):
        import devex_clone_run_cli as cli
        import devex_clone_seed_runtime as runtime

        def args(*, request=None, write=False):
            return SimpleNamespace(command="seed-runtime", run_dir=self.directory,
                                   operation="source-rebind", request=request,
                                   producer_binding=None, write=write)
        with patch.object(cli, "execute") as execute:
            for request, write in ((None, False), (None, True), (Path(self.successor["path"]), False)):
                with self.subTest(request=request, write=write), self.assertRaises(ValueError):
                    cli.dispatch(args(request=request, write=write), self.backend)
            execute.assert_not_called()
        completed = {"status": "stage_finished", "stage": "seed-runtime", "mode": "source-rebind",
                     "attempt": 54, "restore_qualified": False}
        with patch.object(cli, "execute", return_value=completed) as execute:
            observed = cli.dispatch(args(request=Path(self.successor["path"]), write=True), self.backend)
        self.assertEqual(observed, completed)
        self.assertEqual(execute.call_args.kwargs["seed_request"], Path(self.successor["path"]))
        with patch.object(runtime, "require_quiet"), patch.object(rebind, "register_rebind", return_value=self.value()) as register:
            result = runtime.execute_seed(self.backend, self.directory, {}, "source-rebind", 54, Path(self.successor["path"]))
        self.assertEqual(result, self.value())
        register.assert_called_once_with(self.backend, self.directory, Path(self.successor["path"]), 54)

    def test_rebind_rejects_changed_current_generation_between_observations(self):
        state = {"attempts": self.prefix + [self.record(54, "seed-runtime", "source-rebind", status="running")]}
        changed = copy.deepcopy(self.current)
        changed["storage"]["identity"]["started"] = "300"
        with self.patches(), patch("reference_fixture_successor._source_with_loader", return_value=self.source), \
                patch.object(rebind, "load_state", return_value=state), \
                patch.object(rebind, "current_storage_binding", side_effect=[self.current, changed]), \
                patch("devex_clone_run._require_owned_run"), self.assertRaises(ValueError):
            rebind.register_rebind(self.backend, self.directory, Path(self.successor["path"]), 54)
        self.assertEqual(binding(Path(self.published["path"])), self.published)
        self.assertFalse((self.directory / "results/0054.json").exists())

    def test_duplicate_rebind_is_rejected_before_live_storage_probe(self):
        _, state = self.published_rebind()
        state["attempts"].append(self.record(55, "seed-runtime", "source-rebind", status="running"))
        with self.patches(), patch("reference_fixture_successor._source_with_loader", return_value=self.source), \
                patch.object(rebind, "load_state", return_value=state), \
                patch.object(rebind, "current_storage_binding") as current, \
                patch("devex_clone_run._require_owned_run"), self.assertRaises(ValueError):
            rebind.register_rebind(self.backend, self.directory, Path(self.successor["path"]), 55)
        current.assert_not_called()

    def test_failed_export_only_opens_exact_explicit_owned_reconcile(self):
        _, state = self.published_rebind()
        state["attempts"] += [self.record(55, "seed-runtime", "source-generation-start", self.successor),
                              self.record(56, "seed-runtime", "source-generation-stop", self.successor)]
        failed = self.record(57, "seed-runtime", "source-export", status="failed")
        active = self.record(58, "seed-runtime", "source-export-reconcile", status="running")
        state["attempts"] += [failed, active]
        with patch("devex_clone_run._require_owned_run") as owned:
            rebind.history(self.directory, state, self.published, current=58)
        owned.assert_called_once_with(self.directory)
        with patch("devex_clone_run._require_owned_run", side_effect=ValueError("not owner")), self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published, current=58)
        with self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published)
        active["mode"] = "arm-input"
        with patch("devex_clone_run._require_owned_run"), self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published, current=58)

    def test_failed_readonly_reconcile_can_retry_under_the_same_owned_run(self):
        _, state = self.published_rebind()
        state["attempts"] += [self.record(55, "seed-runtime", "source-generation-start", self.successor),
                              self.record(56, "seed-runtime", "source-generation-stop", self.successor)]
        failed_export = self.record(57, "seed-runtime", "source-export", status="failed")
        failed_reconcile = self.record(58, "seed-runtime", "source-export-reconcile", status="failed")
        active = self.record(59, "seed-runtime", "source-export-reconcile", status="running")
        state["attempts"] += [failed_export, failed_reconcile, active]
        with patch("devex_clone_run._require_owned_run") as owned:
            rebind.history(self.directory, state, self.published, current=59)
        owned.assert_called_once_with(self.directory)
        with self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published)


class SegmentedSourceResumeTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="segmented-resume-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.source = {"fixed": "execution-source"}
        self.descriptor = {"path": "C52", "bytes": 1, "sha256": "a" * 64}
        self.successor = self.file("successor.json", {"successor": "frozen"})
        self.old_storage = {"attempt": 3, "storage": {"identity": {"pid": 10}}}
        self.new_storage = {"attempt": 8, "restart_result": {"result": 8},
                            "storage": {"identity": {"pid": 11}}}
        self.cache_binding = {"attempt": 9, "restart_result": {"result": 9},
                              "request": {"cache": True}, "redis": {"pid": 12}}
        self.recovery = self.record(4, "seed-runtime", "source-generation-recover", "passed", {"closed": True})
        self.archive = {"recovery": self.recovery,
                        "receipt": {"current_storage": self.old_storage, "review_successor": self.successor}}
        self.cache_stop = self.record(5, "cache-target", "stop", "passed", {"cache-stop": True})
        self.failed_stop = self.record(6, "storage-target", "stop", "failed", error="ValueError")
        self.storage_stop = self.record(7, "storage-target", "stop", "passed", {"storage-stop": True})
        self.failure = self.file("failure.json", {"fixed": True})
        self.controller = self.file("controller.json", {"fixed": True})

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return binding(path)

    def record(self, number, stage, mode, status, result=None, *, error=None):
        return {"number": number, "stage": stage, "mode": mode, "status": status, "result": result,
                "error_type": error, "sources": self.source, "started_at": "before", "finished_at": "after"}

    def patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(segment, "_same_product_source"))
        stack.enter_context(patch.object(segment, "_require_clean_source"))
        stack.enter_context(patch.object(segment, "_verify_cache_stop",
                                         return_value={"request": self.cache_binding["request"], "result": {}}))
        stack.enter_context(patch.object(segment, "_verify_storage_stop", return_value={
            "request": {"storage": True}, "result": {"processes": [{"attempt": 2}]}}))
        stack.enter_context(patch.object(segment, "_verify_failed_storage_stop",
                                         return_value=(self.failure, self.controller)))
        stack.enter_context(patch.object(segment, "registered_storage_binding", return_value=self.new_storage))
        stack.enter_context(patch.object(rebind, "transition"))
        stack.enter_context(patch("devex_clone_cache.seed_history_proof", return_value=self.successor))
        stack.enter_context(patch("devex_clone_cache.registered_cache_binding", return_value=self.cache_binding))
        return stack

    def test_only_adjacent_stops_then_one_storage_and_cache_restart_reach_ready(self):
        attempts = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop]
        with self.patches(), patch("devex_clone_cache.seed_history_proof",
                                  return_value=self.successor) as frozen_proof:
            self.assertEqual(segment.segmented_resume(self.backend, self.directory, attempts, self.archive,
                                                     self.descriptor)["phase"], "stopped")
            storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
            attempts.append(storage)
            self.assertEqual(segment.segmented_resume(self.backend, self.directory, attempts, self.archive,
                                                     self.descriptor)["phase"], "storage-ready")
            cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
            attempts.append(cache)
            result = segment.segmented_resume(self.backend, self.directory, attempts, self.archive, self.descriptor)
        self.assertEqual(result["phase"], "ready")
        self.assertEqual(result["storage"], self.new_storage)
        self.assertEqual(result["cache"], self.cache_binding)
        frozen_proof.assert_called_once_with(
            self.backend, self.directory, cache, self.descriptor,
            frozen_successor=self.successor)

    def test_running_restarts_require_exact_last_owned_stage(self):
        base = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop]
        storage = self.record(8, "storage-target", "restart", "running")
        with self.patches(), patch("devex_clone_run._require_owned_run") as owned:
            result = segment.segmented_resume(self.backend, self.directory, base + [storage], self.archive,
                                             self.descriptor, current=8)
        self.assertEqual(result["phase"], "storage-running")
        owned.assert_called_once_with(self.directory)
        with self.patches(), self.assertRaises(ValueError):
            segment.segmented_resume(self.backend, self.directory, base + [storage], self.archive,
                                    self.descriptor)

        cache = self.record(9, "cache-target", "restart", "running")
        passed_storage = self.record(8, "storage-target", "restart", "passed",
                                     self.new_storage["restart_result"])
        with (self.patches(), patch("devex_clone_run._require_owned_run"),
              patch("devex_clone_cache.seed_history_proof",
                    return_value=self.successor) as live_proof):
            segment.segmented_resume(
                self.backend, self.directory, base + [passed_storage, cache],
                self.archive, self.descriptor, current=9)
        live_proof.assert_called_once_with(
            self.backend, self.directory, cache, self.descriptor,
            frozen_successor=None)

    def test_incomplete_reordered_repeated_or_early_generation_is_rejected(self):
        base = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop]
        storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
        cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
        invalid = [
            base[:-1],
            [self.recovery, self.failed_stop, self.cache_stop, self.storage_stop],
            base + [self.record(8, "cache-target", "restart", "passed", {"wrong": True})],
            base + [storage, self.record(9, "seed-runtime", "source-generation-start", "running")],
            base + [storage, cache, self.record(10, "storage-target", "restart", "passed", {"again": True})],
            base + [storage, cache, self.record(10, "seed-runtime", "source-rebind", "passed", {"again": True})],
            base + [storage, cache, self.record(10, "seed-runtime", "arm-input", "running")],
            base + [storage, cache, self.record(10, "seed-runtime", "source-export", "running")],
            base + [storage, cache, self.record(10, "unknown", "mode", "running")],
        ]
        with self.patches():
            for attempts in invalid:
                with self.subTest(attempts=attempts), self.assertRaises(ValueError):
                    segment.segmented_resume(self.backend, self.directory, attempts, self.archive, self.descriptor)

    def test_full_sources_of_first_two_stops_must_equal_the_closed_receipt(self):
        base = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop]
        with self.patches():
            for index in (1, 2):
                changed = copy.deepcopy(base)
                changed[index]["sources"] = {"other": index}
                with self.subTest(index=index), self.assertRaisesRegex(ValueError, "完整工具来源"):
                    segment.segmented_resume(self.backend, self.directory, changed,
                                             self.archive, self.descriptor)

    def test_tail_accepts_only_one_ordered_generation_export_lifecycle(self):
        storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
        cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
        base = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop, storage, cache]
        start = self.record(10, "seed-runtime", "source-generation-start", "passed", {"start": True})
        stop = self.record(11, "seed-runtime", "source-generation-stop", "passed", {"stop": True})
        export = self.record(12, "seed-runtime", "source-export", "passed", {"export": True})
        valid = [base + [self.record(10, "seed-runtime", "source-generation-start", "running")],
                 base + [start, self.record(11, "seed-runtime", "source-generation-stop", "running")],
                 base + [start, stop, self.record(12, "seed-runtime", "source-export", "running")],
                 base + [start, stop, export]]
        invalid = [base + [start, {**start, "number": 11}],
                   base + [start, stop, export, {**export, "number": 13}],
                   base + [start, {**stop, "number": 12}],
                   base + [start, stop, self.record(12, "seed-runtime", "arm-input", "running")],
                   base + [start, self.record(11, "seed-runtime", "source-generation-recover", "running")]]
        with self.patches():
            for attempts in valid:
                with self.subTest(valid=attempts[-1]):
                    self.assertEqual(segment.segmented_resume(
                        self.backend, self.directory, attempts, self.archive, self.descriptor)["phase"], "ready")
            for attempts in invalid:
                with self.subTest(invalid=attempts[-1]), self.assertRaises(ValueError):
                    segment.segmented_resume(self.backend, self.directory, attempts,
                                             self.archive, self.descriptor)

    def test_generation_lifecycle_rechecks_clean_product_source_for_every_stage(self):
        storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
        cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
        lifecycle = [self.record(10, "seed-runtime", "source-generation-start", "passed"),
                     self.record(11, "seed-runtime", "source-generation-stop", "passed"),
                     self.record(12, "seed-runtime", "source-export", "passed")]
        attempts = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop,
                    storage, cache, *lifecycle]
        with (self.patches(), patch.object(segment, "_same_product_source") as product,
              patch.object(segment, "_require_clean_source") as clean):
            segment.segmented_resume(
                self.backend, self.directory, attempts, self.archive, self.descriptor)
        for row in (storage, cache, *lifecycle):
            product.assert_any_call(self.recovery["sources"], row)
            clean.assert_any_call(row["sources"])

    def test_successful_export_requires_complete_adjacent_cleanup_and_keeps_restart_bindings(self):
        storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
        cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
        start = self.record(10, "seed-runtime", "source-generation-start", "passed", {"start": True})
        stop = self.record(11, "seed-runtime", "source-generation-stop", "passed", {"stop": True})
        export = self.record(12, "seed-runtime", "source-export", "passed", {"export": True})
        cache_stop = self.record(13, "cache-target", "stop", "passed", {"cache-cleanup": True})
        storage_stop = self.record(14, "storage-target", "stop", "passed", {"storage-cleanup": True})
        base = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop,
                storage, cache, start, stop, export]
        with self.patches() as stack:
            cache_verify = stack.enter_context(patch.object(
                segment, "_verify_cache_stop",
                return_value={"request": self.cache_binding["request"], "result": {}}))
            storage_verify = stack.enter_context(patch.object(
                segment, "_verify_storage_stop",
                return_value={"request": {"storage": True}, "result": {"processes": [{"attempt": 2}]}}))
            result = segment.segmented_resume(
                self.backend, self.directory, base + [cache_stop, storage_stop],
                self.archive, self.descriptor)
        self.assertEqual(result["phase"], "closed")
        self.assertEqual(result["storage"], self.new_storage)
        self.assertEqual(result["cache"], self.cache_binding)
        self.assertEqual([row["number"] for row in result["records"]], [5, 6, 7, 8, 9, 13, 14])
        self.assertEqual(cache_verify.call_count, 2)
        self.assertEqual(storage_verify.call_count, 2)

        invalid = [base + [storage_stop, cache_stop],
                   base + [cache_stop, {**storage_stop, "number": 15}],
                   base[:-1] + [cache_stop, storage_stop],
                   base + [cache_stop, storage_stop,
                           self.record(15, "storage-target", "restart", "passed", {"again": True})]]
        with self.patches():
            for attempts in invalid:
                with self.subTest(attempts=attempts[-2:]), self.assertRaises(ValueError):
                    segment.segmented_resume(self.backend, self.directory, attempts,
                                             self.archive, self.descriptor)

    def test_cleanup_running_and_between_stop_prefixes_are_only_accepted_in_order(self):
        storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
        cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
        start = self.record(10, "seed-runtime", "source-generation-start", "passed", {"start": True})
        stop = self.record(11, "seed-runtime", "source-generation-stop", "passed", {"stop": True})
        export = self.record(12, "seed-runtime", "source-export", "passed", {"export": True})
        base = [self.recovery, self.cache_stop, self.failed_stop, self.storage_stop,
                storage, cache, start, stop, export]
        running_cache = self.record(13, "cache-target", "stop", "running")
        passed_cache = self.record(13, "cache-target", "stop", "passed", {"cache-cleanup": True})
        running_storage = self.record(14, "storage-target", "stop", "running")
        with self.patches(), patch("devex_clone_run._require_owned_run") as owned:
            result = segment.segmented_resume(
                self.backend, self.directory, base + [running_cache], self.archive,
                self.descriptor, current=13)
        self.assertEqual(result["phase"], "cache-stopping")
        owned.assert_called_once_with(self.directory)
        with self.patches():
            result = segment.segmented_resume(
                self.backend, self.directory, base + [passed_cache], self.archive, self.descriptor)
        self.assertEqual(result["phase"], "cache-stopped")
        with self.patches(), patch("devex_clone_run._require_owned_run") as owned:
            result = segment.segmented_resume(
                self.backend, self.directory, base + [passed_cache, running_storage],
                self.archive, self.descriptor, current=14)
        self.assertEqual(result["phase"], "storage-stopping")
        owned.assert_called_once_with(self.directory)
        with self.patches(), self.assertRaises(ValueError):
            segment.segmented_resume(
                self.backend, self.directory, base + [running_cache], self.archive,
                self.descriptor)

    def test_closed_tail_remains_readable_through_history_and_storage_resolution(self):
        (self.directory / "results").mkdir(exist_ok=True)
        published = self.record(1, "seed-runtime", "source-register", "passed", self.descriptor)
        rebind_receipt = self.file("results/0002.json", {
            "status": "seed_source_rebound", "source_registration": self.descriptor,
            "review_successor": self.successor, "original_storage": self.old_storage,
            "current_storage": self.old_storage, "history_length": 1,
            "history_sha256": plan_hash([published]), "remote_writes": 0,
            "restore_qualified": False})
        rebound = self.record(2, "seed-runtime", "source-rebind", "passed", rebind_receipt)
        storage = self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"])
        cache = self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"])
        start = self.record(10, "seed-runtime", "source-generation-start", "passed", {"start": True})
        stop = self.record(11, "seed-runtime", "source-generation-stop", "passed", {"stop": True})
        export = self.record(12, "seed-runtime", "source-export", "passed", {"export": True})
        cache_stop = self.record(13, "cache-target", "stop", "passed", {"cache-cleanup": True})
        storage_stop = self.record(14, "storage-target", "stop", "passed", {"storage-cleanup": True})
        attempts = [published, rebound, self.recovery, self.cache_stop, self.failed_stop,
                    self.storage_stop, storage, cache, start, stop, export, cache_stop, storage_stop]
        archive = {**self.archive, "records": [self.recovery],
                   "receipt": {**self.archive["receipt"], "source_registration": self.descriptor}}
        source = {"directory": self.directory, "storage": {"storage": self.old_storage}}
        state = {"attempts": attempts}
        with self.patches(), patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive):
            later = rebind.history(self.directory, state, self.descriptor, backend=self.backend)
            resolved = rebind.resolve_storage(
                self.backend, self.descriptor, source, state, live_storage=False)
        self.assertEqual(later[-1], storage_stop)
        self.assertEqual(resolved["storage"]["storage"], self.new_storage)
        self.assertEqual(resolved["source_rebind"], rebind_receipt)

    def test_closed_tail_flows_through_public_source_generation_and_export_consumers(self):
        import devex_clone_seed_export as seed_export
        import devex_clone_seed_generation as generation
        import devex_clone_seed_source as seed_source
        import reference_fixture_successor as successor

        write_json(self.directory / "manifest.json", {"copy_stage": "source_to_seed"})
        (self.directory / "results").mkdir(exist_ok=True)
        historical = {"id": "historical-seed", "side": "seed"}
        historical_binding = self.file("historical-request.json", historical)
        source_registration = self.file("source-registration.json", {"frozen": True})
        original_source_request = self.file("source-request.json", {"physical": "same-source"})
        relationship = {
            "source_result": self.descriptor, "source_registration": source_registration,
            "predecessor_review": {"path": "review", "bytes": 1, "sha256": "b" * 64},
            "predecessor_request": {**historical_binding,
                                    "canonical_sha256": plan_hash(historical)}}
        published = self.record(1, "seed-runtime", "source-register", "passed", self.descriptor)
        rebind_receipt = self.file("results/0002.json", {
            "status": "seed_source_rebound", "source_registration": self.descriptor,
            "review_successor": self.successor, "original_storage": self.old_storage,
            "current_storage": self.old_storage, "history_length": 1,
            "history_sha256": plan_hash([published]), "remote_writes": 0,
            "restore_qualified": False})
        rows = [published, self.record(2, "seed-runtime", "source-rebind", "passed", rebind_receipt),
                self.recovery, self.cache_stop, self.failed_stop, self.storage_stop,
                self.record(8, "storage-target", "restart", "passed", self.new_storage["restart_result"]),
                self.record(9, "cache-target", "restart", "passed", self.cache_binding["restart_result"]),
                self.record(10, "seed-runtime", "source-generation-start", "passed", {"start": True}),
                self.record(11, "seed-runtime", "source-generation-stop", "passed")]
        export_row = self.record(12, "seed-runtime", "source-export", "passed")
        rows.extend((export_row, self.record(13, "cache-target", "stop", "passed", {"cache": True}),
                     self.record(14, "storage-target", "stop", "passed", {"storage": True})))
        state = {"attempts": rows}
        archive = {**self.archive, "records": [self.recovery],
                   "receipt": {**self.archive["receipt"], "source_registration": self.descriptor}}
        source_environment = self.file("source-environment.json", {"environment": {}})
        original_generation = {"physical_binding": {"same": "source"}}
        registered = {"directory": self.directory, "result": {"registration": source_registration},
                      "registration": {"source_request": original_source_request,
                                       "source_environment": source_environment,
                                       "generation_verified": self.file(
                                           "original-generation.json", original_generation)},
                      "seed_target": historical,
                      "request": read_json(Path(original_source_request["path"])),
                      "storage": {"storage": self.old_storage}, "environment": {"environment": {}},
                      "generation": original_generation}

        generation_output = self.directory / "g0010"
        (generation_output / "runtime").mkdir(parents=True)
        runtime_value = {"runtime": "frozen"}
        runtime_binding = self.file("g0010/runtime/runtime.json", runtime_value)
        build_value = {"sources": {"full": {"source": {"snapshot": {"head": "a" * 40}}}}}
        backend_build = self.file("g0010/backend-build.json", build_value)
        maintenance = self.file("g0010/maintenance.json", {"maintenance": True})
        processes = {role: self.file(f"g0010/{role}.json", {"identity": {"role": role}})
                     for role in ("api", "worker")}
        producers = [{"name": "api", "identity": {"role": "api"}},
                     {"name": "worker", "identity": {"role": "worker"}}]
        producers_registry = self.file("g0010/producers.json", {"processes": producers})
        effective_request = {"runtime": runtime_binding, "backend_build": backend_build,
                             "maintenance_build": maintenance, "processes": processes,
                             "producers_registry": producers_registry,
                             "source": {"scope_id": "same-source"},
                             "worktree_fingerprint": "sha256:" + "d" * 64}
        write_json(generation_output / "source-request.json", effective_request)
        source_request = binding(generation_output / "source-request.json")
        expected_generation = {
            "request_sha256": plan_hash(effective_request), "runtime": runtime_value,
            "source": build_value["sources"]["full"]["source"]["snapshot"],
            "worktree_fingerprint": effective_request["worktree_fingerprint"],
            "maintenance": {"maintenance": True},
            "processes": {role: {"role": role} for role in ("api", "worker")},
            "operator_declared_producers": producers,
            "physical_binding": registered["generation"]["physical_binding"],
            "external_writers_discovered": False, "clone_verified": False,
            "restore_qualified": False}
        write_json(generation_output / "generation-verified.json", expected_generation)
        image = {"image": {"same": True}}
        images = {}
        for name in ("before", "running", "stop-before", "after"):
            (generation_output / name).mkdir()
            images[name] = self.file(f"g0010/{name}/image.json", image)
        start_result = self.file("g0010/start.json", {"start": True})
        dataset_lineage = self.file("g0010/dataset-lineage.json", {"lineage": True})
        runtime_evidence = self.file("g0010/runtime-evidence.json", {"runtime": True})
        source_runtime = self.file("g0010/source-runtime.json", {"source": True})
        request = {"source_registration": self.descriptor, "source_rebind": rebind_receipt,
                   "review_successor": self.successor, "current_storage": self.new_storage,
                   "source_environment": source_environment}
        request_binding = self.file("g0010/request.json", request)
        generation_value = {
            "status": "seed_source_generation_published", "request": request_binding,
            **{key: request[key] for key in ("source_registration", "source_rebind",
                                             "review_successor", "current_storage")},
            "history_length": len(rows[:9]), "history_sha256": plan_hash(rows[:9]),
            "source_request": source_request,
            "generation_verified": binding(generation_output / "generation-verified.json"),
            "runtime_evidence": runtime_evidence, "before": images["before"],
            "after": images["after"], "dataset_lineage": dataset_lineage,
            "start": start_result, "source_runtime": source_runtime,
            "running": images["running"], "stop_before": images["stop-before"],
            "remote_writes": 0, "restore_qualified": False}
        generation_result = self.file("results/0011.json", generation_value)
        rows[9]["result"] = generation_result

        output = self.directory / "e0012"
        output.mkdir()
        exported = {"request": source_request, "logical_inventory_sha256": "c" * 64,
                    "databases": [{"key": "control"}], "objects": [{"entries": []}]}
        write_json(output / "export.json", exported)
        controller = self.directory / "controller-0012.json"
        initial = {**copy.deepcopy(export_row), "status": "running", "finished_at": None,
                   "result": None, "error_type": None}
        write_json(controller, {"format_version": 1, "kind": "devex-stage-controller",
                                "owner": {"directory": str(self.directory), "manifest_sha256": binding(
                                    self.directory / "manifest.json")["sha256"]},
                                "attempt": 12, "attempt_sha256": plan_hash(initial)})
        intent = {"format_version": 1, "kind": "devex-clone-export-attempt", "attempt": 12,
                  "output": str(output), "manifest": binding(self.directory / "manifest.json"),
                  "controller": binding(controller), "source_request": source_request,
                  "source_storage": self.new_storage, "execution_source": export_row["sources"],
                  "remote_operations": "read_only"}
        write_json(self.directory / "export-0012.intent.json", intent)
        export_binding = binding(output / "export.json")
        summary = {"export_sha256": plan_hash(exported), "generation_sha256": plan_hash({}),
                   "proof_files_sha256": plan_hash([]), "logical_inventory_sha256": "c" * 64,
                   "databases": 1, "objects": 0}
        write_json(self.directory / "export-0012.verified.json", {
            "format_version": 1, "kind": "devex-clone-export-verified", "attempt": 12,
            "intent": binding(self.directory / "export-0012.intent.json"), "export": export_binding,
            "source_storage": self.new_storage, "summary": summary, "remote_writes": 0})
        export_value = {"status": "seed_source_export_published", "origin_attempt": 12,
                        "origin_intent": binding(self.directory / "export-0012.intent.json"),
                        "source_registration": self.descriptor, "source_rebind": rebind_receipt,
                        "source_generation": generation_result, "review_successor": self.successor,
                        "source_request": source_request, "source_storage": self.new_storage,
                        "export": export_binding, "summary": summary, "remote_writes": 0,
                        "restore_qualified": False}
        export_descriptor = self.file("results/0012.json", export_value)
        export_row["result"] = export_descriptor

        def load_registered(backend, descriptor, *, live_storage, validate_seed_target):
            self.assertFalse(live_storage)
            validate_seed_target(backend, historical)
            return copy.deepcopy(registered)

        resolved_source = copy.deepcopy(registered)
        resolved_source["storage"]["storage"] = self.new_storage
        resolved_source["source_rebind"] = rebind_receipt
        facts = {"directory": self.directory, "output": generation_output,
                 "request": request, "source": resolved_source, "execution": self.backend,
                 "receipt": {"request": request_binding, "dataset_lineage": dataset_lineage,
                             "before": images["before"], "running": images["running"]}}
        verified_runtime = {"receipt": {"source_generation": start_result,
                                        "dataset_lineage": dataset_lineage},
                            "after": image}

        with (self.patches(), patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive),
              patch.object(seed_source, "_registered_source", side_effect=load_registered),
              patch.object(seed_source, "load_state", return_value=state),
              patch.object(seed_source, "_validate_evidence_bindings"),
              patch.object(generation, "resolve_generation",
                           wraps=generation.resolve_generation) as resolved,
              patch.object(generation, "verify_running_source", return_value=facts),
              patch.object(generation, "validate_request"),
              patch.object(generation, "source_request", return_value=effective_request),
              patch("devex_clone_seed_generation_images.verify_image",
                    side_effect=lambda _backend, descriptor, *_args, **_kwargs: read_json(
                        Path(descriptor["path"]))),
              patch("devex_clone_seed_generation_runtime.verify_runtime_evidence",
                    return_value={"physical_binding": registered["generation"]["physical_binding"]}),
              patch("restore_source_runtime.verify_source_runtime", return_value=verified_runtime),
              patch.object(successor, "_validated_relationship", return_value=relationship),
              patch.object(successor, "pending_request_binding"),
              patch.object(seed_export, "load_state", return_value=state)):
            public = successor.published_source(self.backend, self.successor, live_storage=False)
            adopted = seed_export.published_export(self.backend, export_descriptor, public)
        resolved.assert_called_once()
        self.assertEqual(public["storage"]["storage"], self.new_storage)
        self.assertEqual(adopted, export_value)

    def test_cache_stop_rows_are_bound_to_each_generation_runtime_or_predecessor_proof(self):
        (self.directory / "results").mkdir(exist_ok=True)
        output = self.directory / "cache-target/a0004"
        output.mkdir(parents=True)
        identity = {"pid": 1}
        runtime = {"format_version": 1, "process_receipt": {"sha256": "a" * 64},
                   "boot_id": "boot", "linux_identity": identity}
        attempt = self.record(5, "cache-target", "stop", "passed")
        request = {"cache": "request"}
        descriptor = {"path": "cache-request", "sha256": "d" * 64}
        direct = {"status": "redis_process_stopped", "alive": False, "boot_id": "boot",
                  "identity": identity, "resources_deleted": False,
                  "runtime": runtime, "terminated": True}
        attempt["result"] = self.file("results/0005.json", {
            "status": "cache_stopped", "side": "target", "request": descriptor,
            "processes": [{"attempt": 4, "process": direct}], "copy_usable": False,
            "remote_writes": 0, "resources_deleted": False})
        generation = [(self.record(4, "cache-target", "restart", "passed"), output, runtime)]
        with (patch("devex_clone_cache.registration", return_value=(request, descriptor)),
              patch("devex_clone_cache.starts", return_value=generation)):
            segment._verify_cache_stop(self.backend, self.directory, [attempt], attempt,
                                       self.descriptor, self.successor)
        changed = read_json(Path(attempt["result"]["path"]))
        changed["processes"][0]["process"]["runtime"] = {"changed": True}
        result_path = Path(attempt["result"]["path"])
        result_path.unlink()
        write_json(result_path, changed)
        attempt["result"] = binding(result_path)
        with (patch("devex_clone_cache.registration", return_value=(request, descriptor)),
              patch("devex_clone_cache.starts", return_value=generation), self.assertRaises(ValueError)):
            segment._verify_cache_stop(self.backend, self.directory, [attempt], attempt,
                                       self.descriptor, self.successor)

        observation = {"state": "stopped", "launcher_alive": False,
                       "linux": {"alive": False, "boot_id": "boot", "identity": {}, "terminated": False},
                       "tools_sha256": "e" * 64}
        proof_path = output / "predecessor-0004.json"
        write_json(proof_path, {"process_sha256": "f" * 64,
                                "runtime_sha256": plan_hash(runtime), "observation": observation})
        predecessor = {**observation, "evidence": binding(proof_path)}
        _ = segment._stopped_cache_process(
            self.backend, predecessor, 4, runtime, (output,))
        tampered = copy.deepcopy(predecessor)
        tampered["linux"]["alive"] = True
        with self.assertRaises(ValueError):
            segment._stopped_cache_process(self.backend, tampered, 4, runtime, (output,))

    def test_failed_stop_binds_historical_call_semantics_and_empty_partial_output(self):
        write_json(self.directory / "manifest.json", {"fixed": True})
        attempt = self.record(2, "storage-target", "stop", "failed", error="ValueError")
        owner = {"format_version": 1, "directory": str(self.directory),
                 "manifest_sha256": binding(self.directory / "manifest.json")["sha256"],
                 "identity": {"pid": 991001, "started": "12345",
                              "executable": str(self.backend / "python.exe")}}
        source = {"snapshot": {"head": "a" * 40, "clean": True, "files": [],
                               "patch_sha256": hashlib.sha256(b"").hexdigest()},
                  "worktree_fingerprint": "sha256:" + "b" * 64,
                  "fingerprints": {"product": {"sha256": "c" * 64, "files": 0}}}
        attempt["sources"] = source
        initial = {**copy.deepcopy(attempt), "status": "running", "finished_at": None,
                   "result": None, "error_type": None}
        controller_path = self.directory / "controller-0002.json"
        write_json(controller_path, {"format_version": 1, "kind": "devex-stage-controller", "owner": owner,
                                     "attempt": 2, "attempt_sha256": plan_hash(initial)})
        failure_path = self.directory / "failure-0002.json"
        write_json(failure_path, {"format_version": 1, "kind": "devex-stage-failure", "attempt": 2,
                                  "stage": "storage-target", "mode": "stop", "error_type": "ValueError",
                                  "frames": [{"file": file, "function": function, "line": index + 10}
                                             for index, (file, function) in enumerate(segment._FAILED_STOP_CALLS)],
                                  "controller": binding(controller_path)})
        partial = self.directory / "storage-target/a0002/p0001"
        partial.mkdir(parents=True)
        with patch.object(segment, "require_recorded_producer_stopped") as stopped:
            evidence = segment._verify_failed_storage_stop(self.backend, self.directory, attempt, attempt, 1)
        self.assertEqual(evidence, (binding(failure_path), binding(controller_path)))
        stopped.assert_called_once_with(owner["identity"])
        value = read_json(failure_path)
        value["frames"][3]["function"] = "other"
        failure_path.unlink()
        write_json(failure_path, value)
        with patch.object(segment, "require_recorded_producer_stopped"), self.assertRaises(ValueError):
            segment._verify_failed_storage_stop(self.backend, self.directory, attempt, attempt, 1)


if __name__ == "__main__":
    unittest.main()
