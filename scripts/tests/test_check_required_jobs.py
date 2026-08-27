from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check_required_jobs.py"
ROOT = SCRIPT.parents[1]
SPEC = importlib.util.spec_from_file_location("check_required_jobs", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def plan_outputs(**overrides: str) -> dict[str, str]:
    outputs = {
        "preflight": "true",
        "rust_gate": "true",
        "integration": "true",
        "consumer_contract": "false",
    }
    outputs.update(overrides)
    return outputs


def successful_results(
    event: str,
    action: str,
    outputs: dict[str, str],
    git_ref: str = "",
) -> dict[str, str]:
    results = {name: "skipped" for name in MODULE.ALL_JOBS}
    results["plan"] = "success"
    for job, output in MODULE.PLAN_CONTROLLED_JOBS.items():
        enabled = outputs[output] == "true"
        if job == "consumer-contract" and event != "pull_request":
            enabled = False
        results[job] = "success" if enabled else "skipped"
    if not (event == "pull_request" and action == "edited"):
        results["security-audit"] = "success"
        results["deployment-assets"] = "success"
        results["windows-smoke"] = "skipped" if event == "schedule" else "success"
        results["supply-chain"] = (
            "success" if event in ("schedule", "workflow_dispatch") else "skipped"
        )
        results["full-stack-e2e"] = (
            "success"
            if event in ("schedule", "workflow_dispatch")
            or (event == "push" and git_ref.startswith("refs/tags/v"))
            else "skipped"
        )
    return results


class RequiredJobsTests(unittest.TestCase):
    def test_accepts_dynamic_event_plans(self) -> None:
        cases = (
            ("push", "", plan_outputs()),
            ("schedule", "", plan_outputs()),
            ("workflow_dispatch", "", plan_outputs()),
            (
                "pull_request",
                "synchronize",
                plan_outputs(consumer_contract="true"),
            ),
            (
                "pull_request",
                "synchronize",
                plan_outputs(rust_gate="false", integration="false"),
            ),
            (
                "pull_request",
                "edited",
                plan_outputs(
                    preflight="false",
                    rust_gate="false",
                    integration="false",
                    consumer_contract="true",
                ),
            ),
        )
        for event, action, outputs in cases:
            with self.subTest(event=event, action=action, outputs=outputs):
                results = successful_results(event, action, outputs)
                self.assertEqual(
                    MODULE.validate_required_jobs(event, action, results, outputs),
                    [],
                )

    def test_plan_and_enabled_jobs_must_succeed(self) -> None:
        outputs = plan_outputs(consumer_contract="true")
        for job in ("plan", "preflight", "rust-gate", "integration", "consumer-contract"):
            for result in ("skipped", "failure", "cancelled"):
                results = successful_results("pull_request", "synchronize", outputs)
                results[job] = result
                with self.subTest(job=job, result=result):
                    self.assertTrue(
                        MODULE.validate_required_jobs(
                            "pull_request", "synchronize", results, outputs
                        )
                    )

    def test_full_stack_runs_only_for_scheduled_manual_or_release_tag(self) -> None:
        outputs = plan_outputs()
        for event, git_ref, expected in (
            ("schedule", "refs/heads/main", "success"),
            ("workflow_dispatch", "refs/heads/main", "success"),
            ("push", "refs/tags/v1.2.3", "success"),
            ("push", "refs/heads/main", "skipped"),
            ("pull_request", "refs/pull/1/merge", "skipped"),
        ):
            with self.subTest(event=event, git_ref=git_ref):
                results = successful_results(event, "", outputs, git_ref)
                self.assertEqual(results["full-stack-e2e"], expected)
                self.assertEqual(
                    MODULE.validate_required_jobs(event, "", results, outputs, git_ref),
                    [],
                )

    def test_skipped_plan_job_must_match_the_output(self) -> None:
        outputs = plan_outputs(rust_gate="false", integration="false")
        results = successful_results("pull_request", "synchronize", outputs)
        results["rust-gate"] = "success"
        self.assertTrue(
            MODULE.validate_required_jobs(
                "pull_request", "synchronize", results, outputs
            )
        )

    def test_pull_request_edit_accepts_only_contract_plan(self) -> None:
        invalid_outputs = plan_outputs(consumer_contract="true")
        results = successful_results("pull_request", "edited", invalid_outputs)
        errors = MODULE.validate_required_jobs(
            "pull_request", "edited", results, invalid_outputs
        )
        self.assertTrue(any("edited" in error for error in errors))

    def test_plan_outputs_are_complete_boolean_strings(self) -> None:
        outputs = plan_outputs()
        results = successful_results("push", "", outputs)
        for invalid in (
            {**outputs, "rust_gate": "yes"},
            {name: value for name, value in outputs.items() if name != "rust_gate"},
            {**outputs, "unknown": "false"},
        ):
            with self.subTest(invalid=invalid):
                self.assertTrue(MODULE.validate_required_jobs("push", "", results, invalid))

    def test_needs_json_is_the_single_strict_result_and_plan_source(self) -> None:
        outputs = plan_outputs(consumer_contract="true")
        results = successful_results("pull_request", "synchronize", outputs)
        needs = {
            name: {
                "result": result,
                "outputs": outputs if name == "plan" else {},
            }
            for name, result in results.items()
        }
        encoded = json.dumps(needs)
        self.assertEqual(MODULE.parse_needs_json(encoded), results)
        self.assertEqual(MODULE.parse_plan_outputs(encoded), outputs)
        for invalid in (
            "[]",
            '{"plan": "success"}',
            '{"plan": {"result": "success"}}',
            "not-json",
        ):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                MODULE.parse_plan_outputs(invalid)

    def test_unknown_or_missing_need_is_rejected(self) -> None:
        outputs = plan_outputs()
        missing = successful_results("push", "", outputs)
        missing.pop("rust-gate")
        self.assertTrue(MODULE.validate_required_jobs("push", "", missing, outputs))
        unknown = successful_results("push", "", outputs)
        unknown["unreviewed-job"] = "failure"
        errors = MODULE.validate_required_jobs("push", "", unknown, outputs)
        self.assertTrue(any("未知 required job" in error for error in errors))

    def test_workflow_uses_plan_outputs_and_checked_required_script(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        plan = workflow.split("\n  plan:\n", 1)[1].split("\n  preflight:\n", 1)[0]
        self.assertIn("cargo xtask ci plan", plan)
        for output in MODULE.PLAN_OUTPUTS:
            self.assertIn(f"{output}: ${{{{ steps.plan.outputs.{output} }}}}", plan)
        for job, output in MODULE.PLAN_CONTROLLED_JOBS.items():
            block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn(f"needs.plan.outputs.{output} == 'true'", block)
        required = workflow.split("  required:", 1)[1]
        self.assertIn("python scripts/check_required_jobs.py", required)
        self.assertIn("${{ toJSON(needs) }}", required)
        self.assertIn('--action "$EVENT_ACTION"', required)
        self.assertIn('--ref "$GIT_REF"', required)
        self.assertNotIn("needs.plan.result", required)

    def test_pull_request_edit_skips_every_job_except_plan_contract_required(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for job in ("windows-smoke", "security-audit", "supply-chain", "deployment-assets"):
            block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn("github.event.action != 'edited'", block)
        consumer = workflow.split("  consumer-contract:", 1)[1].split(
            "  windows-smoke:", 1
        )[0]
        self.assertIn("github.event_name == 'pull_request'", consumer)

    def test_sccache_uses_fixed_remote_backend_without_directory_cache(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertEqual(workflow.count("tool: sccache@0.17.0"), 5)
        self.assertEqual(workflow.count('SCCACHE_GHA_ENABLED: "true"'), 5)
        self.assertEqual(workflow.count('CARGO_INCREMENTAL: "0"'), 5)
        self.assertNotIn("RUSTFLAGS:", workflow)
        self.assertNotIn("SCCACHE_BASEDIRS:", workflow)
        self.assertNotIn("SCCACHE_DIR:", workflow)
        self.assertNotIn("SCCACHE_CACHE_SIZE:", workflow)
        self.assertNotIn("v2-sccache-", workflow)
        self.assertEqual(workflow.count("--show-stats --stats-format=json"), 5)
        self.assertEqual(workflow.count("### sccache ·"), 5)
        self.assertEqual(workflow.count("name: sccache-"), 5)
        self.assertEqual(
            workflow.count(
                "actions/github-script@3a2844b7e9c422d3c10d287c895573f7108da1b3"
            ),
            5,
        )

    def test_full_stack_uses_isolated_reset_and_always_uploads_diagnostics(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        block = workflow.split("\n  full-stack-e2e:\n", 1)[1].split(
            "\n  security-audit:\n", 1
        )[0]
        self.assertIn("github.event_name == 'schedule'", block)
        self.assertIn("github.event_name == 'workflow_dispatch'", block)
        self.assertIn("startsWith(github.ref, 'refs/tags/v')", block)
        self.assertIn("APP_ENV: test", block)
        self.assertIn('APP_RESET_LEGACY_MYSQL_EXCLUSIVE: "true"', block)
        self.assertIn(
            'APP_OBJECT_STORAGE_LOCAL_BASE_DIR=$RUNNER_TEMP/ryframe-full-stack/storage',
            block,
        )
        self.assertIn(
            'RYFRAME_RESET_STATE_DIR=$RUNNER_TEMP/ryframe-full-stack/reset-state',
            block,
        )
        self.assertNotIn("APP_OBJECT_STORAGE_LOCAL_BASE_DIR: ${{ runner.temp }}", block)
        self.assertNotIn("RYFRAME_RESET_STATE_DIR: ${{ runner.temp }}", block)
        self.assertIn("python scripts/ci_full_stack.py prepare", block)
        self.assertIn("python scripts/ci_full_stack.py start", block)
        self.assertIn("python scripts/ci_full_stack.py collect", block)
        self.assertNotIn("cargo build --locked", block)
        self.assertNotIn("ryframe-reset plan", block)
        self.assertNotIn("nohup", block)
        self.assertNotIn("curl --fail", block)
        self.assertIn("pnpm ci:browser-real", block)
        self.assertEqual(block.count("if: ${{ always() }}"), 4)
        self.assertIn("sccache-full-stack.json", block)
        self.assertIn("name: sccache-full-stack-", block)
        self.assertIn("frontend/.local-tests/playwright-real/report", block)
        self.assertIn("frontend/.local-tests/playwright-real/results", block)
        self.assertEqual(block.count("if-no-files-found: error"), 1)
        self.assertEqual(block.count("if-no-files-found: warn"), 1)

    def test_rust_gate_and_integration_use_internal_commands(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        rust_gate = workflow.split("\n  rust-gate:\n", 1)[1].split(
            "\n  integration:\n", 1
        )[0]
        self.assertIn("scripts/select_frontend_commit.py", rust_gate)
        self.assertIn("pnpm install --frozen-lockfile", rust_gate)
        self.assertIn("cargo xtask ci rust-gate --frontend-dir ../frontend", rust_gate)
        integration = workflow.split("\n  integration:\n", 1)[1].split(
            "\n  consumer-contract:\n", 1
        )[0]
        self.assertIn("cargo xtask ci integration", integration)
        self.assertNotIn("cargo test --locked -p ryframe-db", integration)
        self.assertIn("path: backend", integration)
        self.assertIn("working-directory: backend", integration)
        self.assertIn("hashFiles('backend/Cargo.lock', 'backend/Cargo.toml')", integration)

    def test_windows_smoke_preserves_exact_frontend_selection(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        windows = workflow.split("  windows-smoke:", 1)[1].split(
            "  security-audit:", 1
        )[0]
        self.assertIn("scripts/select_frontend_commit.py", windows)
        self.assertIn("--prefer-marker", windows)
        self.assertIn("--candidate-openapi", windows)
        self.assertIn("ref: ${{ steps.windows-frontend-ref.outputs.ref }}", windows)
        self.assertIn("github.event.pull_request.head.sha || github.sha", windows)
        self.assertIn("RYFRAME_CI_FRONTEND_REF", windows)
        self.assertIn("RYFRAME_CI_RUST_GATE_PROFILE: windows-smoke", windows)
        self.assertIn(
            "cargo xtask ci rust-gate --frontend-dir ../ryframe-vue3", windows
        )
        self.assertNotIn("cargo check --locked -p ryframe", windows)
        self.assertNotIn("--test process_windows", windows)

    def test_consumer_job_keeps_formal_source_check_and_uses_xtask(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        consumer = workflow.split("  consumer-contract:", 1)[1].split(
            "  windows-smoke:", 1
        )[0]
        self.assertIn("scripts/select_frontend_commit.py", consumer)
        self.assertIn("--candidate-openapi", consumer)
        self.assertIn("RYFRAME_CI_BACKEND_HEAD", consumer)
        self.assertIn("RYFRAME_CI_CANDIDATE_OPENAPI", consumer)
        self.assertIn("RYFRAME_CI_FRONTEND_REF", consumer)
        self.assertIn("cargo xtask ci consumer-contract --frontend-dir ../frontend", consumer)
        self.assertNotIn("pnpm consumer:check --", consumer)
        self.assertNotIn("cargo run --locked", consumer)
        self.assertNotIn("cmp --silent", consumer)

    def test_ci_yaml_parser_is_reinstalled_from_hashed_requirement(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertEqual(workflow.count("--requirement scripts/requirements-ci.txt"), 4)
        self.assertEqual(workflow.count("--force-reinstall"), 4)
        self.assertEqual(workflow.count("--require-hashes"), 4)

    def test_preflight_passes_the_fetched_trusted_base_to_xtask(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        preflight = workflow.split("\n  preflight:\n", 1)[1].split(
            "\n  rust-gate:\n", 1
        )[0]
        self.assertIn("fetch-depth: 0", preflight)
        self.assertIn("github.event.pull_request.base.sha", preflight)
        self.assertIn("github.event.before", preflight)
        self.assertIn("cargo xtask ci preflight", preflight)


if __name__ == "__main__":
    unittest.main()
