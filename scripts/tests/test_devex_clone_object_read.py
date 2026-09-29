"""对象读取阶段复用与源 HEAD 分批；离线功能检查，不声明真实协议或性能达标。"""
import copy
import json
import subprocess
import threading
import unittest
from unittest.mock import patch

import devex_clone_capture as capture
import devex_clone_export_verify as export_verify
from devex_clone_live_object import ObjectObservationError, observe_target_object
from devex_clone_source import export_source
from devex_clone_source_fixture import SourceFixture
from restore_build import file_digest
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import ExternalTools
from test_devex_clone_capture import ObjectReads


class SharedCaptureStagesTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.store = ObjectReads()
        self.body = self.work / "registered.bin"
        self.body.write_bytes(self.store.body)
        self.expected = file_digest(self.body)
        self.tools = ExternalTools(self.plan, self.work, self.store)
        self.output = self.work / "observed"
        self.env = patch.dict("os.environ", {"TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def observe(self):
        return observe_target_object(self.backend, self.tools, "uploads", "target/system/file.bin", self.output,
                                     expected=self.expected, max_bytes=1024)

    def test_same_capture_schema_and_original_stage_bytes_with_five_actual_calls(self):
        direct = capture.capture_object(self.tools, "target", "uploads", "target/system/file.bin", self.work / "direct",
                                        expected=self.expected, max_bytes=1024, owner_buckets=("uploads",))
        self.store.calls.clear()
        self.store.heads = 0
        observed = self.observe()
        self.assertEqual(len(self.store.calls), 5)
        self.assertEqual(capture.read_json(observed.capture_directory / "capture.json"), direct)
        verified = capture.verify_capture(self.tools, "target", "uploads", "target/system/file.bin",
                                          observed.capture_directory, expected=self.expected, max_bytes=1024, owner_buckets=("uploads",))
        self.assertEqual(verified["capture"], direct)
        self.assertEqual(len(self.store.calls), 5, "保存证据的复核不能新增远端读取")
        outer = capture.read_json(self.output / "observation.json")
        self.assertEqual(outer["ownership_before"], direct["ownership_before"])
        self.assertEqual(outer["ownership_after"], direct["ownership_after"])
        for name in outer["evidence"]:
            if name != "intent.json":
                self.assertEqual(file_digest(self.output / name), file_digest(observed.capture_directory / name))

    def test_no_external_saved_json_can_complete_without_actual_stage_reads(self):
        self.output.mkdir()
        stages = capture.CaptureStages(capture.CaptureReader(self.tools, "target", self.output),
                                       "uploads", "target/system/file.bin", self.expected, 1024)
        capture.write_json(self.output / "head-before.json", self.store.head)
        with self.assertRaisesRegex(ValueError, "本次真实 HEAD"):
            stages.complete(self.output / "capture")
        self.assertEqual(self.store.calls, [])

    def test_stage_completion_is_single_use_and_preserves_original_head_identity(self):
        self.output.mkdir()
        stages = capture.CaptureStages(capture.CaptureReader(self.tools, "target", self.output),
                                       "uploads", "target/system/file.bin", self.expected, 1024)
        stages.owners_before()
        stages.head_before()
        stages.complete(self.output / "capture")
        with self.assertRaises(ValueError):
            stages.complete(self.output / "another")
        self.assertEqual(len(self.store.calls), 13)

    def test_changed_initial_evidence_is_rejected_before_get_and_retained(self):
        original = capture.CaptureStages.complete

        def changed(stages, output):
            (stages.reader.output / "head-before.json").write_text("{}", encoding="utf-8")
            return original(stages, output)

        with patch.object(capture.CaptureStages, "complete", changed), self.assertRaises(ObjectObservationError):
            self.observe()
        self.assertEqual(len(self.store.calls), 2)
        self.assertTrue((self.output / "failure.json").is_file())
        self.assertTrue((self.output / "capture/failure.json").is_file())
        self.assertEqual((self.output / "head-before.json").read_text(), "{}")

    def test_mutation_during_download_cannot_publish_or_adopt_initial_evidence(self):
        def changed(command, **kwargs):
            result = self.store(command, **kwargs)
            if command[command.index("s3api") + 1] == "get-object" and not command[command.index("--key") + 1].endswith("/.ryframe-owner"):
                (self.output / "owner-before-uploads.diagnostic.json").write_text("{}", encoding="utf-8")
            return result

        self.tools.run = changed
        with self.assertRaises(ObjectObservationError):
            self.observe()
        self.assertEqual(len(self.store.calls), 5)
        self.assertTrue((self.output / "capture/failure.json").is_file())
        self.assertFalse((self.output / "observation.json").exists())

    def test_unknown_metadata_and_after_identity_still_fail_with_original_evidence(self):
        for name, modify in (("metadata", lambda: self.store.head.update(ObjectLockMode="GOVERNANCE")),
                             ("identity", lambda: self.store.after.update(ETag='"changed"')),
                             ("owner", lambda: None)):
            with self.subTest(name=name):
                self.output = self.work / name
                self.store = ObjectReads()
                self.tools.run = self.store
                modify()
                if name == "owner":
                    def changed(command, **kwargs):
                        result = self.store(command, **kwargs)
                        if command[command.index("s3api") + 1] == "get-object" and not command[command.index("--key") + 1].endswith("/.ryframe-owner"):
                            self.store.owner_failure = "uploads"
                        return result
                    self.tools.run = changed
                with self.assertRaises(ObjectObservationError):
                    self.observe()
                self.assertTrue((self.output / "capture/head-before.json").is_file())
                self.assertTrue((self.output / "capture/failure.json").is_file())
                self.assertFalse((self.output / "observation.json").exists())


class ConcurrentSourceHeadTests(unittest.TestCase):
    def setUp(self):
        self.f = SourceFixture(self)
        original = self.f.inventory

        def inventory(command):
            result = original(command)
            entries = next(row["entries"] for row in result["objects"] if row["bucket"] == "uploads")
            template = entries[0]
            entries.extend({**template, "key": self.f.scope + f"/system/file-{index}.txt"} for index in range(8))
            return result

        self.f.inventory = inventory
        export_source(self.f.backend, self.f.request_path, self.f.output, self.f)
        bound = {"path": str(self.f.output / "export.json"), **file_digest(self.f.output / "export.json")}
        self.verified = export_verify.verify_source_export(self.f.backend, bound)
        self.environment = dict(self.f.environment)
        self.tools = ExternalTools({"source": copy.deepcopy(self.f.request["source"]), "tools": self.f.request["tools"]},
                                   self.f.local, self.read)
        self.output = self.f.local / "head-observation"
        self.keys = sorted(self.verified["captures"]["uploads"])
        self.events, self.started, self.finished = [], [], []
        self.lock, self.on_head = threading.Lock(), None

    def read(self, command, **kwargs):
        operation = command[command.index("s3api") + 1]
        bucket = command[command.index("--bucket") + 1]
        if operation == "list-objects-v2":
            with self.lock:
                self.events.append("list")
            keys = [self.f.scope + "/.ryframe-owner"] + (self.keys if bucket == "uploads" else [])
            return subprocess.CompletedProcess(command, 0, json.dumps({"IsTruncated": False, "Contents": [{"Key": key} for key in keys]}).encode(), b"")
        key = command[command.index("--key") + 1]
        if operation == "get-object":
            self.assertTrue(key.endswith("/.ryframe-owner"))
            with self.lock:
                self.events.append("owner")
            return self.f.aws(command)
        self.assertEqual(operation, "head-object")
        index = self.keys.index(key)
        with self.lock:
            self.events.append("head")
            self.started.append(index)
        try:
            if self.on_head:
                self.on_head(index)
            return self.f.aws(command)
        finally:
            with self.lock:
                self.finished.append(index)

    def observe(self):
        return export_verify.observe_source_objects(self.f.backend, self.tools, self.verified, self.output,
                                                    environment=self.environment)

    def test_source_head_batches_keep_complete_listing_owner_barriers_and_sorted_results(self):
        rendezvous = threading.Barrier(4)
        self.on_head = lambda index: rendezvous.wait(timeout=10) if index < 8 else None
        result = self.observe()
        self.assertEqual([item["key"] for item in result["observed"]], self.keys)
        self.assertEqual(self.events[:10], ["owner"] * 5 + ["list"] * 5)
        self.assertEqual(self.events[10:19], ["head"] * 9)
        self.assertEqual(self.events[19:], ["list"] * 5 + ["owner"] * 5)
        self.assertEqual(len(list(self.output.glob("head-*.json"))), 18)
        self.assertEqual(len(list(self.output.glob("head-*.diagnostic.json"))), 9)
        self.assertFalse(result["source_body_sha_recomputed"])

    def test_changed_proof_during_parallel_heads_fails_after_all_reads_before_publication(self):
        rendezvous = threading.Barrier(4)
        path = self.f.output / "schema-after.json"

        def changed(index):
            if index < 4:
                rendezvous.wait(timeout=10)
                if index == 0:
                    path.write_text("{}", encoding="utf-8")

        self.on_head = changed
        with self.assertRaises(export_verify.SourceObjectObservationError):
            self.observe()
        self.assertCountEqual(self.finished, range(9))
        self.assertTrue((self.output / "failure.json").is_file())
        self.assertFalse((self.output / "observation.json").exists())


if __name__ == "__main__":
    unittest.main()
