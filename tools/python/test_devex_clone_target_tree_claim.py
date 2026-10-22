"""原生对象收据必须证明逐桶前后像和精确 owner 文件。"""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from devex_clone_capture import read_json, write_json
import devex_clone_target_tree_claim as claim
from restore_reference_plan import BUCKETS, plan_hash


def object_configuration():
    return {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1",
            "access_key": "fixture-access", "secret_key": "fixture-secret"}


def receipt(operation="head-bucket", bucket="uploads", *, scope="claim-fixture",
            configuration=None, **changes):
    configuration = configuration or object_configuration()
    return {"transport": "rust-s3-sigv4", "operation": operation, "bucket": bucket,
            "returncode": 0, "error_type": None, "response": {}, "key": None,
            "destination": None, "remote_writes": 0, "scope_id": scope,
            "endpoint": configuration["endpoint"],
            "configuration_sha256": plan_hash(configuration),
            "prefix": scope + "/" if operation == "list-objects-v2" else None,
            "continuation": None, **changes}


def owner_receipt(scope, bucket, destination):
    body = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
    return receipt("get-object", bucket, scope=scope, key=scope + "/.ryframe-owner", destination=destination,
                   response={"ContentLength": len(body), "ETag": '"owner"', "Metadata": {},
                             "ContentType": "text/plain", "LastModified": "2026-10-23T00:00:00Z"})


class NativeObjectClaimTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(
            dir=Path(__file__).resolve().parents[2] / ".local-tests")
        self.addCleanup(self.temporary.cleanup)
        self.output = Path(self.temporary.name)
        self.scope = "claim-fixture"
        self.request = {"target": {"scope_id": self.scope, "s3": object_configuration()}}
        self.additions, self.prepared = {}, []
        self.counter = 0

    def save(self, stage, value, *, prepared=False):
        self.counter += 1
        name = f"{stage}-{self.counter:032x}.command.json"
        write_json(self.output / name, value)
        if prepared:
            self.prepared.append({"path": name})
        else:
            self.additions[name] = {"type": "file"}
        return name

    def static(self):
        for stage, operation in (("bucket-readiness", "head-bucket"),
                                 ("bucket-versioning", "get-bucket-versioning"),
                                 ("scoped-list", "list-objects-v2")):
            for bucket in sorted(BUCKETS):
                before = receipt(operation, bucket, scope=self.scope)
                after = copy.deepcopy(before)
                if operation == "list-objects-v2":
                    before["response"] = {"IsTruncated": False, "Contents": []}
                    after["response"] = {"IsTruncated": False,
                                         "Contents": [{"Key": self.scope + "/.ryframe-owner"}]}
                self.save(stage, before, prepared=True)
                self.save(stage, after)

    def owners(self):
        for index, bucket in enumerate(sorted(BUCKETS)):
            name = f"owner-{bucket}-{index:032x}.bin"
            (self.output / name).write_bytes(
                f"ryframe-owner:v1:{self.scope}:object-storage:{bucket}".encode())
            self.additions[name] = {"type": "file"}
            self.save("owner-read", owner_receipt(self.scope, bucket, name))

    def replace(self, name, value):
        (self.output / name).write_text(json.dumps(value), encoding="utf-8")

    def test_static_round_covers_exact_five_buckets(self):
        self.static()
        self.assertEqual(claim._static_receipts(self.output, self.additions,
                                               self.prepared, self.request), set(self.additions))

    def test_static_rejects_mutated_schema_and_observations(self):
        self.static()
        first = next(iter(self.additions))
        valid = read_json(self.output / first)
        mutations = [{"remote_writes": 1}, {"remote_writes": False}, {"returncode": False},
                     {"operation": "put-object"}, {"transport": "aws-cli"},
                     {"bucket": "unknown"}, {"bucket": []}, {"key": "other/key"},
                     {"destination": "escape.bin"}, {"response": []},
                     {"scope_id": "other-scope"}, {"endpoint": "http://127.0.0.1:1"},
                     {"configuration_sha256": "f" * 64}, {"prefix": "other/"},
                     {"continuation": "unexpected"},
                     {"response": {"Status": "Enabled"}}, {"error_type": "TimeoutError"}]
        for change in mutations:
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.replace(first, {**valid, **change})
                claim._static_receipts(self.output, self.additions, self.prepared, self.request)
        for field in valid:
            with self.subTest(missing=field), self.assertRaises(ValueError):
                self.replace(first, {key: value for key, value in valid.items() if key != field})
                claim._static_receipts(self.output, self.additions, self.prepared, self.request)

    def test_lists_reject_partial_duplicate_outside_scope_and_unknown_fields(self):
        self.static()
        name = next(name for name in self.additions if name.startswith("scoped-list"))
        valid = read_json(self.output / name)
        owner = {"Key": self.scope + "/.ryframe-owner"}
        for response in ({"IsTruncated": True, "Contents": [owner]},
                         {"IsTruncated": 0, "Contents": [owner]},
                         {"IsTruncated": False, "Contents": [owner, owner]},
                         {"IsTruncated": False, "Contents": [{"Key": "other/.ryframe-owner"}]},
                         {"IsTruncated": False, "Contents": []},
                         {"IsTruncated": False, "Contents": [owner], "NextContinuationToken": "x"}):
            with self.subTest(response=response), self.assertRaises(ValueError):
                self.replace(name, {**valid, "response": response})
                claim._static_receipts(self.output, self.additions, self.prepared, self.request)

    def test_static_rejects_duplicate_bucket_and_nonempty_baseline(self):
        self.static()
        name = self.prepared[0]["path"]
        valid = read_json(self.output / name)
        with self.assertRaises(ValueError):
            self.replace(name, {**valid, "bucket": "uploads"})
            claim._static_receipts(self.output, self.additions, self.prepared, self.request)
        self.replace(name, valid)
        name = next(item["path"] for item in self.prepared if item["path"].startswith("scoped-list"))
        valid = read_json(self.output / name)
        with self.assertRaises(ValueError):
            self.replace(name, {**valid, "response": {"IsTruncated": False,
                "Contents": [{"Key": self.scope + "/.ryframe-owner"}]}})
            claim._static_receipts(self.output, self.additions, self.prepared, self.request)

    def test_owner_files_match_bucket_scope_metadata_and_bytes(self):
        self.owners()
        self.assertEqual(claim._owner_files(self.output, self.additions, self.request), set(self.additions))

    def test_owner_rejects_path_scope_bucket_metadata_and_content_changes(self):
        self.owners()
        name = next(name for name in self.additions if name.startswith("owner-read"))
        valid = read_json(self.output / name)
        for change in ({"destination": "../escape.bin"}, {"destination": "C:\\escape.bin"},
                       {"destination": valid["destination"].replace(valid["bucket"], "uploads")},
                       {"key": "other/.ryframe-owner"}, {"response": {}},
                       {"response": {**valid["response"], "ContentLength": 0}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.replace(name, {**valid, **change})
                claim._owner_files(self.output, self.additions, self.request)
        self.replace(name, valid)
        (self.output / valid["destination"]).write_bytes(b"changed")
        with self.assertRaises(ValueError):
            claim._owner_files(self.output, self.additions, self.request)


if __name__ == "__main__":
    unittest.main()
