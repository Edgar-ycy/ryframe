import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from restore_build import file_digest
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import COPY_METADATA_FIELDS, ExternalTools, ObjectCreateError, copy_object_metadata, object_create_outcome


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
        bucket, key = command[command.index("--bucket") + 1], command[command.index("--key") + 1]
        conditional = "--if-none-match" in command and command[command.index("--if-none-match") + 1] == "*"
        if conditional and (bucket, key) in self.objects:
            raise subprocess.CalledProcessError(255, command, stderr=b"An error occurred (PreconditionFailed) when calling the PutObject operation: existing")
        if self.failure in {"AccessDenied", "403", "ConditionalRequestConflict", "409", "UnknownCode"}:
            message = f"An error occurred ({self.failure}) when calling the PutObject operation: fixture-private-detail"
            raise subprocess.CalledProcessError(255, command, stderr=message.encode())
        request = Path(command[command.index("--cli-input-json") + 1].removeprefix("file://"))
        source = Path(command[command.index("--body") + 1])
        self.objects[bucket, key] = source.read_bytes(), json.loads(request.read_text(encoding="utf-8"))
        if self.failure == "timeout-after-write":
            raise subprocess.TimeoutExpired(command, 1800)
        if self.failure == "body-changed":
            source.write_bytes(b"different")
        output = json.dumps({"ETag": '"fixture-etag"'}).encode()
        if self.failure == "invalid-response":
            output = b"not-json"
        if self.failure == "no-etag":
            output = b"{}"
        return subprocess.CompletedProcess(command, 0, stdout=output)


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
        self.assertEqual(command[command.index("--if-none-match") + 1], "*")
        self.assertEqual(command[command.index("--endpoint-url") + 1], self.plan["target"]["s3"]["endpoint"])
        self.assertEqual(kwargs["env"]["AWS_MAX_ATTEMPTS"], "1")
        self.assertEqual(kwargs["env"]["AWS_ACCESS_KEY_ID"], "access-fixture")
        self.assertNotIn("access-fixture", command)
        self.assertNotIn("secret-fixture", command)
        self.assertEqual(self.store.objects["uploads", "target/system/file.txt"], (b"fixture-data", copy_object_metadata(metadata())))
        request = Path(result["request_file"])
        self.assertTrue(request.is_relative_to(self.work))
        self.assertEqual(json.loads(request.read_text()), copy_object_metadata(metadata()))
        diagnostic = json.loads(Path(result["diagnostic_file"]).read_text())
        self.assertEqual(diagnostic["request_file"], str(request))
        self.assertEqual(diagnostic["outcome"], "created_unverified")
        self.assertEqual(diagnostic["stderr"], "")

    def test_absent_business_headers_are_explicit_null_and_not_sent_as_defaults(self):
        value = {**dict.fromkeys(COPY_METADATA_FIELDS), "ContentType": "application/octet-stream", "Metadata": {}}
        self.create(metadata=value)
        self.assertEqual(self.store.objects["uploads", "target/system/file.txt"][1],
                         {"ContentType": "application/octet-stream", "Metadata": {}})

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
        self.assertIn("PreconditionFailed", diagnostic["stderr"])

    def test_denied_conflict_and_unknown_errors_are_not_absence_or_retried(self):
        for failure, outcome in (("AccessDenied", "denied"), ("403", "denied"),
                                 ("ConditionalRequestConflict", "conflict"), ("409", "conflict"),
                                 ("UnknownCode", "needs_reconciliation")):
            self.store.failure = failure
            self.store.calls.clear()
            with self.subTest(failure=failure), self.assertRaises(ObjectCreateError) as caught:
                self.create()
            self.assertEqual(caught.exception.outcome, outcome)
            self.assertNotIn("fixture-private-detail", str(caught.exception))
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
            self.assertIsNotNone(diagnostic["error_type"])

    def test_cli_retry_annotation_is_recognized_and_each_request_keeps_sanitized_diagnostics(self):
        credential = 'fixture+/secret"with space'
        token = "fixture-session-token"
        with patch.dict("os.environ", {"TEST_SECRET": credential, "APP_SESSION_TOKEN": token}):
            for code, expected in (("PreconditionFailed", "already_exists"), ("412", "already_exists"),
                                   ("ConditionalRequestConflict", "conflict"), ("AccessDenied", "denied"),
                                   ("UnknownCode", "needs_reconciliation")):
                raw = (f"\nAn error occurred ({code}) when calling the PutObject operation (reached max retries: 0): "
                       f"fixture diagnostic access-fixture {credential} {quote(credential, safe='')} "
                       f"{json.dumps(credential)[1:-1]} {token}\n")
                run = Mock(side_effect=subprocess.CalledProcessError(255, ["aws"], stderr=raw.encode()))
                self.tools.run = run
                with self.subTest(code=code), self.assertRaises(ObjectCreateError) as caught:
                    self.create()
                self.assertEqual(caught.exception.outcome, expected)
                self.assertEqual(run.call_count, 1)
                diagnostic_path = Path(caught.exception.diagnostic_file)
                self.assertTrue(diagnostic_path.is_relative_to(self.work))
                document = json.loads(diagnostic_path.read_text(encoding="utf-8"))
                self.assertEqual(document["request_file"], caught.exception.request_file)
                self.assertEqual(document["outcome"], expected)
                self.assertEqual(document["returncode"], 255)
                self.assertEqual(document["error_type"], "CalledProcessError")
                self.assertIn("(reached max retries: 0):", document["stderr"])
                self.assertIn("[REDACTED]", document["stderr"])
                for secret in (credential, quote(credential, safe=""), json.dumps(credential)[1:-1], token, "access-fixture"):
                    self.assertNotIn(secret, document["stderr"])
                self.assertNotIn(credential, str(caught.exception))

    def test_installed_cli_error_prefix_uses_captured_precondition_failure(self):
        raw = ("\r\naws: [ERROR]: An error occurred (PreconditionFailed) when calling the PutObject operation "
               "(reached max retries: 0): At least one of the pre-conditions you specified did not hold\r\n")
        run = Mock(side_effect=subprocess.CalledProcessError(254, ["aws"], stderr=raw.encode()))
        self.tools.run = run
        with self.assertRaises(ObjectCreateError) as caught:
            self.create()
        self.assertEqual(caught.exception.outcome, "already_exists")
        self.assertEqual(run.call_count, 1)
        diagnostic = json.loads(Path(caught.exception.diagnostic_file).read_text(encoding="utf-8"))
        self.assertEqual(diagnostic["stderr"], raw)
        self.assertEqual(diagnostic["returncode"], 254)
        for ambiguous in (raw + raw, raw + raw.replace("aws: [ERROR]: ", ""),
                          raw.replace("aws: [ERROR]: ", "aws: [WARNING]: "),
                          raw.replace("aws: [ERROR]: ", "unrecognized prefix ")):
            with self.subTest(stderr=ambiguous):
                self.assertEqual(object_create_outcome(ambiguous), "needs_reconciliation")

    def test_unknown_or_conflicting_cli_error_formats_remain_unknown(self):
        valid = "An error occurred (PreconditionFailed) when calling the PutObject operation (reached max retries: 0): existing"
        for text in (valid.replace("PutObject", "GetObject"), valid.replace("max retries: 0", "max retries: unknown"),
                     valid + "\n" + valid.replace("PreconditionFailed", "AccessDenied"),
                     "unrecognized " + valid, ""):
            with self.subTest(stderr=text):
                self.assertEqual(object_create_outcome(text), "needs_reconciliation")

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

    def test_formal_restore_put_keeps_existing_behavior(self):
        run = Mock(return_value=subprocess.CompletedProcess([], 0, stdout=b"{}"))
        tools = ExternalTools(self.plan, self.work, run)
        tools.aws("target", "put-object", "uploads", "target/system/file.txt", self.source, content_type="text/plain")
        command = run.call_args.args[0]
        self.assertNotIn("--if-none-match", command)
        self.assertNotIn("--cli-input-json", command)
        self.assertEqual(command[command.index("--content-type") + 1], "text/plain")


if __name__ == "__main__":
    unittest.main()
