"""父控制器串行切换来源环境，批内四路只写独立读取证据。"""
import os
from pathlib import Path
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import devex_clone_factory as factory
from process_environment import Environments
from devex_clone_transfer import ObjectObservation


class FactoryObjectBatchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.work = self.root / ".local-tests/batch"
        self.work.mkdir(parents=True)
        self.lock = threading.Lock()
        self.started, self.finished, self.paths, self.proofs = [], [], [], []
        self.active = self.peak = 0
        self.source_hook = self.target_hook = None
        self.original_environment = dict(os.environ)
        self.targets = [("uploads", f"target/system/{index}.bin") for index in range(4)]
        self.parent = None

    def session(self):
        session = factory.CloneSession.__new__(factory.CloneSession)
        session.backend, session.output = self.root, self.work
        session.plan = {"objects": [{"bucket": bucket, "source_key": key.replace("target/", "source/", 1),
                                    "target_key": key, "artifact": {"bytes": 4, "sha256": "a" * 64}}
                                   for bucket, key in self.targets]}
        session.tools = SimpleNamespace(work=self.work)
        session.source_verified = {"fixture": "fixed source proof"}
        session.environment = Environments({"CLONE_SIDE": "source", "READ_SECRET": "source-fixture"},
                                           {"CLONE_SIDE": "target", "READ_SECRET": "target-fixture"})
        session.latest_objects = {}
        return session

    def proof(self, _backend, _verified, *, selection):
        self.assertEqual(threading.get_ident(), self.parent)
        self.assertEqual(os.environ["CLONE_SIDE"], "source")
        self.proofs.append(selection)

    def read(self, side, key, output, expected_environment=None):
        self.assertNotEqual(threading.get_ident(), self.parent)
        self.assertEqual(os.environ["CLONE_SIDE"], side)
        self.assertEqual(os.environ["READ_SECRET"], side + "-fixture")
        if expected_environment is not None:
            self.assertEqual(dict(expected_environment), dict(os.environ))
        with self.lock:
            if side == "target":
                self.assertEqual(sum(value[0] == "source" for value in self.finished), 4)
            self.active += 1
            self.peak = max(self.peak, self.active)
            self.started.append((side, key))
            self.paths.append(output)
        try:
            hook = self.source_hook if side == "source" else self.target_hook
            if hook:
                hook(key)
            output.mkdir()
            (output / "proof.json").write_text('{"fixture":"read only"}', encoding="utf-8")
        finally:
            with self.lock:
                self.active -= 1
                self.finished.append((side, key))

    def source(self, _backend, _tools, _verified, _bucket, key, output, *, environment):
        self.read("source", key, output, environment)

    def target(self, _backend, _tools, bucket, key, output, *, expected, max_bytes):
        self.assertEqual(expected, {"bytes": 4, "sha256": "a" * 64})
        self.assertEqual(max_bytes, 4)
        self.read("target", key, output)
        return ObjectObservation({"bucket": bucket, "key": key}, None, "a" * 64)

    def execute(self):
        self.parent = threading.get_ident()
        session = self.session()
        result = session.observe_objects(self.targets)
        self.assertEqual(session.latest_objects, dict(zip(self.targets, result, strict=True)))
        return result

    def patches(self):
        return (patch.object(factory, "verify_export_bindings", self.proof),
                patch.object(factory, "observe_source_object", self.source),
                patch.object(factory, "observe_target_object", self.target))

    def test_source_batch_fully_exits_before_target_environment_and_unique_evidence(self):
        barrier = threading.Barrier(4)
        self.source_hook = self.target_hook = lambda _: barrier.wait(timeout=10)
        source, target, proof = self.patches()
        with source, target, proof:
            result = self.execute()
        self.assertEqual([item.resource["key"] for item in result], [key for _, key in self.targets])
        self.assertEqual(self.peak, 4)
        self.assertEqual(self.active, 0)
        self.assertEqual(len(set(self.paths)), 8)
        self.assertEqual(len(self.proofs), 12)
        self.assertEqual(dict(os.environ), self.original_environment)

    def test_source_failure_waits_all_inflight_before_restoring_environment_and_never_reads_target(self):
        barrier = threading.Barrier(4)
        release, failed, done = threading.Event(), threading.Event(), threading.Event()
        outcome = []

        def hold(key):
            barrier.wait(timeout=10)
            if key.endswith("/0.bin"):
                failed.set()
                raise ValueError("source-read-failed")
            if not release.wait(timeout=10):
                raise AssertionError("测试未释放source批")

        def controller():
            try:
                outcome.append(self.execute())
            except BaseException as error:
                outcome.append(error)
            finally:
                done.set()

        self.source_hook = hold
        source, target, proof = self.patches()
        with source, target, proof:
            thread = threading.Thread(target=controller)
            thread.start()
            try:
                self.assertTrue(failed.wait(timeout=10))
                self.assertFalse(done.wait(timeout=0.1))
                self.assertEqual(os.environ["CLONE_SIDE"], "source")
            finally:
                release.set()
                thread.join(timeout=10)
                barrier.abort()
        self.assertFalse(thread.is_alive())
        self.assertIsInstance(outcome[0], ValueError)
        self.assertEqual(len(self.started), 4)
        self.assertCountEqual(self.finished, self.started)
        self.assertTrue(all(side == "source" for side, _ in self.started))
        self.assertEqual(dict(os.environ), self.original_environment)

    def test_post_target_proof_failure_cannot_publish_batch_observations(self):
        count = 0
        original = self.proof

        def proof(*args, **kwargs):
            nonlocal count
            count += 1
            original(*args, **kwargs)
            if count == 9:
                raise ValueError("原始source proof变化")

        self.parent = threading.get_ident()
        session = self.session()
        with patch.object(factory, "verify_export_bindings", proof), \
                patch.object(factory, "observe_source_object", self.source), \
                patch.object(factory, "observe_target_object", self.target):
            with self.assertRaisesRegex(ValueError, "proof变化"):
                session.observe_objects(self.targets)
        self.assertEqual(session.latest_objects, {})
        self.assertEqual(dict(os.environ), self.original_environment)

    def test_duplicate_unregistered_and_oversized_requests_fail_before_any_read(self):
        self.parent = threading.get_ident()
        session = self.session()
        source, target, proof = self.patches()
        with source, target, proof:
            for targets in (self.targets + [self.targets[0]], [self.targets[0]] * 2, [("uploads", "other/key")], []):
                with self.subTest(targets=targets), self.assertRaises(ValueError):
                    session.observe_objects(targets)
        self.assertEqual(self.started, [])
        self.assertEqual(self.proofs, [])


if __name__ == "__main__":
    unittest.main()
