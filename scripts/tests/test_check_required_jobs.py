from __future__ import annotations

import importlib.util
import json
import re
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check_required_jobs.py"
ROOT = SCRIPT.parents[1]
SPEC = importlib.util.spec_from_file_location("check_required_jobs", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def successful_results(event: str) -> dict[str, str]:
    results = {name: "success" for name in MODULE.ALWAYS_REQUIRED}
    results["consumer-contract"] = "success" if event == "pull_request" else "skipped"
    results["supply-chain"] = (
        "success" if event in ("schedule", "workflow_dispatch") else "skipped"
    )
    results["windows-smoke"] = "skipped" if event == "schedule" else "success"
    return results


class RequiredJobsTests(unittest.TestCase):
    def test_accepts_each_supported_event_matrix(self) -> None:
        for event in MODULE.EVENTS:
            with self.subTest(event=event):
                self.assertEqual(
                    MODULE.validate_required_jobs(event, successful_results(event)),
                    [],
                )

    def test_never_accepts_skipped_or_failed_core_job(self) -> None:
        for result in ["skipped", "failure", "cancelled"]:
            results = successful_results("pull_request")
            results["check"] = result
            with self.subTest(result=result):
                self.assertTrue(MODULE.validate_required_jobs("pull_request", results))

    def test_needs_json_is_the_single_strict_result_source(self) -> None:
        expected = successful_results("pull_request")
        needs = {
            name: {"result": result, "outputs": {}}
            for name, result in expected.items()
        }
        self.assertEqual(MODULE.parse_needs_json(json.dumps(needs)), expected)
        for invalid in (
            "[]",
            '{"check": "success"}',
            '{"check": {"outputs": {}}}',
            "not-json",
        ):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                MODULE.parse_needs_json(invalid)

    def test_unknown_or_missing_need_is_rejected(self) -> None:
        missing = successful_results("pull_request")
        missing.pop("check")
        self.assertTrue(MODULE.validate_required_jobs("pull_request", missing))

        unknown = successful_results("pull_request")
        unknown["unreviewed-job"] = "failure"
        errors = MODULE.validate_required_jobs("pull_request", unknown)
        self.assertTrue(any("未知 required job" in error for error in errors))

    def test_conditional_jobs_must_match_the_event(self) -> None:
        pull_request = successful_results("pull_request")
        pull_request["consumer-contract"] = "skipped"
        self.assertTrue(MODULE.validate_required_jobs("pull_request", pull_request))

        schedule = successful_results("schedule")
        schedule["windows-smoke"] = "success"
        self.assertTrue(MODULE.validate_required_jobs("schedule", schedule))

        push = successful_results("push")
        push["supply-chain"] = "success"
        self.assertTrue(MODULE.validate_required_jobs("push", push))

        dispatch = successful_results("workflow_dispatch")
        dispatch["supply-chain"] = "skipped"
        self.assertTrue(MODULE.validate_required_jobs("workflow_dispatch", dispatch))

    def test_workflow_invokes_the_checked_script(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("python scripts/check_required_jobs.py", workflow)
        self.assertIn("${{ toJSON(needs) }}", workflow)
        self.assertIn('--needs-json "$NEEDS_JSON"', workflow)
        required = workflow.split("  required:", 1)[1]
        self.assertNotIn("needs.check.result", required)
        self.assertNotIn("--job", required)

    def test_sccache_primary_keys_are_unique_per_job_and_run(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        keys = re.findall(r"^\s+key: (v2-sccache-.+)$", workflow, re.MULTILINE)
        self.assertEqual(len(keys), 4)
        self.assertEqual(len(set(keys)), len(keys))
        for key in keys:
            self.assertIn("${{ github.sha }}", key)
            self.assertIn("${{ github.run_id }}", key)
            self.assertIn("${{ github.run_attempt }}", key)
        for job in ("check", "integration", "consumer-contract", "windows-smoke"):
            self.assertTrue(any(f"-{job}-" in key for key in keys), job)

    def test_ci_yaml_parser_is_reinstalled_from_hashed_requirement(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertEqual(workflow.count("--requirement scripts/requirements-ci.txt"), 4)
        self.assertEqual(workflow.count("--force-reinstall"), 4)
        self.assertEqual(workflow.count("--require-hashes"), 4)
        setup_python = (
            "actions/setup-python@"
            "a309ff8b426b58ec0e2a45f0f869d46889d02405"
        )
        self.assertEqual(workflow.count(setup_python), 4)
        self.assertEqual(workflow.count('python-version: \"3.12\"'), 4)

    def test_pull_request_edits_refresh_contract_selection(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        trigger = workflow.split("  pull_request:", 1)[1].split(
            "  schedule:", 1
        )[0]
        self.assertIn(
            "types: [ opened, synchronize, reopened, edited ]",
            trigger,
        )

    def test_linux_check_runs_the_registered_feature_matrix(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        check = workflow.split("  check:", 1)[1].split("  integration:", 1)[0]
        self.assertIn("run: cargo xtask feature-matrix", check)

    def test_windows_smoke_uses_the_consumer_frontend_selector(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        windows = workflow.split("  windows-smoke:", 1)[1].split(
            "  security-audit:", 1
        )[0]
        self.assertIn("scripts/select_frontend_commit.py", windows)
        self.assertIn("--prefer-marker", windows)
        self.assertIn("ref: ${{ steps.windows-frontend-ref.outputs.ref }}", windows)
        self.assertIn("github.event.pull_request.head.sha || github.sha", windows)
        self.assertIn("git -C ryframe-vue3 rev-parse --verify HEAD", windows)
        self.assertIn("id: windows-frontend-commit", windows)
        self.assertIn('echo "commit=$resolved" >> "$GITHUB_OUTPUT"', windows)

    def test_consumer_job_checks_a_committed_formal_contract(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        consumer = workflow.split("  consumer-contract:", 1)[1].split(
            "  windows-smoke:", 1
        )[0]
        self.assertIn("pnpm consumer:check --", consumer)
        self.assertIn("--mode formal", consumer)
        self.assertIn("scripts/verify_frontend_contract_source.py", consumer)
        self.assertIn("id: formal-source", consumer)
        self.assertIn(
            "BACKEND_COMMIT: ${{ steps.formal-source.outputs.commit }}",
            consumer,
        )
        self.assertIn('--require-pin "true"', consumer)
        self.assertNotIn(
            "BACKEND_COMMIT: ${{ github.event.pull_request.head.sha }}",
            consumer,
        )
        self.assertIn("git -C frontend rev-parse --verify HEAD", consumer)
        self.assertIn("id: frontend-commit", consumer)
        self.assertIn('echo "commit=$resolved" >> "$GITHUB_OUTPUT"', consumer)

    def test_migration_history_uses_a_fetched_trusted_base(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        check = workflow.split("  check:", 1)[1].split("  integration:", 1)[0]
        self.assertIn("fetch-depth: 0", check)
        self.assertIn("github.event.pull_request.base.sha", check)
        self.assertIn("github.event.before", check)
        self.assertIn("--trusted-ref \"$trusted_ref\"", check)
        self.assertIn('trusted_ref="$(git rev-parse --verify HEAD^)"', check)
        self.assertNotIn("trusted_ref=HEAD^", check)
        self.assertNotIn("--trusted-ref \"${{ github.sha }}\"", check)


if __name__ == "__main__":
    unittest.main()
