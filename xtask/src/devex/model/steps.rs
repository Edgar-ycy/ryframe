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
    args: &["xtask", "dev", "--measure-once"],
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
