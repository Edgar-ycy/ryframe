"""部门采用守卫只接受原生对象只读证据和当前作用域的 owner。"""
import copy
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import devex_clone_department as department
from devex_clone_capture import write_json
from test_devex_clone_target_tree_claim import object_configuration, owner_receipt, receipt


class DepartmentObjectReceiptTests(unittest.TestCase):
    scope = "department-fixture"

    def target(self):
        return {"scope_id": self.scope, "s3": object_configuration()}

    def owner(self, *, bucket="uploads", number=1):
        name = f"owner-target-{bucket}-{number:032x}.txt"
        return owner_receipt(self.scope, bucket, name)

    def test_accepts_native_read_only_operations(self):
        for value in (receipt(scope=self.scope), receipt("get-bucket-versioning", scope=self.scope), self.owner(),
                      receipt("list-objects-v2", scope=self.scope, response={"IsTruncated": False,
                              "Contents": [{"Key": self.scope + "/.ryframe-owner"}]})):
            with self.subTest(operation=value["operation"]):
                self.assertEqual(department._read_only_command(value, self.target()), value)

    def test_rejects_missing_unknown_fields_and_disguised_writes(self):
        valid = self.owner()
        for field in valid:
            with self.subTest(missing=field), self.assertRaises(ValueError):
                department._read_only_command({key: value for key, value in valid.items()
                                               if key != field}, self.target())
        for change in ({"operation": "put-object"}, {"operation": []}, {"remote_writes": 1},
                       {"remote_writes": False}, {"returncode": False}, {"returncode": 1},
                       {"error_type": "TimeoutError"}, {"bucket": "unknown"}, {"bucket": []},
                       {"transport": "aws-cli"}, {"response": []}, {"unknown": "field"}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                department._read_only_command({**valid, **change}, self.target())

    def test_rejects_legacy_object_command(self):
        with self.assertRaises(ValueError):
            department._read_only_command({"command": ["aws.exe", "s3api", "get-object"],
                "returncode": 0, "error_type": None, "stdout": "{}", "stderr": ""}, self.target())

    def test_rejects_owner_outside_scope_or_attempt_filename(self):
        valid = self.owner()
        for change in ({"key": "other/.ryframe-owner"}, {"key": self.scope + "/business.bin"},
                       {"destination": "../owner.txt"}, {"destination": "C:\\owner.txt"},
                       {"destination": valid["destination"].replace("uploads", "avatar")},
                       {"destination": None}, {"response": {}},
                       {"response": {**valid["response"], "ContentLength": False}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                department._read_only_command({**valid, **change}, self.target())
        for scope in (None, "", "../scope", "bad\nscope"):
            with self.subTest(scope=scope), self.assertRaises(ValueError):
                department._read_only_command(valid, {"scope_id": scope,
                                                      "s3": object_configuration()})

    def test_rejects_non_owner_or_incomplete_listing(self):
        owner = {"Key": self.scope + "/.ryframe-owner"}
        for response in ({}, {"IsTruncated": True, "Contents": [owner]},
                         {"IsTruncated": 0, "Contents": [owner]},
                         {"IsTruncated": False, "Contents": []},
                         {"IsTruncated": False, "Contents": [owner, owner]},
                         {"IsTruncated": False, "Contents": [{"Key": "other/.ryframe-owner"}]},
                         {"IsTruncated": False, "Contents": [owner], "NextContinuationToken": "next"}):
            with self.subTest(response=response), self.assertRaises(ValueError):
                department._read_only_command(receipt("list-objects-v2", scope=self.scope,
                                                      response=response), self.target())
        for operation in ("head-bucket", "get-bucket-versioning", "list-objects-v2"):
            with self.subTest(operation=operation), self.assertRaises(ValueError):
                department._read_only_command(receipt(operation, scope=self.scope,
                                                      destination="unexpected.txt"), self.target())
        with self.assertRaises(ValueError):
            department._read_only_command(receipt("get-bucket-versioning", scope=self.scope,
                                                 response={"Status": "Enabled"}), self.target())

    def test_guard_artifacts_binds_owner_body_and_rejects_duplicates(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parents[2] / ".local-tests") as temporary:
            output = Path(temporary)
            context = SimpleNamespace(backend=output, target_config=self.target())
            value = self.owner()
            body = output / value["destination"]
            body.write_bytes(f"ryframe-owner:v1:{self.scope}:object-storage:uploads".encode())
            name = "command-" + "1" * 32 + ".json"
            write_json(output / name, value)
            self.assertEqual(len(department._guard_artifacts(context, output)), 2)
            body.write_bytes(b"wrong-owner")
            with self.assertRaises(ValueError):
                department._guard_artifacts(context, output)
            body.write_bytes(f"ryframe-owner:v1:{self.scope}:object-storage:uploads".encode())
            write_json(output / ("command-" + "2" * 32 + ".json"), copy.deepcopy(value))
            with self.assertRaises(ValueError):
                department._guard_artifacts(context, output)

    def test_guard_rejects_owner_without_receipt_and_receipt_without_body(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parents[2] / ".local-tests") as temporary:
            output = Path(temporary)
            context = SimpleNamespace(backend=output, target_config=self.target())
            value = self.owner()
            body = output / value["destination"]
            body.write_bytes(f"ryframe-owner:v1:{self.scope}:object-storage:uploads".encode())
            with self.assertRaises(ValueError):
                department._guard_artifacts(context, output)
            body.unlink()
            write_json(output / ("command-" + "1" * 32 + ".json"), value)
            with self.assertRaises(ValueError):
                department._guard_artifacts(context, output)


if __name__ == "__main__":
    unittest.main()
