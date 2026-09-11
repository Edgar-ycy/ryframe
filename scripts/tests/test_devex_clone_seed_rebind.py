"""发布源重绑定的历史、存储身份和零写入边界。"""
from contextlib import ExitStack
import argparse
import copy
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_rebind as rebind
from devex_clone_capture import write_json
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
        stack.enter_context(patch.object(rebind, "process_identity", return_value=None))
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
        with self.patches(), patch.object(rebind, "process_identity", return_value={"reused": True}), self.assertRaises(ValueError):
            rebind.transition(self.backend, self.original, self.current)
        with self.patches(), patch.object(rebind, "directory_identity", side_effect=ValueError("changed inode")), self.assertRaises(ValueError):
            rebind.transition(self.backend, self.original, self.current)

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
        export = self.record(55, "seed-runtime", "source-export", receipt)
        rebind.history(self.directory, {"attempts": state["attempts"] + [export]}, self.published)
        for attempts in (self.prefix + [export], state["attempts"] + [export, {**export, "number": 56}]):
            with self.subTest(attempts=attempts), self.assertRaises(ValueError):
                rebind.history(self.directory, {"attempts": attempts}, self.published)
        active = {**export, "status": "running", "result": None}
        with patch("devex_clone_run._require_owned_run"):
            rebind.history(self.directory, {"attempts": state["attempts"] + [active]}, self.published, current=55)

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

        parser = argparse.ArgumentParser()
        cli.add_commands(parser.add_subparsers(dest="command", required=True))
        args = ["seed-runtime", "--backend-dir", str(self.backend), "--run-dir", str(self.directory),
                "--operation", "source-rebind"]
        with patch.object(cli, "execute") as execute:
            for extra in ([], ["--write"], ["--request", self.successor["path"]]):
                with self.subTest(extra=extra), self.assertRaises(ValueError):
                    cli.dispatch(parser.parse_args(args + extra), self.backend)
            execute.assert_not_called()
        completed = {"status": "stage_finished", "stage": "seed-runtime", "mode": "source-rebind",
                     "attempt": 54, "restore_qualified": False}
        with patch.object(cli, "execute", return_value=completed) as execute:
            observed = cli.dispatch(parser.parse_args(args + ["--request", self.successor["path"], "--write"]), self.backend)
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
        failed = self.record(55, "seed-runtime", "source-export", status="failed")
        active = self.record(56, "seed-runtime", "source-export-reconcile", status="running")
        state["attempts"] += [failed, active]
        with patch("devex_clone_run._require_owned_run") as owned:
            rebind.history(self.directory, state, self.published, current=56)
        owned.assert_called_once_with(self.directory)
        with patch("devex_clone_run._require_owned_run", side_effect=ValueError("not owner")), self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published, current=56)
        with self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published)
        active["mode"] = "arm-input"
        with patch("devex_clone_run._require_owned_run"), self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published, current=56)

    def test_failed_readonly_reconcile_can_retry_under_the_same_owned_run(self):
        _, state = self.published_rebind()
        failed_export = self.record(55, "seed-runtime", "source-export", status="failed")
        failed_reconcile = self.record(56, "seed-runtime", "source-export-reconcile", status="failed")
        active = self.record(57, "seed-runtime", "source-export-reconcile", status="running")
        state["attempts"] += [failed_export, failed_reconcile, active]
        with patch("devex_clone_run._require_owned_run") as owned:
            rebind.history(self.directory, state, self.published, current=57)
        owned.assert_called_once_with(self.directory)
        with self.assertRaises(ValueError):
            rebind.history(self.directory, state, self.published)


if __name__ == "__main__":
    unittest.main()
