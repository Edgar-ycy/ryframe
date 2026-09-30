"""不可变 post-copy amendment 与活动登记代次；全部使用离线文件和 mock。"""
import copy
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_post as post
import devex_clone_post_process as process
import devex_clone_run as run
import devex_clone_seed as seed
import test_devex_clone_post as fixtures
from devex_clone_capture import read_json, write_json
from process_environment import Environments
from devex_clone_run_state import begin, bind_controller_attempt, binding, finish, load_state, run_lock


class AmendmentTests(unittest.TestCase):
    write = fixtures.PostTests.write
    file = fixtures.PostTests.file
    register = fixtures.PostTests.register

    def setUp(self):
        fixtures.PostTests.setUp(self)
        self.old_api = {**self.api, "APP_CORS_ALLOW_ORIGINS": '["http://127.0.0.1:4190"]'}
        self.request["api_environment"] = self.file("old-api", {"environment": self.old_api})
        request = self.file("old-request", self.request)
        number = begin(self.directory, "post-copy", "register", {})
        self.register_attempt = number
        with patch.object(post, "validate_registration"):
            result = post.register(self.backend, self.directory, self.value, Path(request["path"]))
        finish(self.directory, number, result=result)
        self.base = binding(self.directory / "post-copy.json")
        self.runtime = self.directory / "post-copy/runtime"
        self.runtime.mkdir(parents=True)
        self.write(self.runtime / "runtime.json", {
            "scope_id": "target-one", "worker_ready_url": "http://127.0.0.1:18211/readyz"})
        self.write(self.runtime / "binaries.json", {"ryframe": "fixture-api", "ryframe-worker": "fixture-worker"})
        self.handoff = {"registration": self.base, "runtime": binding(self.runtime / "runtime.json"),
            "binaries": binding(self.runtime / "binaries.json"), "api_url": self.selected["api_url"],
            "worker_started": False}
        self.write(self.runtime / "handoff.json", self.handoff)
        self.prepare = self.stage("post-copy", "prepare", {
            "status": "post_copy_api_prepared", "handoff": binding(self.runtime / "handoff.json"),
            "worker_started": False})
        self.identity = {"pid": 2147482001, "started": "fixture-start", "executable": "fixture-api"}
        self.write(self.runtime / "api.json", {
            "format_version": 1, "role": "api", "scope_id": "target-one", "identity": self.identity})
        self.write(self.runtime / "producer-history.json", {
            "format_version": 1, "kind": "devex-clone-producer-history", "scope_id": "target-one",
            "runtime_directory": str(self.runtime), "events": [
                {"sequence": 0, "event": "start-intent", "role": "api", "token": "fixture-token"},
                {"sequence": 1, "event": "started", "role": "api", "token": "fixture-token",
                 "identity": self.identity},
                {"sequence": 2, "event": "start-failed", "role": "api", "token": "fixture-token",
                 "error_type": "RuntimeError"}]})
        self.failed = begin(self.directory, "runtime-target", "start", {})
        write_json(self.directory / f"controller-{self.failed:04d}.json", {"fixture": "controller"})
        write_json(self.directory / f"failure-{self.failed:04d}.json", {
            "format_version": 1, "kind": "devex-stage-failure", "attempt": self.failed,
            "stage": "runtime-target", "mode": "start", "error_type": "RuntimeError",
            "frames": [{"file": "tools/python/devex_clone_runtime.py", "function": "_start_one", "line": 1}],
            "controller": binding(self.directory / f"controller-{self.failed:04d}.json")})
        finish(self.directory, self.failed, error=RuntimeError("fixture config rejected"))
        self.stopped = begin(self.directory, "runtime-target", "stop", {})
        stop_result = {"format_version": 1, "kind": "devex-clone-runtime-control", "operation": "stop",
            "runtime_directory": str(self.runtime), "scope_id": "target-one",
            "processes": {"api": {"state": "stopped", "identity": None, "ready": False}},
            "producer_history": str(self.runtime / "producer-history.json")}
        finish(self.directory, self.stopped, result=stop_result)
        self.corrected = copy.deepcopy(self.request)
        self.corrected["api_environment"] = self.file("corrected-api", {"environment": self.api})

    def stage(self, stage, mode, result):
        number = begin(self.directory, stage, mode, {})
        finish(self.directory, number, result=result)
        return load_state(self.directory)["attempts"][-1]["result"]

    def publish(self):
        candidate = self.file("corrected-request", self.corrected)
        number = begin(self.directory, "post-copy", "amend", {})
        with patch("full_stack_process.process_identity", return_value=None), \
                patch.object(post, "require_closed_port"):
            result = post.amend(self.backend, self.directory, self.value, Path(candidate["path"]), number)
        finish(self.directory, number, result=result)
        return post.registration(self.backend, self.directory)

    def new_prepare(self, active):
        runtime = active.runtime
        runtime.mkdir(parents=True)
        self.write(runtime / "runtime.json", {"scope_id": "target-one"})
        self.write(runtime / "binaries.json", {"ryframe": "fixture-api"})
        handoff = {"registration": active.descriptor, "runtime": binding(runtime / "runtime.json"),
            "binaries": binding(runtime / "binaries.json"), "api_url": self.selected["api_url"],
            "worker_started": False}
        self.write(runtime / "handoff.json", handoff)
        return self.stage("post-copy", "prepare", {
            "status": "post_copy_api_prepared", "handoff": binding(runtime / "handoff.json"),
            "worker_started": False})

    def test_amend_preserves_history_and_selects_raw_cors_runtime_generation(self):
        paths = [self.directory / "post-copy.json", self.runtime / "runtime.json", self.runtime / "binaries.json",
                 self.runtime / "handoff.json", self.runtime / "api.json", self.runtime / "producer-history.json",
                 self.directory / "results" / f"{self.register_attempt:04d}.json", Path(self.prepare["path"]),
                 self.directory / f"controller-{self.failed:04d}.json",
                 self.directory / f"failure-{self.failed:04d}.json",
                 self.directory / "results" / f"{self.stopped:04d}.json"]
        before = {path: path.read_bytes() for path in paths}
        active = self.publish()
        self.assertEqual(active.sequence, 1)
        self.assertEqual(active.runtime, self.directory / "post-copy/runtime-0001")
        self.assertEqual(active.request, self.corrected)
        self.assertEqual(active.request["api_environment"], self.corrected["api_environment"])
        self.assertEqual(read_json(Path(active.request["api_environment"]["path"]))["environment"]
                         ["APP_CORS_ALLOW_ORIGINS"], "http://127.0.0.1:4190")
        self.assertEqual({path: path.read_bytes() for path in paths}, before)
        self.assertFalse((self.directory / "post-copy/current.json").exists())
        amendment = read_json(Path(active.descriptor["path"]))
        self.assertNotIn("request", amendment)
        self.assertEqual(amendment["predecessor"], self.base)

    def test_only_complete_request_and_single_raw_cors_change_are_accepted(self):
        cases = []
        bad_cors = {**self.api, "APP_CORS_ALLOW_ORIGINS": '["http://127.0.0.1:4190"]'}
        cases.append({**self.corrected, "api_environment": self.file("bad-cors", {"environment": bad_cors})})
        cases.append({**self.corrected, "unexpected": True})
        cases.append({**self.corrected, "source_admin": {**self.admin, "subject_id": "10"}})
        changed = {**self.api, "APP_ENV": "other"}
        cases.append({**self.corrected, "api_environment": self.file("bad-other-env", {"environment": changed})})
        for index, candidate in enumerate(cases):
            path = self.file(f"rejected-{index}", candidate)
            with self.subTest(index=index), patch("full_stack_process.process_identity", return_value=None), \
                    patch.object(post, "require_closed_port"), self.assertRaises(ValueError):
                post.amend(self.backend, self.directory, self.value, Path(path["path"]), 999)
        self.assertFalse((self.directory / "post-copy/amendments").exists())

    def test_live_api_open_port_or_worker_receipt_blocks_amendment(self):
        candidate = self.file("corrected-request", self.corrected)
        with patch("full_stack_process.process_identity", return_value=self.identity), \
                patch.object(post, "require_closed_port"), self.assertRaises(ValueError):
            post.amend(self.backend, self.directory, self.value, Path(candidate["path"]), 999)
        with patch("full_stack_process.process_identity", return_value=None), \
                patch.object(post, "require_closed_port", side_effect=ValueError("port open")), \
                self.assertRaises(ValueError):
            post.amend(self.backend, self.directory, self.value, Path(candidate["path"]), 999)
        self.write(self.runtime / "worker.json", {"unexpected": True})
        with patch("full_stack_process.process_identity", return_value=None), \
                patch.object(post, "require_closed_port"), self.assertRaises(ValueError):
            post.amend(self.backend, self.directory, self.value, Path(candidate["path"]), 999)

    def test_missing_or_changed_evidence_invalidates_published_amendment(self):
        active = self.publish()
        path = self.directory / f"controller-{self.failed:04d}.json"
        saved = path.read_bytes()
        path.unlink()
        with self.assertRaises((FileNotFoundError, ValueError)):
            post.registration(self.backend, self.directory)
        self.assertEqual(post.registration(self.backend, self.directory,
                         descriptor=self.base, cleanup=True).request, self.request)
        self.assertEqual(post.registration(self.backend, self.directory,
                         descriptor=active.descriptor, cleanup=True).request, self.corrected)
        path.write_bytes(saved)
        amendment = Path(active.descriptor["path"])
        saved = amendment.read_bytes()
        value = json.loads(saved)
        value["invalidated_runtime"]["stopped"]["attempt"] += 1
        amendment.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(ValueError):
            post.registration(self.backend, self.directory)
        amendment.write_bytes(saved)
        self.assertEqual(post.registration(self.backend, self.directory).descriptor, active.descriptor)

    def test_failed_latest_amendment_blocks_writes_but_historical_cleanup_resolves(self):
        active = self.publish()
        number = begin(self.directory, "post-copy", "amend", {})
        write_json(self.directory / "post-copy/amendments/0002.json", {"unpublished": True})
        finish(self.directory, number, result={"status": "unpublished_amendment"},
               error=ValueError("outer source final check failed"))
        with self.assertRaises(ValueError):
            post.registered(self.backend, self.directory)
        self.assertEqual(post.registration(self.backend, self.directory,
                         descriptor=active.descriptor, cleanup=True).request, self.corrected)
        self.assertEqual(process.historical_producer_request(
                         self.backend, self.directory, "post-copy", self.base), self.request)

    def test_old_prepare_cannot_start_amended_runtime(self):
        self.publish()
        with patch("devex_clone_runtime.control") as control, self.assertRaises(ValueError):
            run.run_runtime(self.backend, self.value, Environments({}, {}), "target", "start", ("api",),
                            run_directory=self.directory)
        control.assert_not_called()

    def test_new_prepare_and_seed_bind_active_registration_and_runtime(self):
        active = self.publish()
        self.new_prepare(active)
        runtime, environment = post.runtime_inputs(self.backend, self.directory, descriptor=active.descriptor)
        self.assertEqual(runtime["runtime_dir"], str(active.runtime))
        self.assertEqual(environment, self.api)
        verified = self.stage("post-copy", "verify", {"status": "post_copy_existing_data_verified",
            "registration": active.descriptor, "worker_must_remain_stopped": True})
        generic = self.file("seed-generic", {"fixture": True})
        request = {field: generic for field in seed.FIELDS}
        request.update(format_version=1, kind="devex-clone-seed-runtime",
            run_manifest=binding(self.directory / "manifest.json"), post_copy=active.descriptor,
            post_verify=verified, node=self.corrected["node"])
        self.assertEqual(seed.history(self.backend, self.directory, request), self.corrected)
        with self.assertRaises(ValueError):
            seed.history(self.backend, self.directory, {**request, "post_copy": self.base})

    def test_node_producer_accepts_active_descriptor_and_rejects_base(self):
        active = self.publish()
        with run_lock(self.directory) as owner:
            number = begin(self.directory, "post-copy", "verify", {})
            bind_controller_attempt(self.directory, number, owner)
            output = self.directory / "post-copy" / f"attempt-{number:04d}"
            output.mkdir()
            common = {"backend": self.backend, "directory_root": self.directory, "output": output,
                      "private": {"target_api": self.api}}
            old = SimpleNamespace(**common, request=self.request, request_binding=self.base)
            with patch.object(process, "producer_command") as command, self.assertRaises(ValueError):
                process.Producer(old, "existing")
            command.assert_not_called()
            context = SimpleNamespace(**common, request=self.corrected, request_binding=active.descriptor)
            with patch.object(process, "producer_command", side_effect=RuntimeError("active reached command")), \
                    self.assertRaisesRegex(RuntimeError, "active reached command"):
                process.Producer(context, "existing")


if __name__ == "__main__":
    unittest.main()
