"""并发捕获不改变单对象证明、稳定顺序和失败收口。"""
import hashlib
from pathlib import Path
import sys
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_object_batch as batch


class ObjectBatchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.tools = SimpleNamespace(work=Path(self.tmp.name))
        self.inventory = {"objects": [
            {"bucket": "uploads", "prefix": "source/", "entries": [
                {"key": f"source/{n:02d}", "bytes": 4, "sha256": hashlib.sha256(b"data").hexdigest()}
                for n in reversed(range(9))]},
            {"bucket": "avatar", "prefix": "source/", "entries": []}]}

    def test_keeps_all_single_object_proofs_and_stable_order(self):
        guard = threading.Lock()
        active = peak = 0
        started, verified = [], []
        rendezvous = threading.Barrier(4)

        def capture(tools, side, bucket, key, output, **kwargs):
            nonlocal active, peak
            with guard:
                active += 1
                peak = max(peak, active)
                started.append((side, bucket, key, kwargs))
            if key != "source/08":
                rendezvous.wait(timeout=5)
            output.mkdir()
            (output / "object.bin").write_bytes(b"data")
            (output / "capture.json").write_text("{}", encoding="utf-8")
            with guard:
                active -= 1
            return {"metadata": {"key": key}, "get_header_consistent": True}

        def verify(tools, side, bucket, key, output, **kwargs):
            self.assertEqual((output / "object.bin").read_bytes(), b"data")
            with guard:
                verified.append((side, bucket, key, kwargs))

        with patch.object(batch, "capture_object", capture), patch.object(batch, "verify_capture", verify):
            buckets, index = batch.capture_objects(self.tools, {"max_object_bytes": 4}, self.inventory)
        self.assertEqual(peak, 4)
        self.assertCountEqual(started, verified)
        self.assertEqual(len(verified), 9)
        self.assertEqual(buckets[0], {"bucket": "avatar", "entries": []})
        self.assertEqual([item["key"] for item in buckets[1]["entries"]], [f"source/{n:02d}" for n in range(9)])
        self.assertEqual(set(index), {("uploads", f"{n:02d}") for n in range(9)})
        self.assertTrue(all(item["get_header_consistent"] for item in buckets[1]["entries"]))

    def test_failed_batch_joins_active_workers_and_never_starts_next_batch(self):
        rendezvous = threading.Barrier(4)
        guard = threading.Lock()
        called, finished = [], []

        def capture(tools, request, bucket, item):
            key = item["key"]
            with guard:
                called.append(key)
            rendezvous.wait(timeout=5)
            try:
                if key == "source/00":
                    raise ValueError("保留原始证明失败")
                return {}, {}
            finally:
                with guard:
                    finished.append(key)

        with patch.object(batch, "capture_one", capture), self.assertRaisesRegex(ValueError, "保留原始证明失败"):
            batch.capture_objects(self.tools, {"max_object_bytes": 4}, self.inventory)
        self.assertCountEqual(called, [f"source/{n:02d}" for n in range(4)])
        self.assertCountEqual(finished, called)

    def test_offline_verification_failure_is_not_a_completed_capture(self):
        def capture(tools, side, bucket, key, output, **kwargs):
            output.mkdir()
            (output / "capture.json").write_text("{}", encoding="utf-8")
            return {"metadata": {}, "get_header_consistent": True}

        inventory = {"objects": [{**self.inventory["objects"][0], "entries": self.inventory["objects"][0]["entries"][:1]}]}
        with (patch.object(batch, "capture_object", capture),
              patch.object(batch, "verify_capture", side_effect=ValueError("原始证据损坏")),
              self.assertRaisesRegex(ValueError, "原始证据损坏")):
            batch.capture_objects(self.tools, {"max_object_bytes": 4}, inventory)


if __name__ == "__main__":
    unittest.main()
