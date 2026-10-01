from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class CiWorkflowTests(unittest.TestCase):
    def test_frontend_source_selection_uses_only_the_typed_entry(self) -> None:
        daily = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        extended = (ROOT / ".github/workflows/extended-ci.yml").read_text(
            encoding="utf-8"
        )
        self.assertEqual(daily.count("cargo xtask check ci frontend-source"), 5)
        self.assertEqual(extended.count("cargo xtask check ci frontend-source"), 1)
        self.assertNotIn("tools/python/select_frontend_commit.py", daily + extended)
        self.assertNotIn("--backend-worktree", daily + extended)
        for job in ("plan", "rust-gate", "resource-gate", "windows-smoke"):
            block = daily.split(f"\n  {job}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertLess(
                block.index("安装 Rust 工具链"),
                block.index("cargo xtask check ci frontend-source"),
                job,
            )
        full_stack = extended.split("\n  full-stack-e2e:\n", 1)[1]
        self.assertLess(
            full_stack.index("安装 Rust 工具链"),
            full_stack.index("cargo xtask check ci frontend-source"),
        )

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
        self.assertIn("cargo xtask check ci frontend-source", block)
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
            "cargo xtask check ci resource-gate --frontend-dir ../frontend", block
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
        windows = workflow.split("\n  windows-smoke:\n", 1)[1].split("\n  #", 1)[0]
        self.assertIn("github.event.action != 'edited'", windows)
        security = workflow.split("\n  security-audit:\n", 1)[1].split("\n  #", 1)[0]
        self.assertNotIn("github.event.action != 'edited'", security)
        self.assertIn("if: ${{ needs.plan.result == 'success' }}", security)
        resource = workflow.split("  resource-gate:", 1)[1].split("  integration:", 1)[0]
        self.assertIn("needs.plan.outputs.consumer_contract == 'true'", resource)
        self.assertIn("cargo xtask check ci consumer-contract", resource)

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
        self.assertEqual(security.count("cargo xtask check ci security source"), 1)
        self.assertNotIn("python tools/python/check_supply_chain.py", security)
        self.assertNotIn("cargo audit --deny warnings", security)
        self.assertNotIn("cargo deny check licenses bans sources", security)
        self.assertIn("tool: cargo-audit@0.22.2", security)
        self.assertIn("tool: cargo-deny@0.20.2", security)
        self.assertEqual(
            security.count("cargo xtask check ci security deployment source"), 1
        )
        self.assertEqual(
            security.count("cargo xtask check ci security deployment image"), 1
        )
        self.assertNotIn("python tools/python/check_deployment_assets.py", security)
        self.assertNotIn("git diff --quiet", security)
        self.assertNotIn("docker compose", security)
        self.assertNotIn("docker run", security)
        self.assertEqual(security.count("docker build"), 1)
        self.assertEqual(
            security.count("steps.deployment.outputs.required == 'true'"), 2
        )
        source = security.index("cargo xtask check ci security source")
        deployment_source = security.index(
            "cargo xtask check ci security deployment source"
        )
        image_build = security.index("docker build")
        deployment_image = security.index(
            "cargo xtask check ci security deployment image"
        )
        self.assertLess(source, deployment_source)
        self.assertLess(deployment_source, image_build)
        self.assertLess(image_build, deployment_image)

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
        for path in ("Cargo.lock", "Cargo.toml", "crates/**", "config/**", "migrations/**",
                     "openapi/**", "tools/**", "scripts/requirements-ci.txt", "vendor/**", "xtask/**"):
            self.assertIn(f'"{path}"', workflow)
        self.assertNotIn('"tools/python/ci_full_stack.py"', workflow.split("schedule:", 1)[0])
        for path in ("tools/python/check_supply_chain.py", "tools/policies/supply_chain_policy.json",
                     "tools/python/test_check_supply_chain.py"):
            self.assertTrue((ROOT / path).is_file())
        self.assertNotIn("check_vulnerability_exceptions.py", workflow)
        self.assertNotIn("vulnerability-exceptions.json", workflow)
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
        self.assertIn("cargo xtask check recovery fixture", block)
        self.assertNotIn("python backend/tools/python/prepare_full_stack_fixture.py", block)
        for operation in ("prepare", "start", "collect"):
            self.assertIn(
                f"cargo xtask check recovery full-stack {operation}", block
            )
        self.assertIn("cargo xtask check recovery full-stack rate-limit", block)
        self.assertIn('--environment-file "$GITHUB_ENV"', block)
        self.assertNotIn("python tools/python/ci_full_stack.py", block)
        self.assertNotIn("python tools/python/full_stack_rate_limit_config.py", block)
        collect = block.split("- name: 收集进程与基础设施日志", 1)[1].split(
            "- name: 保存全栈诊断产物", 1
        )[0]
        self.assertIn("if: ${{ always() }}", collect)
        self.assertNotIn("cargo build --locked", block)
        self.assertNotIn("ryframe-reset plan", block)
        self.assertNotIn("nohup", block)
        self.assertNotIn("curl --fail", block)
        self.assertIn("corepack pnpm build", block)
        self.assertNotIn("corepack pnpm check", block)
        self.assertIn("--override-filename ryframe-backend.cdx", block)
        self.assertNotIn("--override-filename ryframe-backend.cdx.json", block)
        self.assertIn(
            'mv crates/ryframe/ryframe-backend.cdx.json "$SBOM_PATH"', block
        )
        self.assertEqual(
            block.count("cargo xtask check ci security report cyclonedx"), 2
        )
        self.assertEqual(block.count("cargo xtask check ci security report trivy"), 1)
        self.assertNotIn("python tools/python/check_supply_chain.py", block)
        self.assertIn("--require-reproducible", block)
        self.assertEqual(block.count("if: ${{ always() }}"), 2)
        self.assertNotIn("sccache-full-stack.json", block)
        self.assertNotIn("name: sccache-full-stack-", block)
        self.assertNotIn("playwright-real", block)
        self.assertGreaterEqual(block.count("if-no-files-found: error"), 1)
        self.assertEqual(block.count("if-no-files-found: warn"), 0)

    def test_rust_gate_and_integration_use_internal_commands(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        rust_gate = workflow.split("\n  rust-gate:\n", 1)[1].split(
            "\n  integration:\n", 1
        )[0]
        self.assertIn("cargo xtask check ci frontend-source", rust_gate)
        self.assertIn("corepack pnpm install --frozen-lockfile", rust_gate)
        self.assertIn("cargo xtask check ci rust-gate --frontend-dir ../frontend", rust_gate)
        integration = workflow.split("\n  integration:\n", 1)[1].split(
            "\n  windows-smoke:\n", 1
        )[0]
        self.assertIn("cargo xtask check ci integration", integration)
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
        self.assertIn("cargo xtask check ci frontend-source", windows)
        self.assertIn("--prefer-marker", windows)
        self.assertIn("--candidate-openapi", windows)
        self.assertIn("ref: ${{ steps.windows-frontend-ref.outputs.ref }}", windows)
        self.assertIn("github.event.pull_request.head.sha || github.sha", windows)
        self.assertIn("RYFRAME_CI_FRONTEND_REF", windows)
        self.assertIn("RYFRAME_CI_RUST_GATE_PROFILE: windows-smoke", windows)
        self.assertIn(
            "cargo xtask check ci rust-gate --frontend-dir ../ryframe-vue3", windows
        )
        self.assertNotIn("cargo check --locked -p ryframe", windows)
        self.assertNotIn("--test process_windows", windows)

    def test_resource_job_keeps_formal_consumer_check_and_uses_xtask(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        consumer = workflow.split("  resource-gate:", 1)[1].split(
            "  integration:", 1
        )[0]
        self.assertIn("cargo xtask check ci frontend-source", consumer)
        self.assertIn("--candidate-openapi", consumer)
        self.assertIn("RYFRAME_CI_BACKEND_HEAD", consumer)
        self.assertIn("RYFRAME_CI_CANDIDATE_OPENAPI", consumer)
        self.assertIn("RYFRAME_CI_FRONTEND_REF", consumer)
        self.assertIn("cargo xtask check ci consumer-contract --frontend-dir ../frontend", consumer)
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
        self.assertIn("cargo xtask check ci preflight", preflight)

    def test_preflight_checks_the_exact_frontend_source(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        preflight = workflow.split("\n  plan:\n", 1)[1].split(
            "\n  rust-gate:\n", 1
        )[0]
        self.assertIn("cargo xtask check ci frontend-source", preflight)
        self.assertIn("--prefer-marker", preflight)
        self.assertIn(
            "ref: ${{ steps.preflight-frontend-ref.outputs.ref }}", preflight
        )
        self.assertIn(
            "cargo xtask check ci preflight --frontend-dir ../frontend", preflight
        )


if __name__ == "__main__":
    unittest.main()
