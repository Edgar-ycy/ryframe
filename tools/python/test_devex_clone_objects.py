import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from restore_build import file_digest
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import COPY_METADATA_FIELDS, ExternalTools, ObjectCreateError, copy_object_metadata


def metadata():
    return {**dict.fromkeys(COPY_METADATA_FIELDS), "ContentType": "text/plain; charset=utf-8",
            "CacheControl": "private, max-age=60", "ContentDisposition": 'attachment; filename="report.txt"',
            "ContentEncoding": "gzip", "ContentLanguage": "zh-CN", "Expires": "2030-01-01T00:00:00Z",
            "WebsiteRedirectLocation": "/report", "Metadata": {"purpose": "fixture", "empty": ""}}


class ObjectStore:
    """离线条件写模型；只用于观察是否重试、丢弃条件或覆盖，不代表真实 RustFS。"""
    def __init__(self):
        self.objects, self.calls, self.failure = {}, [], None

    def __call__(self, command, **kwargs):
        self.calls.append((command, kwargs))
        if "__s3-put-if-absent" in command:
            return self.native_put(command, kwargs)
        raise AssertionError("对象条件写入只接受原生 Rust S3 私有协议")

    def native_put(self, command, kwargs):
        request = json.loads(kwargs["input"].decode("utf-8"))
        bucket, key = request["bucket"], request["key"]
        base = {"automatic_retry_permitted": False}
        if (bucket, key) in self.objects:
            outcome = {**base, "status": "already_exists", "http_status": 412,
                       "remote_write_may_have_occurred": False}
        elif self.failure in {"AccessDenied", "403"}:
            outcome = {**base, "status": "denied", "http_status": 403,
                       "remote_write_may_have_occurred": False}
        elif self.failure in {"ConditionalRequestConflict", "409", "UnknownCode"}:
            outcome = {**base, "status": "needs_reconciliation", "http_status": 409,
                       "remote_write_may_have_occurred": True}
        else:
            source = Path(request["source"])
            self.objects[bucket, key] = source.read_bytes(), request["metadata"]
            if self.failure == "timeout-after-write":
                raise subprocess.TimeoutExpired(command, 1810)
            if self.failure == "body-changed":
                source.write_bytes(b"different")
                outcome = {**base, "status": "needs_reconciliation", "reason": "source_changed",
                           "http_status": 200, "remote_write_may_have_occurred": True}
            elif self.failure in {"invalid-response", "no-etag"}:
                outcome = {**base, "status": "needs_reconciliation",
                           "reason": "success_response_incomplete", "http_status": 200,
                           "remote_write_may_have_occurred": True}
            else:
                outcome = {**base, "status": "created_unverified", "http_status": 200,
                           "remote_write_may_have_occurred": True,
                           "response": {"ETag": '"fixture-etag"'}}
        return subprocess.CompletedProcess(
            command, 0, stdout=json.dumps({"ok": True, "result": outcome}).encode(), stderr=b""
        )


class CloneObjectTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.source = self.work / "source.bin"
        self.source.write_bytes(b"fixture-data")
        self.expected = file_digest(self.source)
        self.store = ObjectStore()
        self.tools = ExternalTools(self.plan, self.work, self.store)
        self.environment = patch.dict("os.environ", {"TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture", "AWS_MAX_ATTEMPTS": "9"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def create(self, **values):
        return self.tools.create_object_if_absent(values.get("bucket", "uploads"), values.get("key", "target/system/file.txt"),
                                                 self.source, values.get("metadata", metadata()), values.get("expected", self.expected))

    def test_all_business_metadata_preserved_and_dates_have_unambiguous_utc_semantics(self):
        original = metadata()
        normalized = copy_object_metadata(original)
        self.assertEqual(normalized, {**original, "Expires": "2030-01-01T00:00:00+00:00"})
        self.assertEqual(original["Expires"], "2030-01-01T00:00:00Z")
        shifted = {**original, "Expires": "2030-01-01T08:00:00+08:00"}
        self.assertEqual(copy_object_metadata(shifted), normalized)
        normalized["Metadata"]["purpose"] = "changed"
        self.assertEqual(original["Metadata"]["purpose"], "fixture")

    def test_missing_unknown_or_ambiguous_metadata_never_reaches_external_tool(self):
        invalid = [None, {**metadata(), "StorageClass": "STANDARD"}]
        for field in COPY_METADATA_FIELDS:
            value = metadata(); value.pop(field); invalid.append(value)
        for value in (None, "", "text", "text/*", "text/plain\r\nInjected: value", "text/plain; charset=x; Charset=y"):
            invalid.append({**metadata(), "ContentType": value})
        for field, value in (("Metadata", None), ("Metadata", {"Name": "value"}),
                             ("Metadata", {"name": "value\n"}), ("Metadata", {"name": "x" * 2049}),
                             ("ContentDisposition", "附件"), ("CacheControl", ""),
                             ("Expires", "2030-01-01T00:00:00"), ("Expires", "2030-02-31T00:00:00Z")):
            invalid.append({**metadata(), field: value})
        for value in invalid:
            with self.subTest(metadata=value), self.assertRaises(ValueError):
                self.create(metadata=value)
        self.assertEqual(self.store.calls, [])

    def test_conditional_create_sends_exact_target_scope_metadata_and_no_secret_in_arguments(self):
        result = self.create()
        self.assertEqual(result["status"], "created_unverified")
        self.assertEqual(len(self.store.calls), 1)
        command, kwargs = self.store.calls[0]
        self.assertIn("__s3-put-if-absent", command)
        self.assertFalse(any(name.startswith("AWS_") for name in kwargs["env"]))
        private = json.loads(kwargs["input"].decode("utf-8"))
        self.assertEqual(
            private["configuration_binding"], self.plan["target"]["s3"]["configuration"]
        )
        self.assertNotIn("access-fixture", command)
        self.assertNotIn("secret-fixture", command)
        self.assertEqual(self.store.objects["uploads", "target/system/file.txt"], (b"fixture-data", copy_object_metadata(metadata())))
        request = Path(result["request_file"])
        self.assertTrue(request.is_relative_to(self.work))
        intent = json.loads(request.read_text())
        self.assertEqual(intent["transport"], "rust-s3-sigv4")
        self.assertEqual(intent["metadata"], copy_object_metadata(metadata()))
        self.assertNotIn("access_key", intent)
        self.assertNotIn("secret_key", intent)
        diagnostic = json.loads(Path(result["diagnostic_file"]).read_text())
        self.assertEqual(diagnostic["request_file"], str(request))
        self.assertEqual(diagnostic["outcome"], "created_unverified")
        self.assertTrue(diagnostic["remote_write_may_have_occurred"])

    def test_absent_business_headers_are_explicit_null_and_not_sent_as_defaults(self):
        value = {**dict.fromkeys(COPY_METADATA_FIELDS), "ContentType": "application/octet-stream", "Metadata": {}}
        self.create(metadata=value)
        self.assertEqual(self.store.objects["uploads", "target/system/file.txt"][1], value)

    def test_existing_object_is_rejected_without_overwrite_or_unconditional_retry(self):
        original = b"keep-original", {"ContentType": "image/png", "Metadata": {"keep": "yes"}}
        self.store.objects["uploads", "target/system/file.txt"] = copy.deepcopy(original)
        with self.assertRaises(ObjectCreateError) as caught:
            self.create()
        self.assertEqual(caught.exception.outcome, "already_exists")
        self.assertEqual(len(self.store.calls), 1)
        self.assertEqual(self.store.objects["uploads", "target/system/file.txt"], original)
        diagnostic = json.loads(Path(caught.exception.diagnostic_file).read_text())
        self.assertEqual(diagnostic["outcome"], "already_exists")
        self.assertFalse(diagnostic["remote_write_may_have_occurred"])

    def test_denied_conflict_and_unknown_errors_are_not_absence_or_retried(self):
        for failure, outcome in (("AccessDenied", "denied"), ("403", "denied"),
                                 ("ConditionalRequestConflict", "needs_reconciliation"), ("409", "needs_reconciliation"),
                                 ("UnknownCode", "needs_reconciliation")):
            self.store.failure = failure
            self.store.calls.clear()
            with self.subTest(failure=failure), self.assertRaises(ObjectCreateError) as caught:
                self.create()
            self.assertEqual(caught.exception.outcome, outcome)
            self.assertEqual(len(self.store.calls), 1)
            self.assertEqual(self.store.objects, {})

    def test_unknown_after_write_retains_result_for_future_reconciliation_and_does_not_retry(self):
        for failure in ("timeout-after-write", "invalid-response", "no-etag", "body-changed"):
            self.source.write_bytes(b"fixture-data")
            self.store.objects.clear(); self.store.calls.clear(); self.store.failure = failure
            with self.subTest(failure=failure), self.assertRaises(ObjectCreateError) as caught:
                self.create()
            self.assertEqual(caught.exception.outcome, "needs_reconciliation")
            self.assertEqual(len(self.store.calls), 1)
            self.assertEqual(self.store.objects["uploads", "target/system/file.txt"][0], b"fixture-data")
            diagnostic = json.loads(Path(caught.exception.diagnostic_file).read_text())
            self.assertEqual(diagnostic["outcome"], "needs_reconciliation")
            self.assertTrue(diagnostic["remote_write_may_have_occurred"])

    def test_wrapper_failures_remain_unknown_and_never_persist_private_stderr(self):
        credential = 'fixture+/secret"with space'
        token = "fixture-session-token"
        with patch.dict("os.environ", {"TEST_SECRET": credential, "APP_SESSION_TOKEN": token}):
            for code in ("ProtocolRejected", "Timeout", "AccessDenied", "UnknownCode"):
                raw = (f"\nNative S3 invocation failed ({code}): "
                       f"fixture diagnostic access-fixture {credential} {quote(credential, safe='')} "
                       f"{json.dumps(credential)[1:-1]} {token}\n")
                run = Mock(side_effect=subprocess.CalledProcessError(255, ["xtask"], stderr=raw.encode()))
                self.tools.run = run
                with self.subTest(code=code), self.assertRaises(ObjectCreateError) as caught:
                    self.create()
                self.assertEqual(caught.exception.outcome, "needs_reconciliation")
                self.assertEqual(run.call_count, 1)
                diagnostic_path = Path(caught.exception.diagnostic_file)
                self.assertTrue(diagnostic_path.is_relative_to(self.work))
                document = json.loads(diagnostic_path.read_text(encoding="utf-8"))
                self.assertEqual(document["request_file"], caught.exception.request_file)
                self.assertEqual(document["outcome"], "needs_reconciliation")
                self.assertEqual(document["returncode"], 255)
                self.assertEqual(document["error_type"], "CalledProcessError")
                self.assertNotIn("stderr", document)
                self.assertTrue(document["remote_write_may_have_occurred"])
                self.assertNotIn(credential, str(caught.exception))

    def test_wrong_scope_owner_path_or_body_hash_rejected_before_write(self):
        for key in ("source/system/file.txt", "target/.ryframe-owner", "target/../outside", "target/",
                    "target//file", "target/system\\file", "target/file\x7f", "target/" + "x" * 1024):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.create(key=key)
        for values in ({"bucket": "unknown"}, {"expected": {"bytes": 12, "sha256": "f" * 64}},
                       {"expected": {"bytes": False, "sha256": "f" * 64}}):
            with self.assertRaises(ValueError):
                self.create(**values)
        self.plan["target"]["scope_id"] = "target/../source"
        with self.assertRaises(ValueError):
            self.create(key="target/../source/file")
        self.assertEqual(self.store.calls, [])

if __name__ == "__main__":
    unittest.main()
