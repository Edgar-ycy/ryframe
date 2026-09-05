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
        "resource_gate": "true",
        "integration": "true",
        "consumer_contract": "false",
    }
    outputs.update(overrides)
    return outputs


def successful_results(
    event: str,
    action: str,
    outputs: dict[str, str],
) -> dict[str, str]:
    results = {name: "skipped" for name in MODULE.ALL_JOBS}
    results["plan"] = "success"
    for job, output in MODULE.PLAN_CONTROLLED_JOBS.items():
        enabled = outputs[output] == "true"
        results[job] = "success" if enabled else "skipped"
    results["resource-gate"] = (
        "success"
        if outputs["resource_gate"] == "true"
        or outputs["consumer_contract"] == "true"
        else "skipped"
    )
    if not (event == "pull_request" and action == "edited"):
        results["security-audit"] = "success"
        results["windows-smoke"] = "success"
    return results


class RequiredJobsTests(unittest.TestCase):
    def test_accepts_dynamic_event_plans(self) -> None:
        cases = (
            ("push", "", plan_outputs()),
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
                    resource_gate="false",
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
        for job in (
            "plan",
            "rust-gate",
            "resource-gate",
            "integration",
        ):
            for result in ("skipped", "failure", "cancelled"):
                results = successful_results("pull_request", "synchronize", outputs)
                results[job] = result
                with self.subTest(job=job, result=result):
                    self.assertTrue(
                        MODULE.validate_required_jobs(
                            "pull_request", "synchronize", results, outputs
                        )
                    )

    def test_skipped_plan_job_must_match_the_output(self) -> None:
        for output, job in (
            ("rust_gate", "rust-gate"),
            ("resource_gate", "resource-gate"),
        ):
            outputs = plan_outputs(**{output: "false"})
            results = successful_results("pull_request", "synchronize", outputs)
            results[job] = "success"
            with self.subTest(job=job):
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
        plan = workflow.split("\n  plan:\n", 1)[1].split("\n  rust-gate:\n", 1)[0]
        self.assertIn("cargo xtask ci plan", plan)
        for output in MODULE.PLAN_OUTPUTS:
            self.assertIn(f"{output}: ${{{{ steps.plan.outputs.{output} }}}}", plan)
        for job, output in MODULE.PLAN_CONTROLLED_JOBS.items():
            block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn(f"needs.plan.outputs.{output} == 'true'", block)
        resource = workflow.split("\n  resource-gate:\n", 1)[1].split("\n  #", 1)[0]
        self.assertIn("needs.plan.outputs.resource_gate == 'true'", resource)
        self.assertIn("needs.plan.outputs.consumer_contract == 'true'", resource)
        required = workflow.split("  required:", 1)[1]
        self.assertIn("python scripts/check_required_jobs.py", required)
        self.assertIn("${{ toJSON(needs) }}", required)
        self.assertIn('--action "$EVENT_ACTION"', required)
        self.assertNotIn("GIT_REF", required)
        self.assertNotIn("--ref", required)
        self.assertNotIn("needs.plan.result", required)

    def test_resource_gate_owns_its_git_range_and_frontend_checkout(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        block = workflow.split("\n  resource-gate:\n", 1)[1].split("\n  #", 1)[0]
        self.assertIn("needs.plan.outputs.resource_gate == 'true'", block)
        self.assertIn('RYFRAME_MYSQL_INTEGRATION: "1"', block)
        self.assertIn('RYFRAME_MYSQL_TLS_INTEGRATION: "1"', block)
        self.assertIn('RYFRAME_REDIS_INTEGRATION: "1"', block)
        self.assertIn("mysql:8.4.11@sha256:", block)
        self.assertIn("redis:7.4.9@sha256:", block)
        self.assertIn("fetch-depth: 0", block)
        self.assertIn("path: backend", block)
        self.assertIn(
            "ref: ${{ github.event.pull_request.head.sha || github.sha }}", block
        )
        self.assertIn("scripts/select_frontend_commit.py", block)
        self.assertIn("--prefer-marker", block)
        self.assertIn("--fallback-main-on-invalid-base", block)
        self.assertIn("path: frontend", block)
        self.assertIn(
            "steps.contract-frontend-ref.outputs.ref || "
            "steps.resource-frontend-ref.outputs.ref",
            block,
        )
        self.assertIn("node-version-file: frontend/.node-version", block)
        self.assertIn("uses: ./frontend/.github/actions/setup-pnpm", block)
        self.assertIn("corepack pnpm install --frozen-lockfile", block)
        self.assertIn(
            "RYFRAME_CI_BASE_SHA: "
            "${{ github.event.pull_request.base.sha || github.event.before }}",
            block,
        )
        self.assertIn(
            "RYFRAME_CI_HEAD_SHA: "
            "${{ github.event.pull_request.head.sha || github.sha }}",
            block,
        )
        self.assertIn(
            "RYFRAME_CI_FRONTEND_REF: ${{ steps.contract-frontend-ref.outputs.ref || "
            "steps.resource-frontend-ref.outputs.ref }}",
            block,
        )
        self.assertIn("working-directory: backend", block)
        self.assertIn(
            "cargo xtask ci resource-gate --frontend-dir ../frontend", block
        )
        self.assertNotIn("RYFRAME_CI_CHANGED_PATHS", block)
        self.assertIn(
            "RYFRAME_RESOURCE_GATE_TARGETED: replay-verified-v1",
            block,
        )
        self.assertNotIn("--resource", block)
        required = workflow.split("\n  required:\n", 1)[1]
        self.assertIn("- resource-gate", required)

    def test_pull_request_edit_reuses_resource_job_for_contract(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for job in ("windows-smoke", "security-audit"):
            block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn("github.event.action != 'edited'", block)
        resource = workflow.split("  resource-gate:", 1)[1].split("  integration:", 1)[0]
        self.assertIn("needs.plan.outputs.consumer_contract == 'true'", resource)
        self.assertIn("cargo xtask ci consumer-contract", resource)

    def test_sccache_only_wraps_compile_heavy_jobs_with_persisted_local_cache(self) -> None:
        daily = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        extended = (ROOT / ".github/workflows/extended-ci.yml").read_text(
            encoding="utf-8"
        )
        workflow = daily + extended
        self.assertEqual(daily.count('CARGO_INCREMENTAL: "0"'), 1)
        self.assertEqual(extended.count('CARGO_INCREMENTAL: "0"'), 1)
        self.assertNotIn("RUSTFLAGS:", workflow)
        self.assertNotIn("v2-sccache-", workflow)
        for job in ("rust-gate", "windows-smoke"):
            block = daily.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn("tool: sccache@0.17.0", block, job)
            self.assertIn("缓存 sccache 编译产物", block, job)
            self.assertIn("SCCACHE_DIR:", block, job)
            self.assertIn("SCCACHE_CACHE_SIZE:", block, job)
        for job in ("resource-gate", "integration"):
            block = daily.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertNotIn("RUSTC_WRAPPER: sccache", block, job)
            self.assertNotIn("tool: sccache@0.17.0", block, job)
            self.assertNotIn("sccache JSON", block, job)
        rust_gate = daily.split("\n  rust-gate:\n", 1)[1].split("\n  #", 1)[0]
        self.assertIn(".cache/sccache/rust-gate", rust_gate)
        self.assertIn("SCCACHE_CACHE_SIZE: 3G", rust_gate)
        windows = daily.split("\n  windows-smoke:\n", 1)[1].split("\n  #", 1)[0]
        self.assertIn(".cache/sccache/windows-smoke", windows)
        self.assertIn("SCCACHE_CACHE_SIZE: 2G", windows)
        self.assertNotIn("RUSTC_WRAPPER: sccache", extended)
        self.assertNotIn("tool: sccache@0.17.0", extended)
        self.assertNotIn("sccache JSON", extended)
        self.assertNotIn("SCCACHE_GHA_", workflow)
        self.assertNotIn("ACTIONS_RUNTIME_TOKEN", workflow)
        self.assertNotIn('cat "$stats_file"', workflow)

    def test_linux_rust_policy_jobs_use_the_fixed_backend_checkout(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for job in ("plan", "security-audit"):
            block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn("path: backend", block, job)
            self.assertIn("defaults:\n      run:\n        working-directory: backend", block, job)
        preflight = workflow.split("\n  plan:\n", 1)[1].split("\n  #", 1)[0]
        self.assertIn(
            "args: backend/.github/workflows/ci.yml "
            "backend/.github/workflows/extended-ci.yml "
            "backend/.github/workflows/release.yml",
            preflight,
        )
        security = workflow.split("\n  security-audit:\n", 1)[1].split(
            "\n  #", 1
        )[0]
        self.assertIn("--verify-cargo-graph", security)
        self.assertIn("$GITHUB_WORKSPACE/backend/deploy/nginx", security)
        self.assertIn("$GITHUB_WORKSPACE/backend/deploy/prometheus", security)
        self.assertNotIn("$GITHUB_WORKSPACE/deploy/", security)

    def test_completed_aws_lc_canary_is_not_retained(self) -> None:
        workflows = "".join(
            path.read_text(encoding="utf-8")
            for path in (ROOT / ".github/workflows").glob("*.yml")
        )
        self.assertNotIn("aws-lc-sccache-canary:", workflows)
        self.assertFalse((ROOT / "scripts/evaluate_sccache_canary.py").exists())

    def test_full_stack_uses_isolated_reset_and_always_uploads_diagnostics(self) -> None:
        workflow = (ROOT / ".github/workflows/extended-ci.yml").read_text(
            encoding="utf-8"
        )
        block = workflow.split("\n  full-stack-e2e:\n", 1)[1]
        self.assertIn("schedule:", workflow)
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("branches: [ main ]", workflow)
        self.assertIn('tags: [ "v*.*.*" ]', workflow)
        self.assertIn('"Dockerfile"', workflow)
        self.assertIn('"scripts/ci_full_stack.py"', workflow)
        self.assertIn('"scripts/check_deployment_assets.py"', workflow)
        self.assertIn("APP_ENV: test", block)
        self.assertIn('APP_API_DOCS_ENABLED: "false"', block)
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
        self.assertIn("corepack pnpm check --stage browser --real", block)
        self.assertIn("--override-filename ryframe-backend.cdx", block)
        self.assertNotIn("--override-filename ryframe-backend.cdx.json", block)
        self.assertIn(
            'mv crates/ryframe/ryframe-backend.cdx.json "$SBOM_PATH"', block
        )
        self.assertEqual(block.count("if: ${{ always() }}"), 2)
        self.assertNotIn("sccache-full-stack.json", block)
        self.assertNotIn("name: sccache-full-stack-", block)
        self.assertIn("frontend/.local-tests/playwright-real/report", block)
        self.assertIn("frontend/.local-tests/playwright-real/results", block)
        self.assertGreaterEqual(block.count("if-no-files-found: error"), 1)
        self.assertEqual(block.count("if-no-files-found: warn"), 0)

    def test_rust_gate_and_integration_use_internal_commands(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        rust_gate = workflow.split("\n  rust-gate:\n", 1)[1].split(
            "\n  integration:\n", 1
        )[0]
        self.assertIn("scripts/select_frontend_commit.py", rust_gate)
        self.assertIn("corepack pnpm install --frozen-lockfile", rust_gate)
        self.assertIn("cargo xtask ci rust-gate --frontend-dir ../frontend", rust_gate)
        integration = workflow.split("\n  integration:\n", 1)[1].split(
            "\n  windows-smoke:\n", 1
        )[0]
        self.assertIn("cargo xtask ci integration", integration)
        self.assertIn('RYFRAME_MYSQL_TLS_INTEGRATION: "1"', integration)
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

    def test_resource_job_keeps_formal_consumer_check_and_uses_xtask(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        consumer = workflow.split("  resource-gate:", 1)[1].split(
            "  integration:", 1
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
        workflow = "".join(
            (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            for name in ("ci.yml", "extended-ci.yml")
        )
        self.assertEqual(workflow.count("--requirement scripts/requirements-ci.txt"), 3)
        self.assertEqual(workflow.count("--force-reinstall"), 3)
        self.assertEqual(workflow.count("--require-hashes"), 3)

    def test_preflight_passes_the_fetched_trusted_base_to_xtask(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        preflight = workflow.split("\n  plan:\n", 1)[1].split(
            "\n  rust-gate:\n", 1
        )[0]
        self.assertIn("fetch-depth: 0", preflight)
        self.assertIn("github.event.pull_request.base.sha", preflight)
        self.assertIn("github.event.before", preflight)
        self.assertIn("cargo xtask ci preflight", preflight)


if __name__ == "__main__":
    unittest.main()
