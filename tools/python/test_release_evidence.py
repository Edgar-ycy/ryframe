import sys
import copy
import hashlib
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_evidence import (
    BUSINESS_OUTPUTS,
    EvidenceError,
    Requirement,
    latest_run,
    validate_fixture,
    validate_pair,
    validate_run,
)
from verify_release_ci import await_evidence


class ReleaseEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.requirement = Requirement("owner/backend", "ci.yml", "a" * 40, ("Required",))
        self.run = {"id": 123, "run_number": 5, "run_attempt": 2, "head_sha": "a" * 40,
                    "status": "completed", "conclusion": "success", "event": "push", "head_branch": "v0.13.0"}
        self.job = {"id": 42, "name": "Required", "run_id": 123, "run_attempt": 2,
                    "head_sha": "a" * 40, "status": "completed", "conclusion": "success"}

    def fixture_receipt(self):
        empty = hashlib.sha256(b"").hexdigest()
        fixture_hash = hashlib.sha256(b"business fixture").hexdigest()
        sources = {
            "backend": {"head": "a" * 40, "patch_sha256": empty, "files": []},
            "frontend": {"head": "b" * 40, "patch_sha256": empty, "files": []},
        }
        files = []
        for path in BUSINESS_OUTPUTS["backend"]:
            digest = (
                fixture_hash
                if path == "crates/order-business/src/resources/mod.rs"
                else hashlib.sha256(path.encode()).hexdigest()
            )
            files.append({"path": path, "sha256": digest})
        generated = {
            "backend": {
                "head": "a" * 40,
                "patch_sha256": hashlib.sha256(b"generated-backend").hexdigest(),
                "files": files,
            },
            "frontend": sources["frontend"],
        }
        return {
            "format_version": 1,
            "fixture": "business",
            "status": "ready",
            "fixture_sha256": fixture_hash,
            "sources": sources,
            "generated": generated,
        }

    def test_success_keeps_run_attempt_and_job_identity(self):
        result = validate_run(self.run, [self.job], self.requirement, jobs_attempt=2)
        self.assertEqual((result["run_id"], result["attempt"]), (123, 2))

    def test_jobs_endpoint_binds_attempt_when_response_omits_it(self):
        job = {key: value for key, value in self.job.items() if key != "run_attempt"}
        self.assertEqual(validate_run(self.run, [job], self.requirement, jobs_attempt=2)["attempt"], 2)
        with self.assertRaises(EvidenceError):
            validate_run(self.run, [job], self.requirement, jobs_attempt=1)

    def test_fail_closed_for_all_unsuccessful_results(self):
        for outcome in ("failure", "cancelled", "timed_out", "skipped", "neutral", None):
            with self.subTest(outcome=outcome), self.assertRaises(EvidenceError):
                validate_run({**self.run, "conclusion": outcome}, [self.job], self.requirement, jobs_attempt=2)

    def test_job_must_be_unique_current_and_successful(self):
        cases = [[self.job, self.job]]
        for field, value in (("head_sha", "b" * 40), ("run_attempt", 1), ("run_id", 99),
                             ("conclusion", "skipped"), ("conclusion", "failure")):
            cases.append([{**self.job, field: value}])
        for jobs in cases:
            with self.subTest(jobs=jobs), self.assertRaises(EvidenceError):
                validate_run(self.run, jobs, self.requirement, jobs_attempt=2)

    def test_stale_jobs_snapshot_waits_for_required_job_conclusion(self):
        self.assertIsNone(validate_run(self.run, [], self.requirement, jobs_attempt=2))
        pending = {**self.job, "conclusion": None}
        self.assertIsNone(validate_run(self.run, [pending], self.requirement, jobs_attempt=2))
        queued = {**self.job, "status": "in_progress", "conclusion": None}
        self.assertIsNone(validate_run(self.run, [queued], self.requirement, jobs_attempt=2))

    def test_pending_runs_wait(self):
        for status in ("queued", "in_progress", "waiting"):
            self.assertIsNone(validate_run({**self.run, "status": status}, [], self.requirement, jobs_attempt=2))

    def test_wrong_sha_and_tag_rejected(self):
        with self.assertRaises(EvidenceError):
            validate_run({**self.run, "head_sha": "b" * 40}, [self.job], self.requirement, jobs_attempt=2)
        tagged = Requirement("owner/backend", "extended-ci.yml", "a" * 40, ("Required",), "v0.13.1")
        self.assertIsNone(latest_run([self.run], tagged))
        with self.assertRaises(EvidenceError):
            validate_run(self.run, [self.job], tagged, jobs_attempt=2)

    def test_old_success_never_masks_new_attempt_or_new_run_failure(self):
        old = {**self.run, "run_attempt": 1}
        failed = {**self.run, "conclusion": "failure"}
        self.assertEqual(latest_run([old, failed], self.requirement), failed)
        new = {**failed, "id": 124, "run_number": 6, "run_attempt": 1}
        self.assertEqual(latest_run([self.run, new], self.requirement), new)

    def test_pair_receipt_cannot_reuse_another_attempt_or_frontend(self):
        evidence = validate_run(self.run, [self.job], self.requirement, jobs_attempt=2)
        receipt = {
            "format_version": 1,
            "backend_sha": "a" * 40,
            "frontend_sha": "b" * 40,
            "run_id": 123,
            "attempt": 2,
            "sources": self.fixture_receipt()["sources"],
        }
        validate_pair(receipt, evidence, "a" * 40, "b" * 40)
        for field, value in (("attempt", 1), ("run_id", 124), ("frontend_sha", "c" * 40)):
            with self.subTest(field=field), self.assertRaises(EvidenceError):
                validate_pair({**receipt, field: value}, evidence, "a" * 40, "b" * 40)
        dirty = copy.deepcopy(receipt)
        dirty["sources"]["frontend"]["patch_sha256"] = "c" * 64
        with self.assertRaises(EvidenceError):
            validate_pair(dirty, evidence, "a" * 40, "b" * 40)

    def test_missing_run_times_out_and_eventual_completion_succeeds(self):
        times = iter((0, 1, 2))
        with self.assertRaises(EvidenceError):
            await_evidence([self.requirement], 1, lambda _: None, lambda _: None, lambda: next(times))
        results = iter((None, {"run_id": 123}))
        self.assertEqual(await_evidence([self.requirement], 3, lambda _: next(results), lambda _: None, lambda: 0), [{"run_id": 123}])

    def test_business_receipt_requires_clean_paired_sources_and_completed_generation(self):
        receipt = self.fixture_receipt()
        expected_fixture = receipt["fixture_sha256"]
        validate_fixture(receipt, "a" * 40, "b" * 40, expected_fixture)
        for key, value in (
            ("status", "preparing"),
            ("fixture", "core"),
            ("fixture_sha256", hashlib.sha256(b"").hexdigest()),
            ("sources", None),
            ("generated", {}),
        ):
            with self.subTest(key=key), self.assertRaises(EvidenceError):
                validate_fixture(
                    {**receipt, key: value},
                    "a" * 40,
                    "b" * 40,
                    expected_fixture,
                )
        for name in ("backend", "frontend"):
            for key, value in (
                ("head", "c" * 40),
                ("files", [{"path": "dirty.rs"}]),
                ("patch_sha256", "b" * 64),
            ):
                changed = copy.deepcopy(receipt)
                changed["sources"][name][key] = value
                with self.subTest(name=name, key=key), self.assertRaises(EvidenceError):
                    validate_fixture(
                        changed, "a" * 40, "b" * 40, expected_fixture
                    )

        with self.assertRaisesRegex(EvidenceError, "发布源码不一致"):
            validate_fixture(receipt, "a" * 40, "b" * 40, "d" * 64)

    def test_business_receipt_binds_backend_outputs_and_keeps_frontend_unchanged(self):
        receipt = self.fixture_receipt()
        expected_fixture = receipt["fixture_sha256"]
        for name in ("backend",):
            missing = copy.deepcopy(receipt)
            missing["generated"][name]["files"].pop()
            with self.subTest(name=name, case="missing"), self.assertRaisesRegex(
                EvidenceError, "缺少必要生成输出"
            ):
                validate_fixture(
                    missing, "a" * 40, "b" * 40, expected_fixture
                )

            unchanged = copy.deepcopy(receipt)
            unchanged["generated"][name] = unchanged["sources"][name]
            with self.subTest(name=name, case="unchanged"), self.assertRaises(EvidenceError):
                validate_fixture(
                    unchanged, "a" * 40, "b" * 40, expected_fixture
                )

        wrong_fixture = copy.deepcopy(receipt)
        model = next(
            entry
            for entry in wrong_fixture["generated"]["backend"]["files"]
            if entry["path"] == "crates/order-business/src/resources/mod.rs"
        )
        model["sha256"] = "c" * 64
        with self.assertRaisesRegex(EvidenceError, "资源定义不一致"):
            validate_fixture(
                wrong_fixture, "a" * 40, "b" * 40, expected_fixture
            )

        changed_frontend = copy.deepcopy(receipt)
        changed_frontend["generated"]["frontend"] = {
            "head": "b" * 40,
            "patch_sha256": "c" * 64,
            "files": [{"path": "src/generated/resources/order/page.vue", "sha256": "d" * 64}],
        }
        with self.assertRaisesRegex(EvidenceError, "不得改写前端"):
            validate_fixture(
                changed_frontend, "a" * 40, "b" * 40, expected_fixture
            )


if __name__ == "__main__":
    unittest.main()
