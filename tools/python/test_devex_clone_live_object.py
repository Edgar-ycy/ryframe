"""当前对象观察的离线状态、归属和未知结果回归；不连接 S3。"""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_live_object as live
from devex_clone_live_object import ObjectObservationError, missing_head, observe_target_object, read_observation
from restore_build import file_digest
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import ExternalTools
from test_devex_clone_capture import ObjectReads


def same_logical_path(left: Path, right: Path) -> bool:
    """比较普通路径与 Windows 扩展路径，使模拟链接与实际文件系统调用命中同一资源。"""
    def logical(value: Path) -> str:
        text = os.fspath(value)
        if text.startswith("\\\\?\\"):
            text = text[4:]
        return os.path.normcase(os.path.normpath(text))

    return logical(left) == logical(right)


class CurrentObjects(ObjectReads):
    def __init__(self):
        super().__init__()
        self.outcomes = ["404", "404"]
        self.on_head = None

    def __call__(self, command, **kwargs):
        if command[-1] != "__s3-read":
            raise AssertionError("对象观察只接受原生 Rust S3 只读协议")
        request = json.loads(kwargs["input"])
        operation, key = request["operation"], request["key"]
        if operation == "head-object" and not key.endswith("/.ryframe-owner"):
            if self.on_head:
                self.on_head()
            outcome = self.outcomes.pop(0) if self.outcomes else None
            if isinstance(outcome, BaseException):
                self.calls.append((command, kwargs))
                raise outcome
            if outcome == "404":
                self.calls.append((command, kwargs))
                value = {"status": "not_found", "http_status": 404,
                         "remote_write_may_have_occurred": False}
                return subprocess.CompletedProcess(
                    command, 0, stdout=json.dumps({"ok": True, "result": value}).encode(), stderr=b"")
            if outcome is not None:
                self.calls.append((command, kwargs))
                response = {"ok": False, "error": {
                    "category": "NativeS3ReadRejected", "message": str(outcome),
                    "remote_write_may_have_occurred": False,
                    "automatic_retry_permitted": False}}
                return subprocess.CompletedProcess(
                    command, 1, stdout=json.dumps(response).encode(), stderr=b"private stderr")
        return super().__call__(command, **kwargs)


class LiveObjectTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.store = CurrentObjects()
        self.original = self.work / "expected.bin"
        self.original.write_bytes(self.store.body)
        self.expected = file_digest(self.original)
        self.tools = ExternalTools(self.plan, self.work, self.store)
        self.output = self.work / "live-object"
        self.env = patch.dict("os.environ", {"TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def observe(self, **changes):
        result = observe_target_object(self.backend, self.tools, changes.get("bucket", "uploads"),
                                     changes.get("key", "target/system/object.bin"), changes.get("output", self.output),
                                     expected=changes.get("expected", self.expected), max_bytes=1024)
        self.receipt_sha256 = file_digest(self.output / "observation.json")["sha256"]
        return result

    def read(self):
        return read_observation(self.output, expected_sha256=self.receipt_sha256)

    def failed(self):
        with self.assertRaises(ObjectObservationError):
            self.observe()
        failure = json.loads((self.output / "failure.json").read_text(encoding="utf-8"))
        self.assertEqual(failure["remote_writes"], 0)
        self.assertFalse(failure["clone_verified"])
        self.assertFalse((self.output / "observation.json").exists())
        return failure

    def business(self):
        return [json.loads(options["input"]) for _, options in self.store.calls
                if not json.loads(options["input"])["key"].endswith("/.ryframe-owner")]

    def test_403_is_not_absence_and_has_no_retry(self):
        self.store.outcomes = ["403"]
        self.failed()
        self.assertEqual(len(self.business()), 1)
        diagnostic = (self.output / "head-before.diagnostic.json").read_text(encoding="utf-8")
        self.assertNotIn("secret-fixture", diagnostic)
        self.assertNotIn("access-fixture", diagnostic)
        self.assertIn("request_rejected", diagnostic)

    def test_native_tool_failure_without_protocol_response_is_not_absence(self):
        self.store.outcomes = [subprocess.CalledProcessError(1, ["xtask"])]
        self.failed()

    def test_timeout_is_not_absence_or_retried(self):
        self.store.outcomes = [subprocess.TimeoutExpired(["xtask", "__s3-read"], 3)]
        self.failed()
        self.assertEqual(len(self.business()), 1)

    def test_ambient_credential_environment_change_does_not_change_file_bound_requests(self):
        import os
        changed = []
        configuration = copy.deepcopy(self.plan["target"]["s3"]["configuration"])

        def change_and_restore():
            changed.append(True)
            os.environ["TEST_SECRET"] = "different-fixture" if len(changed) == 1 else "secret-fixture"

        self.store.on_head = change_and_restore
        self.observe()
        self.assertEqual(len(changed), 2)
        self.assertEqual(len(self.business()), 2)
        self.assertTrue(all(request["configuration_binding"] == configuration
                            for request in self.business()))
        self.assertTrue(all("configuration" not in request for request in self.business()))
        self.assertTrue(all("AWS_SECRET_ACCESS_KEY" not in kwargs["env"]
                            for _, kwargs in self.store.calls))

    def test_invalid_or_preexisting_scope_never_makes_remote_calls(self):
        for key in ("source/system/object.bin", "target/.ryframe-owner", "target/../other"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.observe(key=key)
        self.output.mkdir()
        with self.assertRaises(ValueError):
            self.observe()
        self.assertEqual(self.store.calls, [])


class MissingHeadTests(unittest.TestCase):
    def test_only_closed_native_head_404_is_accepted(self):
        valid = {
            "operation": "head-object", "returncode": 0,
            "error_type": "NativeS3NotFound", "stderr": "",
            "native_failure": {
                "outcome": "not_found", "http_status": 404,
                "remote_write_may_have_occurred": False,
            },
        }
        self.assertTrue(missing_head(valid))
        for value in (
                {**valid, "returncode": 1}, {**valid, "returncode": True},
                {**valid, "error_type": "NativeS3InvocationError"},
                {**valid, "operation": "get-object"}, {**valid, "stderr": "404"},
                {**valid, "native_failure": {**valid["native_failure"], "http_status": 403}},
                {**valid, "native_failure": {**valid["native_failure"],
                                               "remote_write_may_have_occurred": True}},
                {**valid, "extra": "value"},
        ):
            with self.subTest(value=value):
                self.assertFalse(missing_head(value))

    def test_legacy_aws_text_can_never_prove_absence(self):
        for code in ("404", "NotFound", "403"):
            diagnostic = {
                "operation": "head-object", "returncode": 254,
                "error_type": "CalledProcessError",
                "stderr": f"An error occurred ({code}) when calling the HeadObject operation: Not Found",
            }
            with self.subTest(code=code):
                self.assertFalse(missing_head(diagnostic))


if __name__ == "__main__":
    unittest.main()
