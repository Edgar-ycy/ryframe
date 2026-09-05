use std::path::Path;

use crate::{
    Result,
    cli::{BuildOptions, BuildProfile},
    process::{run_owned, run_pnpm},
    workspace::root_dir,
};

pub(crate) fn run(options: BuildOptions, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    run_owned(&root, "cargo", &cargo_arguments(options.profile))?;
    run_pnpm(frontend_dir, frontend_arguments(options.real))
}

pub(crate) fn cargo_arguments(profile: BuildProfile) -> Vec<String> {
    let mut arguments = [
        "build",
        "--locked",
        "--target-dir",
        "target/build",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-api,bin-worker",
        "--bin",
        "ryframe",
        "--bin",
        "ryframe-worker",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if profile == BuildProfile::Release {
        arguments.push("--release".to_owned());
    }
    arguments
}

pub(crate) fn frontend_arguments(real: bool) -> &'static [&'static str] {
    if real {
        &["build", "--real"]
    } else {
        &["build"]
    }
}
