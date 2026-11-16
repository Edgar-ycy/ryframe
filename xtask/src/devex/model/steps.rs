use super::{StepDefinition, WorkingDirectory};

pub(super) const RUST_BUILD_WORKSPACE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["build", "--locked", "--workspace"],
}];
pub(super) const RUST_CHECK_WORKSPACE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["check", "--locked", "--workspace"],
}];
pub(super) const RUST_CHECK_APPLICATION: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["check", "--locked", "-p", "ryframe-application"],
}];
pub(super) const RUST_BUILD_API: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-api",
        "--bin",
        "ryframe",
    ],
}];
pub(super) const RUST_BUILD_WORKER: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-worker",
        "--bin",
        "ryframe-worker",
    ],
}];
pub(super) const RUST_BUILD_MIGRATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-migrate",
        "--bin",
        "ryframe-migrate",
    ],
}];
pub(super) const RUST_CHECK_API: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "check",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-api",
        "--bin",
        "ryframe",
    ],
}];
pub(super) const RUST_CHECK_WORKER: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "check",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-worker",
        "--bin",
        "ryframe-worker",
    ],
}];
pub(super) const RUST_CHECK_MIGRATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "check",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-migrate",
        "--bin",
        "ryframe-migrate",
    ],
}];
pub(super) const CARGO_DEV_SAVE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "xtask",
        "dev",
        "--measure-once",
        "--workspace-root",
        "{backend}",
        "--case",
        "{variant}",
        "--output",
        "{target}/cargo-dev-save-result.json",
    ],
}];
pub(super) const RESOURCE_GENERATOR_ALL: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}",
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
        "generate",
        "resource",
        "--all",
        "--check",
        "--frontend-dir",
        "{frontend}",
    ],
}];
pub(super) const RESOURCE_GENERATOR_POST: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}",
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
        "generate",
        "resource",
        "post",
        "--check",
        "--frontend-dir",
        "{frontend}",
    ],
}];
pub(super) const RESOURCE_GENERATOR_NOTICE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}",
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
        "generate",
        "resource",
        "notice",
        "--check",
        "--frontend-dir",
        "{frontend}",
    ],
}];
pub(super) const RESOURCE_GATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}/driver",
        "-p",
        "xtask",
        "--",
        "check",
        "ci",
        "resource-gate",
        "--targeted",
        "--decision-file",
        "{target}/resource-gate-decision-{label}.json",
        "--jobs",
        "12",
        "--test-jobs",
        "8",
        "--frontend-dir",
        "{frontend}",
    ],
}];
pub(super) const RUST_GATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}/driver",
        "-p",
        "xtask",
        "--",
        "check",
        "ci",
        "rust-gate",
        "--frontend-dir",
        "{frontend}",
    ],
}];
pub(super) const FRONTEND_FAST: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "corepack",
    args: &["pnpm", "check"],
}];
pub(super) const FRONTEND_FAST_STABLE_READINESS_B0: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "corepack",
    args: &["pnpm", "check:fast"],
}];
const COMMON_FRONTEND_FAST: &str = "{runner-frontend}/scripts/devex-fast.mjs";
pub(super) const FRONTEND_FAST_COMMON_B0: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "node",
    args: &[
        COMMON_FRONTEND_FAST,
        "--root",
        "{frontend}",
        "--arm",
        "b0",
        "--cache-root",
        "{frontend-cache}",
    ],
}];
pub(super) const FRONTEND_FAST_COMMON_B1: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "node",
    args: &[
        COMMON_FRONTEND_FAST,
        "--root",
        "{frontend}",
        "--arm",
        "b1",
        "--cache-root",
        "{frontend-cache}",
    ],
}];

pub(in crate::devex) fn is_common_frontend_fast(step: &StepDefinition) -> bool {
    step.program == "node" && step.args.first() == Some(&COMMON_FRONTEND_FAST)
}

pub(super) const FRONTEND_BUILD: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "corepack",
    args: &[
        "pnpm",
        "exec",
        "vite",
        "build",
        "--outDir",
        "{target}/frontend/dist",
        "--emptyOutDir",
    ],
}];
