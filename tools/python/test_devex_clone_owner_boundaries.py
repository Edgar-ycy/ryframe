"""单对象范围不削减完整阶段的其他桶 ownership；离线替身，无服务请求。"""
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_export_verify as source_verify
import devex_clone_live_object as target_verify
from restore_reference_plan import BUCKETS
import test_devex_clone_export_verify as source_fixtures
import test_devex_clone_object_read_concurrency as target_fixtures


class CompleteOwnerBoundaryTests(unittest.TestCase):
    def source_case(self, phase):
        case = source_fixtures.ExportVerificationTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        bad_bucket = sorted(BUCKETS - {"uploads"})[0]
        calls, original = 0, case.run_read

        def read(command, **kwargs):
            nonlocal calls
            result = original(command, **kwargs)
            key, bucket = command[command.index("--key") + 1] if "--key" in command else "", command[command.index("--bucket") + 1]
            if key.endswith("/.ryframe-owner") and bucket == bad_bucket:
                calls += 1
                if calls == (1 if phase == "before" else 2):
                    Path(command[-1]).write_bytes(b"changed-other-bucket-owner")
            return result

        case.tools.run = read
        with self.assertRaises(source_verify.SourceObjectObservationError):
            case.observe()
        failure = json.loads((case.observation / "failure.json").read_text(encoding="utf-8"))
        self.assertEqual(failure["stage"], "owners_" + phase)
        self.assertFalse((case.observation / "observation.json").exists())
        self.assertEqual(calls, 1 if phase == "before" else 2)

    def test_source_whole_stage_rejects_other_bucket_owner_at_entry(self):
        self.source_case("before")

    def test_source_whole_stage_rejects_other_bucket_owner_at_exit(self):
        self.source_case("after")

    def target_case(self, phase):
        case = target_fixtures.ConcurrentTargetHeadTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        bad_bucket = sorted(BUCKETS - {"uploads"})[0]
        if phase == "before":
            case.owner.owner_failure = bad_bucket
        else:
            case.on_head = lambda _: setattr(case.owner, "owner_failure", bad_bucket)
        with patch.object(target_verify, "verify_capture", case.verify), self.assertRaisesRegex(ValueError, "ownership"):
            case.execute()
        self.assertFalse((case.output / "verified.json").exists())
        self.assertTrue((case.output / "failure.json").is_file())
        self.assertEqual(len(case.started), 0 if phase == "before" else len(case.items))
        self.assertEqual(case.active, 0)

    def test_target_whole_heads_reject_other_bucket_owner_before_any_head(self):
        self.target_case("before")

    def test_target_whole_heads_reject_other_bucket_owner_after_all_heads(self):
        self.target_case("after")


if __name__ == "__main__":
    unittest.main()
