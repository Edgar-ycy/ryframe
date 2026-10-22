import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import native_s3_client as client


def request():
    return {
        "format_version": 1,
        "kind": "ryframe-native-s3-put-if-absent",
        "scope_id": "target-scope",
        "configuration": {
            "endpoint": "http://127.0.0.1:9000",
            "region": "us-east-1",
            "access_key": "private-access",
            "secret_key": "private-secret",
        },
        "bucket": "uploads",
        "key": "target-scope/file.txt",
        "source": "D:/fixture/file.txt",
        "expected": {"bytes": 7, "sha256": "a" * 64},
        "metadata": {},
    }


def read_request():
    return {
        "format_version": 1,
        "kind": "ryframe-native-s3-read",
        "scope_id": "source-scope",
        "configuration": request()["configuration"],
        "bucket": "uploads",
        "operation": "head-object",
        "key": "source-scope/file.txt",
        "if_match": None,
        "version": None,
        "destination": None,
        "max_bytes": 0,
    }


def inspect_request():
    return {
        "format_version": 1,
        "kind": "ryframe-native-s3-inspect",
        "scope_id": "source-scope",
        "configuration": request()["configuration"],
        "bucket": "uploads",
        "operation": "list-objects-v2",
        "prefix": "source-scope/",
        "continuation": None,
    }


def create_bucket_request():
    return {
        "format_version": 1,
        "kind": "ryframe-native-s3-create-bucket",
        "scope_id": "fixture-scope",
        "configuration": request()["configuration"],
        "bucket": "uploads",
    }


