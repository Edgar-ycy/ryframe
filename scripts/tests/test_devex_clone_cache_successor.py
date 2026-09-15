"""已发布 seed 的缓存工具换代只继承已验证 successor，不改变资源与旧登记。"""
from contextlib import ExitStack
import copy
from pathlib import Path
from unittest.mock import patch
import json

import devex_clone_cache_request as model
from devex_clone_capture import write_json
from devex_clone_run_state import binding
from devex_clone_run_state import begin, finish, load_state, run_lock
from devex_clone_capture import read_json
from restore_reference_plan import plan_hash
import devex_clone_cache as cache
import devex_clone_cache_process as process
from test_devex_clone_cache import CacheFixture


class SuccessorRequestTests(CacheFixture):
    def successor(self):
        successor = self.json("successor.json", {"kind": "devex-clone-seed-review-successor"})
        Path(self.request["process"]["wsl"]["path"]).write_bytes(b"upgraded-wsl")
        wsl = binding(Path(self.request["process"]["wsl"]["path"]))
        self.tools = {
            "wsl": {key: wsl[key] for key in ("path", "sha256")},
            "redis_python": {"distribution": "Ubuntu-24.04", "path": "/usr/bin/python3",
                             "resolved_path": "/usr/bin/python3.12", "sha256": "d" * 64},
            "redis_server": {"distribution": "Ubuntu-24.04", "resolved_path": self.previous["executable"],
                             "sha256": self.previous["sha256"]},
        }
        self.ready = self.json("ready-review.json", {"tools": self.tools})
        self.source = {"directory": self.directory, "manifest": self.value, "seed_target": self.original,
                       "review_successor": {"successor_review": self.ready, "predecessor_review": self.ready}}
        return successor

    def successor_patches(self):
        stack = self.patches()
        stack.enter_context(patch("reference_fixture_successor._source_with_loader", return_value=self.source))
        stack.enter_context(patch("reference_fixture_environment.validate_current_review_tools", side_effect=lambda _: self.tools))
        stack.enter_context(patch.object(model, "pending_request_binding", return_value=(self.review, self.selected)))
        return stack

    def test_successor_recomputes_only_tool_fields_and_preserves_original_bytes(self):
        successor = self.successor()
        original = copy.deepcopy(self.request)
        files = {path: path.read_bytes() for path in self.root.rglob("*.json")}
        with self.successor_patches():
            effective, private, source = model.successor_process(
                self.backend, self.directory, self.value, self.request, successor)
        expected = {**original["process"], "wsl": self.tools["wsl"],
                    "python": {"path": "/usr/bin/python3", "executable": "/usr/bin/python3.12", "sha256": "d" * 64}}
        self.assertEqual(effective, expected)
        self.assertEqual(private, self.private)
        self.assertEqual(source, self.source)
        self.assertEqual(self.request, original)
        self.assertEqual(files, {path: path.read_bytes() for path in self.root.rglob("*.json")})

    def test_successor_rejects_other_run_resource_tool_or_environment(self):
        successor = self.successor()
        changes = [("distribution", "other"), ("resolved_path", "/usr/bin/other"), ("sha256", "f" * 64)]
        with self.successor_patches():
            for key, changed in changes:
                before = self.tools["redis_server"][key]
                self.tools["redis_server"][key] = changed
                with self.subTest(key=key), self.assertRaises(ValueError):
                    model.successor_process(self.backend, self.directory, self.value, self.request, successor)
                self.tools["redis_server"][key] = before
            self.source["directory"] = self.root
            with self.assertRaises(ValueError):
                model.successor_process(self.backend, self.directory, self.value, self.request, successor)

    def test_changed_original_request_is_not_authorized_by_successor(self):
        successor = self.successor()
        with self.successor_patches():
            for field, value in (("port", 16391), ("scope_id", "other"),
                                 ("configuration", self.ready), ("previous_run_id", "9" * 40)):
                changed = copy.deepcopy(self.request)
                changed["process"][field] = value
                with self.subTest(field=field), self.assertRaises(ValueError):
                    model.successor_process(self.backend, self.directory, self.value, changed, successor)


