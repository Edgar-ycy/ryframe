import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_capture as capture_module
from devex_clone_capture import CaptureReader, ObjectCaptureError, capture_object, verify_capture
from devex_clone_run_state import binding
from restore_build import file_digest
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import COPY_METADATA_FIELDS, ExternalTools
from restore_reference_plan import BUCKETS


class ObjectReads:
    """纯离线对象读模型，精确记录调用，不模拟真实 RustFS 通过。"""
    def __init__(self):
        self.body = b"fixture-raw-bytes"
        self.head = {"ContentLength": len(self.body), "ETag": '"fixture-etag"',
                     "LastModified": "2026-09-04T00:00:00+00:00", "AcceptRanges": "bytes",
                     "ContentType": "text/plain", "Metadata": {"purpose": "fixture"}, "ContentLanguage": "en"}
        self.downloaded = copy.deepcopy(self.head)
        self.after = copy.deepcopy(self.head)
        self.calls, self.heads = [], 0
        self.owner_failure, self.failure, self.raw = None, None, None

    def __call__(self, command, **kwargs):
        self.calls.append((command, kwargs))
        operation = command[command.index("s3api") + 1]
        bucket, key = command[command.index("--bucket") + 1], command[command.index("--key") + 1]
        if operation not in {"get-object", "head-object"}:
            raise AssertionError("只读采集发出了写请求")
        if key.endswith("/.ryframe-owner"):
            body = f"ryframe-owner:v1:{key.split('/')[0]}:object-storage:{bucket}".encode()
            if self.owner_failure == bucket:
                body = b"different-owner"
            Path(command[-1]).write_bytes(body)
            return subprocess.CompletedProcess(command, 0, stdout=b"{}")
        if self.failure == operation:
            raise subprocess.CalledProcessError(254, command, stderr=(
                "aws: [ERROR]: An error occurred (AccessDenied) when calling the GetObject operation: "
                "access-fixture secret-fixture").encode())
        if operation == "get-object":
            Path(command[-1]).write_bytes(self.body)
            value = self.downloaded
        else:
            self.heads += 1
            value = self.head if self.heads == 1 else self.after
        return subprocess.CompletedProcess(command, 0, stdout=self.raw or json.dumps(value).encode())


class CaptureTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.store = ObjectReads()
        self.original = self.work / "registered.bin"
        self.original.write_bytes(self.store.body)
        self.expected = file_digest(self.original)
        self.output = self.work / "capture"
        self.tools = ExternalTools(self.plan, self.work, self.store)
        self.env = patch.dict("os.environ", {"TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def capture(self, **values):
        return capture_object(self.tools, values.get("side", "source"), values.get("bucket", "uploads"),
                              values.get("key", "source/system/registered.bin"), values.get("output", self.output),
                              expected=values.get("expected", self.expected), max_bytes=values.get("max_bytes", 1024))

    def test_read_bound_json_accepts_exact_descriptor(self):
        path = self.work / "bound-valid.json"
        path.write_text('{"value":1}\n', encoding="utf-8")
        descriptor = {"path": str(path), **file_digest(path)}
        self.assertEqual(capture_module.read_bound_json(path, descriptor), {"value": 1})

    @unittest.skipUnless(os.name == "nt", "需要 Windows 扩展路径语义")
    def test_write_json_preserves_long_chinese_space_path_and_existing_receipt(self):
        root = self.work / "长路径 空格"
        directory = root
        for _ in range(5):
            directory /= "证据目录 " + "x" * 40
        os.makedirs(capture_module.filesystem_path(directory))
        self.addCleanup(shutil.rmtree, capture_module.filesystem_path(root))
        path = directory / "收据 文件.json"
        self.assertGreater(len(str(path)), 260)
        value = {"状态": "已核验", "stage": "tenant-data-dedicated-a-verify"}

        capture_module.write_json(path, value)

        with open(capture_module.filesystem_path(path), "rb") as stream:
            content = stream.read()
        self.assertEqual(json.loads(content), value)
        self.assertTrue(content.endswith(b"\n"))
        self.assertNotIn(b"\r\n", content)
        descriptor = binding(path)
        self.assertEqual(descriptor["path"], str(path))
        self.assertFalse(descriptor["path"].startswith("\\\\?\\"))
        self.assertEqual(descriptor["bytes"], len(content))

        with self.assertRaises(FileExistsError):
            capture_module.write_json(path, {"状态": "不可覆盖"})
        self.assertEqual(binding(path), descriptor)
        with open(capture_module.filesystem_path(path), "rb") as stream:
            self.assertEqual(stream.read(), content)

    def test_read_bound_json_rejects_duplicate_keys(self):
        path = self.work / "bound-duplicate.json"
        path.write_text('{"value":1,"value":2}\n', encoding="utf-8")
        descriptor = {"path": str(path), **file_digest(path)}
        with self.assertRaisesRegex(ValueError, "重复 JSON 字段"):
            capture_module.read_bound_json(path, descriptor)

    def test_read_bound_json_rejects_bytes_and_sha_mismatch(self):
        path = self.work / "bound-mismatch.json"
        path.write_text('{"value":1}\n', encoding="utf-8")
        descriptor = {"path": str(path), **file_digest(path)}
        different_sha = ("0" if descriptor["sha256"][0] != "0" else "1") + descriptor["sha256"][1:]
        for changed in ({**descriptor, "bytes": descriptor["bytes"] + 1},
                        {**descriptor, "sha256": different_sha}):
            with self.subTest(descriptor=changed), self.assertRaisesRegex(ValueError, "身份、字节或摘要变化"):
                capture_module.read_bound_json(path, changed)

    def test_read_bound_json_rejects_fstat_identity_change_during_read(self):
        path = self.work / "bound-changing.json"
        path.write_text('{"value":1}\n', encoding="utf-8")
        descriptor = {"path": str(path), **file_digest(path)}
        actual_fstat = capture_module.os.fstat
        calls = 0

        def changing_fstat(file_descriptor):
            nonlocal calls
            state = actual_fstat(file_descriptor)
            calls += 1
            if calls == 2:
                return SimpleNamespace(st_mode=state.st_mode, st_dev=state.st_dev, st_ino=state.st_ino,
                                       st_size=state.st_size, st_mtime_ns=state.st_mtime_ns + 1)
            return state

        with patch.object(capture_module.os, "fstat", side_effect=changing_fstat), \
                self.assertRaisesRegex(ValueError, "身份、字节或摘要变化"):
            capture_module.read_bound_json(path, descriptor)
        self.assertEqual(calls, 2)

    def failed(self):
        with self.assertRaises(ObjectCaptureError) as caught:
            self.capture()
        self.assertEqual(caught.exception.evidence_directory, str(self.output))
        self.assertFalse((self.output / "capture.json").exists())
        failure = json.loads((self.output / "failure.json").read_text(encoding="utf-8"))
        self.assertEqual(failure["status"], "capture_failed")
        self.assertFalse(failure["clone_verified"])
        return failure

    def business_calls(self):
        return [command for command, _ in self.store.calls if "/.ryframe-owner" not in command[command.index("--key") + 1]]

    def test_exact_source_read_binds_all_owners_and_full_raw_body_without_claiming_clone(self):
        result = self.capture()
        self.assertTrue(result["stored_metadata_verified"])
        self.assertTrue(result["bytes_verified"])
        self.assertTrue(result["get_header_consistent"])
        self.assertFalse(result["clone_verified"])
        self.assertEqual(result["get_header_differences"], [])
        self.assertEqual(result["artifact"], {"file": "object.bin", **self.expected})
        self.assertEqual(set(result["metadata"]), COPY_METADATA_FIELDS)
        self.assertIsNone(result["metadata"]["ContentEncoding"])
        self.assertEqual(len(result["ownership_before"]), 5)
        self.assertEqual({item["bucket"] for item in result["ownership_after"]}, BUCKETS)
        self.assertEqual((self.output / "object.bin").read_bytes(), self.store.body)
        self.assertEqual(len(self.store.calls), 13)
        for command, kwargs in self.store.calls:
            self.assertIn(self.plan["source"]["s3"]["endpoint"], command)
            self.assertTrue(command[command.index("--key") + 1].startswith("source/"))
            self.assertEqual(kwargs["env"]["AWS_MAX_ATTEMPTS"], "1")
            self.assertNotIn("secret-fixture", command)
            self.assertNotIn("--range", command)
            self.assertFalse(any(value.startswith("--response-") for value in command))
        self.assertEqual(json.loads((self.output / "capture.json").read_text(encoding="utf-8")), result)

    def test_rustfs_missing_get_language_is_saved_separately_from_head_metadata(self):
        self.store.downloaded.pop("ContentLanguage")
        self.store.downloaded["ChecksumCRC64NVME"] = "fixture-checksum"
        result = self.capture()
        self.assertTrue(result["stored_metadata_verified"])
        self.assertFalse(result["get_header_consistent"])
        self.assertEqual(result["metadata"]["ContentLanguage"], "en")
        self.assertEqual(result["get_header_differences"], [{"field": "ContentLanguage", "head_present": True,
                         "head_value": "en", "get_present": False, "get_value": None}])
        observed = json.loads((self.output / "get.json").read_text(encoding="utf-8"))
        self.assertNotIn("ContentLanguage", observed)
        self.assertEqual(json.loads((self.output / "head-before.json").read_text(encoding="utf-8")), self.store.head)

    def test_product_subset_and_explicit_null_remain_distinct_observations(self):
        for value in (self.store.head, self.store.downloaded, self.store.after):
            value.pop("ContentLanguage")
            value["Metadata"] = {}
        self.store.downloaded["ContentLanguage"] = None
        result = self.capture()
        self.assertIsNone(result["metadata"]["ContentLanguage"])
        self.assertFalse(result["get_header_consistent"])
        self.assertFalse(result["get_header_differences"][0]["head_present"])
        self.assertTrue(result["get_header_differences"][0]["get_present"])

    def test_version_pins_get_but_final_head_checks_current_object(self):
        for value in (self.store.head, self.store.downloaded, self.store.after):
            value["VersionId"] = "version-one"
        self.capture()
        before, download, after = self.business_calls()
        self.assertNotIn("--if-match", before)
        self.assertEqual(download[download.index("--if-match") + 1], '"fixture-etag"')
        self.assertEqual(download[download.index("--version-id") + 1], "version-one")
        self.assertIn("--if-match", after)
        self.assertNotIn("--version-id", after)

    def test_exact_target_side_is_explicitly_read_only(self):
        result = self.capture(side="target", key="target/system/registered.bin")
        self.assertEqual(result["side"], "target")
        self.assertEqual(result["scope_id"], "target")
        self.assertTrue(all(command[command.index("--key") + 1].startswith("target/") for command, _ in self.store.calls))

    def test_unknown_head_metadata_is_preserved_as_failure_and_never_downloaded(self):
        self.store.head["UnrecognizedObjectPolicy"] = "must-not-drop"
        failure = self.failed()
        self.assertIn("UnrecognizedObjectPolicy", failure["reason"])
        self.assertEqual(len(self.business_calls()), 1)
        self.assertEqual(json.loads((self.output / "head-before.json").read_text(encoding="utf-8"))["UnrecognizedObjectPolicy"], "must-not-drop")

    def test_unknown_get_metadata_is_not_ignored_as_a_header_difference(self):
        self.store.downloaded["ObjectLockMode"] = "GOVERNANCE"
        self.failed()
        self.assertTrue((self.output / "object.bin").exists())
        self.assertEqual(len(self.business_calls()), 2)

    def test_missing_custom_metadata_and_truncated_metadata_refuse_capture(self):
        for field, value in (("Metadata", None), ("MissingMeta", 1), ("TagCount", 1), ("StorageClass", "GLACIER")):
            with self.subTest(field=field):
                self.store.head[field] = value
                self.output = self.work / field
                self.store.heads = 0
                self.failed()
                self.store.head.pop(field)
                if field == "Metadata":
                    self.store.head[field] = {"purpose": "fixture"}

    def test_final_head_detects_metadata_or_version_change_with_same_etag(self):
        for field, value in (("ContentLanguage", "zh-CN"), ("ETag", '"new-etag"'), ("VersionId", "new-version")):
            with self.subTest(field=field):
                self.store.after = {**self.store.head, field: value}
                self.output = self.work / field
                self.store.heads = 0
                self.failed()

    def test_get_identity_change_is_rejected_even_if_bytes_match(self):
        self.store.downloaded["ETag"] = '"different-etag"'
        self.failed()
        self.assertEqual(len(self.business_calls()), 2)

    def test_owner_mismatch_prevents_business_reads(self):
        self.store.owner_failure = "imports"
        self.failed()
        self.assertEqual(self.business_calls(), [])

    def test_denied_read_keeps_sanitized_diagnostic_and_is_not_retried(self):
        self.store.failure = "get-object"
        self.failed()
        diagnostic = json.loads((self.output / "get.diagnostic.json").read_text(encoding="utf-8"))
        self.assertEqual(diagnostic["returncode"], 254)
        self.assertIn("AccessDenied", diagnostic["stderr"])
        self.assertIn("[REDACTED]", diagnostic["stderr"])
        self.assertNotIn("secret-fixture", diagnostic["stderr"])
        self.assertNotIn("access-fixture", diagnostic["stderr"])
        self.assertEqual(len(self.business_calls()), 2)

    def test_oversized_head_is_rejected_before_body_and_partial_or_excess_body_never_pass(self):
        self.store.head["ContentLength"] = 2048
        self.failed()
        self.assertEqual(len(self.business_calls()), 1)
        self.store.head["ContentLength"] = self.expected["bytes"]
        for name, body in (("partial", b"short"), ("excess", b"x" * 2048), ("same-size-wrong-sha", b"x" * self.expected["bytes"])):
            with self.subTest(case=name):
                self.store.body = body
                self.output = self.work / name
                self.store.heads = 0
                self.failed()
                self.assertEqual((self.output / "object.bin").read_bytes(), body)

    def test_duplicate_response_json_and_partial_content_header_fail_closed(self):
        self.store.raw = b'{"ETag":"one","ETag":"two"}'
        self.failed()
        self.store.raw = None
        self.store.downloaded["ContentRange"] = "bytes 0-15/200"
        self.output = self.work / "partial-content"
        self.store.heads = 0
        self.failed()

    def test_scope_keys_size_and_duplicate_output_are_rejected_without_requests(self):
        for key in ("target/system/file", "source/.ryframe-owner", "source/../file", "source//file", "source/",
                    "source/path\\file", "source/file\x7f", "source/" + "长" * 400):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.capture(key=key)
        for values in ({"side": "unregistered"}, {"bucket": "unknown"}, {"max_bytes": 1}, {"max_bytes": True},
                       {"expected": {"bytes": True, "sha256": "a" * 64}}, {"output": self.work},
                       {"output": self.work.parent / "outside"}):
            with self.subTest(values=values), self.assertRaises(ValueError):
                self.capture(**values)
        self.output.mkdir()
        original = self.output / "keep.txt"
        original.write_text("keep")
        with self.assertRaises(ValueError):
            self.capture()
        self.assertEqual(original.read_text(encoding="utf-8"), "keep")
        self.assertEqual(self.store.calls, [])

    def test_tool_tampering_and_reader_write_operations_never_reach_external_runner(self):
        reader = CaptureReader(self.tools, "source", self.work)
        with self.assertRaises(ValueError):
            reader.read("forbidden", "put-object", "uploads", "source/system/file", body=self.original)
        Path(self.plan["tools"]["aws"]["path"]).write_bytes(b"changed-tool")
        self.failed()
        self.assertEqual(self.store.calls, [])

    def test_result_fsync_failure_never_returns_success_even_if_capture_json_remains(self):
        original = capture_module.write_json

        def save(path, value):
            if path.name == "capture.json":
                with patch("devex_clone_capture.os.fsync", side_effect=OSError("fixture fsync failure")):
                    original(path, value)
            else:
                original(path, value)

        with patch("devex_clone_capture.write_json", side_effect=save), self.assertRaises(ObjectCaptureError):
            self.capture()
        self.assertTrue((self.output / "capture.json").exists())
        failure = json.loads((self.output / "failure.json").read_text(encoding="utf-8"))
        self.assertEqual(failure["status"], "capture_failed")
        self.assertEqual(failure["error_type"], "OSError")
        with self.assertRaisesRegex(ValueError, "失败证据"):
            verify_capture(self.tools, "source", "uploads", "source/system/registered.bin", self.output,
                           expected=self.expected, max_bytes=1024)

    def test_owner_change_after_body_keeps_bytes_but_fails_capture(self):
        def read(command, **kwargs):
            response = self.store(command, **kwargs)
            if (command[command.index("s3api") + 1] == "get-object"
                    and not command[command.index("--key") + 1].endswith("/.ryframe-owner")):
                self.store.owner_failure = "uploads"
            return response

        self.tools.run = read
        self.failed()
        self.assertEqual((self.output / "object.bin").read_bytes(), self.store.body)

    def verify(self):
        return verify_capture(self.tools, "source", "uploads", "source/system/registered.bin", self.output,
                              expected=self.expected, max_bytes=1024)

    def test_verify_is_offline_read_only_and_preserves_header_difference(self):
        self.store.downloaded.pop("ContentLanguage")
        captured = self.capture()
        before = {path.name: file_digest(path) for path in self.output.iterdir()}
        calls = len(self.store.calls)
        verified = self.verify()
        self.assertEqual(len(self.store.calls), calls)
        self.assertEqual(verified["status"], "capture_evidence_verified")
        self.assertFalse(verified["live_revalidated"])
        self.assertFalse(verified["clone_verified"])
        self.assertFalse(verified["capture"]["get_header_consistent"])
        self.assertEqual(verified["capture"], captured)
        self.assertEqual(before, {path.name: file_digest(path) for path in self.output.iterdir()})

    def test_verify_recomputes_original_headers_and_refuses_summary_or_diagnostic_tampering(self):
        changes = (("intent.json", "side", "target"), ("capture.json", "get_header_consistent", False),
                   ("head-before.json", "ContentLanguage", "different"),
                   ("get.json", "ChecksumCRC64NVME", "new-observation"),
                   ("head-after.json", "ETag", '"different"'),
                   ("owner-before-uploads.diagnostic.json", "returncode", 254))
        for index, (name, field, value) in enumerate(changes):
            with self.subTest(file=name):
                self.output = self.work / f"tamper-{index}"
                self.store.heads = 0
                self.capture()
                path = self.output / name
                document = json.loads(path.read_text(encoding="utf-8"))
                document[field] = value
                path.write_text(json.dumps(document), encoding="utf-8")
                with self.assertRaises(ValueError):
                    self.verify()

    def test_verify_checks_actual_object_owner_and_tool_bytes_not_self_reported_hashes(self):
        for index, name in enumerate(("object.bin", "owner-before-uploads.bin", "owner-after-config-packages.bin")):
            with self.subTest(file=name):
                self.output = self.work / f"bytes-{index}"
                self.store.heads = 0
                self.capture()
                (self.output / name).write_bytes(b"tampered-bytes")
                with self.assertRaises(ValueError):
                    self.verify()
        self.output = self.work / "changed-tool"
        self.store.heads = 0
        self.capture()
        Path(self.plan["tools"]["aws"]["path"]).write_bytes(b"changed-tool")
        with self.assertRaises(ValueError):
            self.verify()

    def test_verify_requires_every_original_owner_response_and_complete_result(self):
        self.capture()
        owner = self.output / "owner-after-exports.json"
        owner.unlink()
        with self.assertRaises(ValueError):
            self.verify()
        owner.write_text("{}")
        result = self.output / "capture.json"
        result.write_text('{"status":"captured"}')
        with self.assertRaises(ValueError):
            self.verify()


if __name__ == "__main__":
    unittest.main()
