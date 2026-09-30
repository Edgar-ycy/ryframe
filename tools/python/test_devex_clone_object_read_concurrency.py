"""目标 HEAD 使用真实线程与同步闸门；仅替身对象工具，无服务或性能测量。"""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_live_object as live
from devex_clone_capture import inspect_response
from devex_clone_transfer import ObjectObservation
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import ExternalTools
from restore_reference_plan import plan_hash
from test_devex_clone_capture import ObjectReads


class ConcurrentTargetHeadTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.output = self.work / "target-heads"
        self.owner = ObjectReads()
        self.head = copy.deepcopy(self.owner.head)
        identity = inspect_response(self.head)
        self.capture = {**identity, "get_header_differences": [],
                        "response_sha256": {"head_before": plan_hash(self.head)}}
        self.items, self.observations = [], {}
        self.lock = threading.Lock()
        self.active = self.peak = 0
        self.started, self.finished, self.environments, self.verified = [], [], [], []
        self.on_head = None
        expected = {"bytes": len(self.owner.body), "sha256": hashlib.sha256(self.owner.body).hexdigest()}
        for index in range(9):
            key = f"target/system/{index}.bin"
            directory = self.work / f"capture-{index}"
            directory.mkdir()
            resource = {"kind": "object", "scope_id": "target", "endpoint": self.plan["target"]["s3"]["endpoint"],
                        "bucket": "uploads", "key": key}
            self.items.append({"bucket": "uploads", "target_key": key, "artifact": expected,
                               "metadata": identity["metadata"]})
            self.observations["uploads", key] = ObjectObservation(resource, directory, None)
        self.tools = ExternalTools(self.plan, self.work, self.read)
        self.environment = patch.dict("os.environ", {"TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def read(self, command, **kwargs):
        key = command[command.index("--key") + 1]
        if key.endswith("/.ryframe-owner"):
            return self.owner(command, **kwargs)
        self.assertEqual(command[command.index("s3api") + 1], "head-object")
        self.assertEqual(command[command.index("--if-match") + 1], self.head["ETag"])
        self.assertNotIn("--version-id", command)
        index = int(key.rsplit("/", 1)[1].removesuffix(".bin"))
        with self.lock:
            self.active += 1
            self.peak = max(self.peak, self.active)
            self.started.append(index)
            self.environments.append(dict(kwargs["env"]))
        try:
            if self.on_head:
                self.on_head(index)
            return subprocess.CompletedProcess(command, 0, json.dumps(self.head).encode(), b"")
        finally:
            with self.lock:
                self.active -= 1
                self.finished.append(index)

    def verify(self, tools, side, bucket, key, directory, **kwargs):
        self.assertEqual(side, "target")
        self.assertEqual(directory, self.observations[bucket, key].capture_directory)
        self.assertEqual(kwargs["expected"], self.items[int(key.rsplit("/", 1)[1].removesuffix(".bin"))]["artifact"])
        with self.lock:
            self.verified.append(key)
        return {"capture": copy.deepcopy(self.capture)}

    def execute(self):
        return live.verify_target_heads(self.backend, self.tools, self.items, self.observations, self.output)

    def controller(self):
        done, outcome = threading.Event(), []

        def run():
            try:
                outcome.append(self.execute())
            except BaseException as error:
                outcome.append(error)
            finally:
                done.set()

        thread = threading.Thread(target=run, name="head-test-controller")
        thread.start()
        return thread, done, outcome

    def test_four_way_head_batches_keep_unique_evidence_and_stable_result_order(self):
        barrier = threading.Barrier(4)
        self.on_head = lambda index: barrier.wait(timeout=10) if index < 8 else None
        with patch.object(live, "verify_capture", self.verify):
            result = self.execute()
        self.assertEqual(self.peak, 4)
        self.assertEqual(self.active, 0)
        self.assertCountEqual(self.started, range(9))
        self.assertCountEqual(self.finished, range(9))
        self.assertCountEqual(self.verified, [item["target_key"] for item in self.items])
        self.assertEqual([item["resource"]["key"] for item in result["objects"]], [item["target_key"] for item in self.items])
        self.assertEqual({path.name for path in self.output.glob("head-*.json")}, {
            f"head-{index}{suffix}.json" for index in range(9) for suffix in ("", ".diagnostic")})
        self.assertEqual(len(result["ownership_before"]), 5)
        self.assertEqual(len(result["ownership_after"]), 5)
        self.assertTrue((self.output / "verified.json").is_file())
        self.assertEqual(result["business_downloads"], 0)
        self.assertEqual(result["remote_writes"], 0)

    def test_failed_first_batch_waits_for_inflight_heads_and_never_starts_next_batch(self):
        barrier = threading.Barrier(4)
        release, failure = threading.Event(), threading.Event()

        def hold(index):
            barrier.wait(timeout=10)
            if index == 0:
                failure.set()
                raise subprocess.CalledProcessError(254, ["aws"], stderr=b"AccessDenied")
            if not release.wait(timeout=10):
                raise AssertionError("测试未释放在途HEAD")

        self.on_head = hold
        with patch.object(live, "verify_capture", self.verify):
            thread, done, outcome = self.controller()
            try:
                self.assertTrue(failure.wait(timeout=10))
                self.assertFalse(done.wait(timeout=0.1))
                with self.lock:
                    self.assertCountEqual(self.started, range(4))
            finally:
                release.set()
                thread.join(timeout=10)
                barrier.abort()
        self.assertFalse(thread.is_alive())
        self.assertIsInstance(outcome[0], subprocess.CalledProcessError)
        self.assertCountEqual(self.started, range(4))
        self.assertCountEqual(self.finished, range(4))
        self.assertEqual(self.active, 0)
        self.assertFalse((self.output / "verified.json").exists())
        self.assertTrue((self.output / "failure.json").is_file())

    def test_frozen_environment_drift_rejects_after_join_without_mixing_command_credentials(self):
        barrier = threading.Barrier(5)
        release = threading.Event()

        def hold(_index):
            barrier.wait(timeout=10)
            if not release.wait(timeout=10):
                raise AssertionError("测试未释放在途HEAD")

        self.on_head = hold
        with patch.object(live, "verify_capture", self.verify):
            thread, done, outcome = self.controller()
            try:
                barrier.wait(timeout=10)
                os.environ["TEST_SECRET"] = "changed-fixture"
                self.assertFalse(done.is_set())
            finally:
                release.set()
                thread.join(timeout=10)
                barrier.abort()
        self.assertFalse(thread.is_alive())
        self.assertIsInstance(outcome[0], ValueError)
        self.assertCountEqual(self.started, range(4))
        self.assertCountEqual(self.finished, range(4))
        self.assertTrue(all(value["AWS_SECRET_ACCESS_KEY"] == "secret-fixture" for value in self.environments))
        self.assertEqual(self.active, 0)
        self.assertFalse((self.output / "verified.json").exists())
        self.assertTrue((self.output / "failure.json").is_file())


if __name__ == "__main__":
    unittest.main()