class SuccessorLifecycleTests(CacheFixture):
    def seed(self):
        self.execute("restart")
        self.alive = False
        with run_lock(self.directory):
            number = begin(self.directory, "seed-runtime", "source-register", {})
            finish(self.directory, number, result={"status": "seed_source_registered"})
        self.source = {"review_successor": {"source_result": binding(self.directory / "results/0002.json")}}
        successor = self.json("successor.json", {"kind": "devex-clone-seed-review-successor"})
        self.request_file = Path(successor["path"])
        self.effective = copy.deepcopy(self.request["process"])
        self.effective["python"]["sha256"] = "d" * 64
        Path(self.effective["wsl"]["path"]).write_bytes(b"current-wsl")
        current = binding(Path(self.effective["wsl"]["path"]))
        self.effective["wsl"] = {key: current[key] for key in ("path", "sha256")}
        return successor

    def generation_patches(self):
        stack = ExitStack()
        stack.enter_context(patch("devex_clone_seed_rebind.quiet_producers"))
        stack.enter_context(patch.object(cache, "producers_stopped", side_effect=lambda backend, directory, _value:
            cache.published_restart_guard(backend, directory, load_state(directory)["attempts"][-1]["number"])))
        stack.enter_context(patch.object(cache, "successor_process", side_effect=lambda *_args, **_kw: (
            copy.deepcopy(self.effective), self.private, self.source)))
        def observe(request, environment, base, output=None):
            value = self.observe(request, environment, base)
            value["redis"]["wsl"] = request["wsl"]
            return value
        stack.enter_context(patch.object(process, "observe", side_effect=observe))
        stack.enter_context(patch.object(process, "predecessor_status", side_effect=lambda _old, _runtime, current: {
            "state": "stopped", "launcher_alive": False,
            "tools_sha256": plan_hash({key: current[key] for key in ("wsl", "python")})}))
        original_start = self.start
        def start(request, environment, output, guard):
            value = original_start(request, environment, output, guard)
            value["redis"]["wsl"] = request["wsl"]
            return value
        stack.enter_context(patch.object(process, "start", side_effect=start))
        return stack

    def test_successor_restart_preserves_registration_and_publishes_bound_death_proof(self):
        with self.patches():
            self.seed()
            originals = {path: path.read_bytes() for path in (self.directory / cache.STAGE).glob("*.json")}
            with self.generation_patches():
                self.execute("restart")
                proof = cache.registered_cache_binding(self.backend, self.directory)
                self.assertEqual(proof["redis"]["wsl"], self.effective["wsl"])
                self.assertEqual(proof["attempt"], 3)
                self.assertIn("tools", proof)
                self.assertEqual(cache.cache_status(self.backend, self.directory)["processes"][0]["process"]["state"], "stopped")
                self.execute("stop")
            self.assertEqual(originals, {path: path.read_bytes() for path in originals})
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 2)

    def test_successor_unknown_write_requires_reconciliation_and_never_replays(self):
        with self.patches():
            self.seed()
            with self.generation_patches():
                self.failure = "after"
                with self.assertRaises(TimeoutError):
                    self.execute("restart")
                self.assertIsNotNone(cache.preflight_successor(self.backend, self.directory, self.value, mode="resume"))
                with self.assertRaises(ValueError):
                    self.execute("resume")
                self.failure = None
                self.assertIsNotNone(cache.preflight_successor(self.backend, self.directory, self.value, mode="reconcile"))
                self.execute("reconcile")
                self.assertEqual(cache.registered_cache_binding(self.backend, self.directory)["attempt"], 5)
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 2)

    def test_tampered_tool_or_death_evidence_cannot_publish_or_reuse_old_success(self):
        with self.patches():
            self.seed()
            with self.generation_patches():
                self.execute("restart")
                path = self.directory / cache.STAGE / "a0003/predecessor-0001.json"
                value = read_json(path)
                value["observation"]["state"] = "running"
                path.write_text(json.dumps(value), encoding="utf-8")
                with self.assertRaises(ValueError):
                    cache.registered_cache_binding(self.backend, self.directory)

    def test_different_successor_is_rejected_before_new_stage_or_file(self):
        with self.patches():
            self.seed()
            with self.generation_patches(), patch("devex_clone_seed_rebind.quiet_producers"):
                self.execute("restart")
                other = self.json("other-successor.json", {"kind": "devex-clone-seed-review-successor", "id": "other"})
                before = {path: path.read_bytes() for path in self.directory.rglob("*") if path.is_file()}
                with self.assertRaisesRegex(ValueError, "同一 successor"):
                    cache.preflight_successor(self.backend, self.directory, self.value, Path(other["path"]))
                self.assertEqual(before, {path: path.read_bytes() for path in self.directory.rglob("*") if path.is_file()})

    def test_failure_before_start_can_recover_with_registered_current_tools(self):
        with self.patches():
            self.seed()
            with self.generation_patches():
                with patch.object(process, "predecessor_status", side_effect=ValueError("unknown previous")), self.assertRaises(ValueError):
                    self.execute("restart")
                with patch.object(process, "stop", side_effect=AssertionError("must not invoke old tool")):
                    self.assertEqual(self.execute("recover")["status"], "cache_stopped")

    def test_rewritten_start_cannot_detach_death_proof_from_published_result(self):
        with self.patches():
            self.seed()
            with self.generation_patches():
                self.execute("restart")
                output = self.directory / cache.STAGE / "a0003"
                proof_path = output / "predecessor-0001.json"
                proof = read_json(proof_path)
                proof["observation"]["extra"] = "changed"
                proof_path.write_text(json.dumps(proof), encoding="utf-8")
                started = read_json(output / "start.json")
                started["predecessors"] = [binding(proof_path)]
                (output / "start.json").write_text(json.dumps(started), encoding="utf-8")
                with self.assertRaises(ValueError):
                    cache.cache_status(self.backend, self.directory)
                with self.assertRaises(ValueError):
                    cache.registered_cache_binding(self.backend, self.directory)