class NativeS3ClientTests(unittest.TestCase):
    def result(self, outcome, returncode=0):
        return subprocess.CompletedProcess(
            [],
            returncode,
            stdout=json.dumps({"ok": True, "result": outcome}).encode(),
            stderr=b"",
        )

    def test_direct_credentials_only_enter_stdin_and_success_is_exact(self):
        calls = []
        outcome = {
            "status": "created_unverified",
            "http_status": 200,
            "response": {"ETag": '"created"'},
            "remote_write_may_have_occurred": True,
            "automatic_retry_permitted": False,
        }

        def run(command, **options):
            calls.append((command, options))
            return self.result(outcome)

        with patch.dict(os.environ, {
            "APP_ENV": "test", "AWS_ACCESS_KEY_ID": "ambient-access",
            "AWS_SECRET_ACCESS_KEY": "ambient-secret", "PATH": os.environ.get("PATH", ""),
        }, clear=True):
            actual, invocation = client.put_if_absent(request(), run)
        self.assertEqual(actual, outcome)
        self.assertEqual(invocation, {"returncode": 0, "error_type": None})
        command, options = calls[0]
        self.assertIn("__s3-put-if-absent", command)
        self.assertNotIn("private-access", command)
        self.assertNotIn("private-secret", command)
        self.assertFalse(any(name.startswith(("APP_", "AWS_", "RYFRAME_")) for name in options["env"]))
        self.assertEqual(json.loads(options["input"])["configuration"]["secret_key"], "private-secret")

    def test_explicit_protocol_rejection_is_known_no_write(self):
        response = {
            "ok": False,
            "error": {
                "category": "NativeS3PutRejected",
                "message": "配置字段无效",
                "remote_write_may_have_occurred": False,
                "automatic_retry_permitted": False,
            },
        }
        run = lambda *_args, **_kwargs: subprocess.CompletedProcess(
            [], 1, stdout=json.dumps(response).encode(), stderr=b"private-secret"
        )
        with self.assertRaises(client.NativeS3InvocationError) as caught:
            client.put_if_absent(request(), run)
        self.assertEqual(caught.exception.outcome, "request_rejected")
        self.assertFalse(caught.exception.remote_write_may_have_occurred)
        self.assertNotIn("private-secret", str(caught.exception))

    def test_create_bucket_uses_private_protocol_and_closed_write_outcome(self):
        calls = []
        outcome = {
            "status": "created_unverified", "http_status": 200,
            "remote_write_may_have_occurred": True,
            "automatic_retry_permitted": False,
        }

        def run(command, **options):
            calls.append((command, options))
            return self.result(outcome)

        actual, invocation = client.create_bucket(create_bucket_request(), run)
        self.assertEqual(actual, outcome)
        self.assertEqual(invocation, {"returncode": 0, "error_type": None})
        command, options = calls[0]
        self.assertIn("__s3-create-bucket", command)
        self.assertNotIn("private-secret", command)
        self.assertEqual(json.loads(options["input"])["configuration"]["secret_key"],
                         "private-secret")
        self.assertFalse(any(name.startswith(("APP_", "AWS_", "RYFRAME_"))
                             for name in options["env"]))

    def test_timeout_malformed_or_inconsistent_response_requires_reconciliation(self):
        cases = [
            lambda *_a, **_k: (_ for _ in ()).throw(subprocess.TimeoutExpired(["xtask"], 1810)),
            lambda *_a, **_k: subprocess.CompletedProcess([], 0, stdout=b"not-json", stderr=b""),
            lambda *_a, **_k: subprocess.CompletedProcess(
                [], 1, stdout=b'{"ok":true,"result":{}}', stderr=b""
            ),
        ]
        for run in cases:
            with self.subTest(run=run), self.assertRaises(client.NativeS3InvocationError) as caught:
                client.put_if_absent(request(), run)
            self.assertEqual(caught.exception.outcome, "needs_reconciliation")
            self.assertTrue(caught.exception.remote_write_may_have_occurred)

    def test_business_outcome_requires_closed_schema_and_retry_prohibition(self):
        invalid = [
            {"status": "created_unverified", "remote_write_may_have_occurred": True},
            {"status": "success", "remote_write_may_have_occurred": True,
             "automatic_retry_permitted": False},
            {"status": "created_unverified", "remote_write_may_have_occurred": True,
             "automatic_retry_permitted": False, "private": "value"},
        ]
        for outcome in invalid:
            with self.subTest(outcome=outcome), self.assertRaises(client.NativeS3InvocationError):
                client.put_if_absent(request(), lambda *_a, **_k: self.result(outcome))

    def test_read_uses_private_stdin_protocol_and_returns_exact_result(self):
        calls = []
        expected = {"ContentLength": 7, "ETag": '"fixture"'}

        def run(command, **options):
            calls.append((command, options))
            return self.result(expected)

        actual, invocation = client.read(read_request(), run)
        self.assertEqual(actual, expected)
        self.assertEqual(invocation, {"returncode": 0, "error_type": None})
        command, options = calls[0]
        self.assertIn("__s3-read", command)
        self.assertNotIn("private-secret", command)
        self.assertEqual(
            json.loads(options["input"])["configuration"]["secret_key"],
            "private-secret",
        )

    def test_only_closed_native_head_404_becomes_not_found(self):
        outcome = {
            "status": "not_found",
            "http_status": 404,
            "remote_write_may_have_occurred": False,
        }
        with self.assertRaises(client.NativeS3NotFound) as caught:
            client.read(read_request(), lambda *_a, **_k: self.result(outcome))
        self.assertEqual(caught.exception.returncode, 0)
        self.assertEqual(caught.exception.http_status, 404)
        self.assertFalse(caught.exception.remote_write_may_have_occurred)

        for changed in ({**outcome, "http_status": 403},
                        {**outcome, "extra": "value"}):
            with self.subTest(changed=changed):
                result, invocation = client.read(
                    read_request(), lambda *_a, **_k: self.result(changed))
                self.assertEqual(result, changed)
                self.assertEqual(invocation["returncode"], 0)

    def test_read_protocol_failures_are_known_no_write(self):
        cases = [
            lambda *_a, **_k: (_ for _ in ()).throw(
                subprocess.TimeoutExpired(["xtask"], 1810)
            ),
            lambda *_a, **_k: subprocess.CompletedProcess(
                [], 0, stdout=b"not-json", stderr=b"private-secret"
            ),
        ]
        for run in cases:
            with self.subTest(run=run), self.assertRaises(
                client.NativeS3InvocationError
            ) as caught:
                client.read(read_request(), run)
            self.assertFalse(caught.exception.remote_write_may_have_occurred)
            self.assertNotIn("private-secret", str(caught.exception))

    def test_inspect_uses_private_stdin_protocol_without_environment_credentials(self):
        calls = []
        expected = {"IsTruncated": False, "Contents": [{"Key": "source-scope/file.txt"}]}

        def run(command, **options):
            calls.append((command, options))
            return self.result(expected)

        with patch.dict(os.environ, {
            "APP_ENV": "test", "AWS_SECRET_ACCESS_KEY": "ambient-secret",
            "PATH": os.environ.get("PATH", ""),
        }, clear=True):
            actual, invocation = client.inspect(inspect_request(), run)
        self.assertEqual(actual, expected)
        self.assertEqual(invocation, {"returncode": 0, "error_type": None})
        command, options = calls[0]
        self.assertIn("__s3-inspect", command)
        self.assertNotIn("private-secret", command)
        self.assertFalse(any(name.startswith(("APP_", "AWS_", "RYFRAME_")) for name in options["env"]))
        self.assertEqual(json.loads(options["input"])["configuration"]["secret_key"], "private-secret")


if __name__ == "__main__":
    unittest.main()
