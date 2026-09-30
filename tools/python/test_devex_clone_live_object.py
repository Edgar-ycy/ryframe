"""当前对象观察的离线状态、归属和未知结果回归；不连接 S3。"""
import json
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


class CurrentObjects(ObjectReads):
    def __init__(self):
        super().__init__()
        self.outcomes = ["404", "404"]
        self.code = 254
        self.operation = "HeadObject"
        self.retry_suffix = ""
        self.description = "Not Found"
        self.on_head = None

    def __call__(self, command, **kwargs):
        operation = command[command.index("s3api") + 1]
        key = command[command.index("--key") + 1]
        if operation == "head-object" and not key.endswith("/.ryframe-owner"):
            if self.on_head:
                self.on_head()
            outcome = self.outcomes.pop(0) if self.outcomes else None
            if isinstance(outcome, BaseException):
                self.calls.append((command, kwargs))
                raise outcome
            if outcome is not None:
                self.calls.append((command, kwargs))
                raise subprocess.CalledProcessError(self.code, command, stderr=(
                    f"\r\naws: [ERROR]: An error occurred ({outcome}) when calling the {self.operation} operation"
                    f"{self.retry_suffix}: {self.description}\r\n").encode())
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
        return [command for command, _ in self.store.calls
                if not command[command.index("--key") + 1].endswith("/.ryframe-owner")]

    def test_two_explicit_404_and_current_bucket_owner_reads_prove_only_current_absence(self):
        result = self.observe()
        self.assertIsNone(result.capture_directory)
        self.assertEqual(result.absence_evidence_sha256, file_digest(self.output / "observation.json")["sha256"])
        receipt = self.read()
        self.assertEqual(receipt["status"], "object_absent")
        self.assertEqual([item["bucket"] for item in receipt["ownership_before"]], ["uploads"])
        self.assertEqual([item["bucket"] for item in receipt["ownership_after"]], ["uploads"])
        self.assertEqual(len(self.store.calls), 4)
        self.assertEqual([c[c.index("s3api") + 1] for c in self.business()], ["head-object", "head-object"])
        self.assertFalse(receipt["clone_verified"])
        self.assertFalse(receipt["restore_qualified"])
        self.assertNotIn("secret-fixture", (self.output / "head-before.diagnostic.json").read_text(encoding="utf-8"))

    def test_current_present_object_is_really_downloaded_and_full_capture_verified(self):
        self.store.outcomes = []
        result = self.observe()
        self.assertIsNone(result.absence_evidence_sha256)
        self.assertEqual(result.capture_directory, self.output / "capture")
        self.assertEqual((result.capture_directory / "object.bin").read_bytes(), self.store.body)
        operations = [c[c.index("s3api") + 1] for c in self.business()]
        self.assertEqual(operations, ["head-object", "get-object", "head-object"])
        self.assertEqual(len(self.store.calls), 5)
        downloaded = next(c for c in self.business() if "get-object" in c)
        self.assertEqual(downloaded[downloaded.index("--if-match") + 1], self.store.head["ETag"])

    def test_actual_aws_zero_retry_404_proves_only_current_absence(self):
        self.store.retry_suffix = " (reached max retries: 0)"
        self.observe()
        receipt = self.read()
        self.assertEqual(receipt["status"], "object_absent")
        self.assertEqual([item["bucket"] for item in receipt["ownership_before"]], ["uploads"])
        self.assertEqual([item["bucket"] for item in receipt["ownership_after"]], ["uploads"])
        self.assertEqual(len(self.business()), 2)
        self.assertEqual(len(self.store.calls), 4)
        self.assertEqual(receipt["remote_writes"], 0)
        self.assertFalse(receipt["clone_verified"])

    def test_get_header_difference_stays_in_complete_capture_evidence(self):
        self.store.outcomes = []
        self.store.downloaded.pop("ContentLanguage")
        result = self.observe()
        capture = json.loads((result.capture_directory / "capture.json").read_text(encoding="utf-8"))
        self.assertFalse(capture["get_header_consistent"])
        self.assertEqual(capture["metadata"]["ContentLanguage"], "en")

    def test_403_is_not_absence_and_has_no_retry(self):
        self.store.outcomes = ["403"]
        self.store.description = "access-fixture secret-fixture"
        self.failed()
        self.assertEqual(len(self.business()), 1)
        diagnostic = (self.output / "head-before.diagnostic.json").read_text(encoding="utf-8")
        self.assertNotIn("secret-fixture", diagnostic)
        self.assertNotIn("access-fixture", diagnostic)
        self.assertIn("[REDACTED]", diagnostic)

    def test_wrong_operation_cannot_supply_absence(self):
        self.store.operation = "GetObject"
        self.failed()

    def test_cli_failure_without_service_response_is_not_absence(self):
        self.store.code = 255
        self.failed()

    def test_timeout_is_not_absence_or_retried(self):
        self.store.outcomes = [subprocess.TimeoutExpired(["aws"], 3)]
        self.failed()
        self.assertEqual(len(self.business()), 1)

    def test_missing_then_present_is_unknown_instead_of_absent(self):
        self.store.outcomes = ["404", None]
        self.assertEqual(self.failed()["stage"], "head_after")
        self.assertEqual(len(self.business()), 2)

    def test_wrong_owner_blocks_business_request(self):
        self.store.owner_failure = "uploads"
        self.failed()
        self.assertEqual(self.business(), [])

    def test_existing_wrong_content_is_not_adopted(self):
        self.store.outcomes = []
        self.store.body = b"different-object"
        self.failed()

    def test_plan_change_during_live_read_is_rejected(self):
        self.store.on_head = lambda: self.plan["target"]["s3"].update(region="other-region")
        self.assertEqual(self.failed()["stage"], "head_before")
        self.assertEqual(len(self.business()), 1)

    def test_authentication_change_during_live_read_is_rejected(self):
        import os
        self.store.on_head = lambda: os.environ.update(TEST_SECRET="different-fixture")
        self.assertEqual(self.failed()["stage"], "head_before")
        self.assertEqual(len(self.business()), 1)

    def test_temporary_environment_change_cannot_mix_requests_then_restore_binding(self):
        import os
        changed = []

        def change_and_restore():
            changed.append(True)
            os.environ["TEST_SECRET"] = "different-fixture" if len(changed) == 1 else "secret-fixture"

        self.store.on_head = change_and_restore
        self.failed()
        self.assertEqual(len(changed), 1)
        self.assertEqual(len(self.business()), 1)
        self.assertTrue(all(kwargs["env"]["AWS_SECRET_ACCESS_KEY"] == "secret-fixture"
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

    def test_publication_failure_keeps_diagnostics_but_rejects_residual_receipt(self):
        write = live.write_json

        def publish_then_change(path, value):
            write(path, value)
            if path.name == "observation.json":
                (self.output / "head-before.diagnostic.json").write_text("{}", encoding="utf-8")

        with patch.object(live, "write_json", side_effect=publish_then_change):
            with self.assertRaises(ObjectObservationError):
                self.observe()
        self.assertTrue((self.output / "observation.json").is_file())
        self.assertTrue((self.output / "failure.json").is_file())
        with self.assertRaisesRegex(ValueError, "失败"):
            read_observation(self.output, expected_sha256="a" * 64)

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
        with patch.object(Path, "is_symlink", lambda path: path == target or original(path)):
            with self.assertRaisesRegex(ValueError, "链接"):
                self.read()

    def test_dangling_failure_marker_is_not_ignored(self):
        self.observe()
        target = self.output / "failure.json"
        original = Path.is_symlink
        with patch.object(Path, "is_symlink", lambda path: path == target or original(path)):
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
    def test_only_unambiguous_head_404_service_errors_are_accepted(self):
        valid = {"operation": "head-object", "returncode": 254, "error_type": "CalledProcessError",
                 "stderr": "An error occurred (404) when calling the HeadObject operation: Not Found"}
        self.assertTrue(missing_head(valid))
        self.assertTrue(missing_head({**valid, "stderr": valid["stderr"].replace("(404)", "(NotFound)")}))
        for code in ("NoSuchBucket", "AccessDenied", "403", "500", "Timeout"):
            self.assertFalse(missing_head({**valid, "stderr": valid["stderr"].replace("(404)", f"({code})")}))
        for value in ({**valid, "returncode": 0}, {**valid, "returncode": True},
                      {**valid, "error_type": "TimeoutExpired"}, {**valid, "operation": "get-object"},
                      {**valid, "stderr": valid["stderr"] + "\n" + valid["stderr"]}):
            with self.subTest(value=value):
                self.assertFalse(missing_head(value))

    def test_actual_aws_zero_retry_format_is_exact_and_unambiguous(self):
        diagnostic = {"operation": "head-object", "returncode": 254, "error_type": "CalledProcessError",
                      "stderr": "\r\naws: [ERROR]: An error occurred (404) when calling the HeadObject operation "
                                "(reached max retries: 0): Not Found\r\n"}
        self.assertTrue(missing_head(diagnostic))
        self.assertTrue(missing_head({**diagnostic, "stderr": diagnostic["stderr"].replace("(404)", "(NotFound)")}))
        for old, new in (("(404)", "(403)"), ("HeadObject", "GetObject"),
                         ("max retries: 0", "max retries: 1"), ("max retries: 0", "max retries: 00"),
                         ("max retries: 0", "max retries: unknown"), ("Not Found", "Timeout"),
                         ("Not Found", "Unknown failure"), ("Not Found", "Not Found; access denied")):
            with self.subTest(new=new):
                self.assertFalse(missing_head({**diagnostic, "stderr": diagnostic["stderr"].replace(old, new)}))
        for value in (diagnostic["stderr"] * 2, diagnostic["stderr"] + "timeout\n",
                      "Unknown diagnostic\n" + diagnostic["stderr"], diagnostic["stderr"].replace(": Not Found", ":\nNot Found")):
            with self.subTest(stderr=value):
                self.assertFalse(missing_head({**diagnostic, "stderr": value}))
        for field, value in (("returncode", 255), ("returncode", 0), ("returncode", True),
                             ("error_type", "TimeoutExpired"), ("operation", "get-object")):
            with self.subTest(field=field, value=value):
                self.assertFalse(missing_head({**diagnostic, field: value}))


if __name__ == "__main__":
    unittest.main()
