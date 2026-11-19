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

    def test_two_explicit_404_and_current_bucket_owner_reads_prove_only_current_absence(self):
        result = self.observe()
        self.assertIsNone(result.capture_directory)
        self.assertEqual(result.absence_evidence_sha256, file_digest(self.output / "observation.json")["sha256"])
        receipt = self.read()
        self.assertEqual(receipt["status"], "object_absent")
        self.assertEqual([item["bucket"] for item in receipt["ownership_before"]], ["uploads"])
        self.assertEqual([item["bucket"] for item in receipt["ownership_after"]], ["uploads"])
        self.assertEqual(len(self.store.calls), 4)
        self.assertEqual([c["operation"] for c in self.business()], ["head-object", "head-object"])
        self.assertFalse(receipt["clone_verified"])
        self.assertFalse(receipt["restore_qualified"])
        self.assertEqual(receipt["remote_writes"], 0)
        self.assertNotIn("secret-fixture", (self.output / "head-before.diagnostic.json").read_text(encoding="utf-8"))

    def test_current_present_object_is_really_downloaded_and_full_capture_verified(self):
        self.store.outcomes = []
        result = self.observe()
        self.assertIsNone(result.absence_evidence_sha256)
        self.assertEqual(result.capture_directory, self.output / "capture")
        self.assertEqual((result.capture_directory / "object.bin").read_bytes(), self.store.body)
        operations = [c["operation"] for c in self.business()]
        self.assertEqual(operations, ["head-object", "get-object", "head-object"])
        self.assertEqual(len(self.store.calls), 5)
        downloaded = next(c for c in self.business() if c["operation"] == "get-object")
        self.assertEqual(downloaded["if_match"], self.store.head["ETag"])

    def test_get_header_difference_stays_in_complete_capture_evidence(self):
        self.store.outcomes = []
        self.store.downloaded.pop("ContentLanguage")
        result = self.observe()
        capture = json.loads((result.capture_directory / "capture.json").read_text(encoding="utf-8"))
        self.assertFalse(capture["get_header_consistent"])
        self.assertEqual(capture["metadata"]["ContentLanguage"], "en")

    def test_403_is_not_absence_and_has_no_retry(self):
        self.store.outcomes = ["403"]
        self.failed()
        self.assertEqual(len(self.business()), 1)
        diagnostic = (self.output / "head-before.diagnostic.json").read_text(encoding="utf-8")
        self.assertNotIn("secret-fixture", diagnostic)
        self.assertNotIn("access-fixture", diagnostic)
        self.assertIn("request_rejected", diagnostic)

    def test_rejection_text_claiming_404_cannot_supply_absence(self):
        self.store.outcomes = ["404 NotFound HeadObject"]
        self.failed()

    def test_native_tool_failure_without_protocol_response_is_not_absence(self):
        self.store.outcomes = [subprocess.CalledProcessError(1, ["xtask"])]
        self.failed()

    def test_timeout_is_not_absence_or_retried(self):
        self.store.outcomes = [subprocess.TimeoutExpired(["xtask", "__s3-read"], 3)]
        self.failed()
        self.assertEqual(len(self.business()), 1)

    def test_existing_wrong_content_is_not_adopted(self):
        self.store.outcomes = []
        self.store.body = b"different-object"
        self.failed()

    def test_plan_change_during_live_read_is_rejected(self):
        self.store.on_head = lambda: self.plan["target"]["s3"].update(region="other-region")
        self.assertEqual(self.failed()["stage"], "head_before")
        self.assertEqual(len(self.business()), 1)

    def test_plan_authentication_metadata_change_during_live_read_is_rejected(self):
        self.store.on_head = lambda: self.plan["target"]["s3"].update(
            secret_key="different-fixture")
        self.assertEqual(self.failed()["stage"], "head_before")
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

    def test_successful_head_with_binding_change_is_rejected_before_download(self):
        self.store.outcomes = []
        self.store.on_head = lambda: self.plan["target"]["s3"].update(region="other-region")
        self.assertEqual(self.failed()["stage"], "head_before")
        self.assertEqual(len(self.business()), 1)

    def test_saved_receipt_refuses_modified_diagnostic(self):
        self.observe()
        (self.output / "head-before.diagnostic.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.read()

    def test_saved_receipt_cannot_read_an_evidence_path_outside_output(self):
        self.observe()
        receipt = self.read()
        receipt["evidence"] = {"../expected.bin": self.expected}
        (self.output / "observation.json").write_text(json.dumps(receipt), encoding="utf-8")
        # 即使外部原始摘要误绑定了恶意路径，也不能越过独立路径校验。
        self.receipt_sha256 = file_digest(self.output / "observation.json")["sha256"]
        with self.assertRaisesRegex(ValueError, "越界"):
            self.read()

    def test_saved_receipt_cannot_remove_required_evidence_from_bound_receipt(self):
        self.observe()
        receipt = self.read()
        receipt["evidence"] = {}
        (self.output / "observation.json").write_text(json.dumps(receipt), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "已绑定摘要"):
            self.read()

    def test_saved_evidence_link_is_rejected_even_with_identical_bytes(self):
        self.observe()
        target = self.output / "head-before.diagnostic.json"
        original = Path.is_symlink
        with patch.object(Path, "is_symlink",
                          lambda path: same_logical_path(path, target) or original(path)):
            with self.assertRaisesRegex(ValueError, "链接"):
                self.read()

    def test_native_saved_evidence_link_is_rejected(self):
        self.observe()
        path = self.output / "head-before.diagnostic.json"
        real = self.work / "native-head-before.diagnostic.json"
        real.write_bytes(path.read_bytes())
        path.unlink()
        try:
            path.symlink_to(real)
        except OSError:
            real.unlink()
            self.skipTest("当前 Windows 权限不允许创建符号链接")
        self.addCleanup(path.unlink)
        with self.assertRaisesRegex(ValueError, "链接"):
            self.read()

    def test_dangling_failure_marker_is_not_ignored(self):
        self.observe()
        target = self.output / "failure.json"
        original = Path.is_symlink
        with patch.object(Path, "is_symlink",
                          lambda path: same_logical_path(path, target) or original(path)):
            with self.assertRaisesRegex(ValueError, "失败"):
                self.read()

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
