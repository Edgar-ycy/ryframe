use super::{StepDefinition, WorkingDirectory};

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
