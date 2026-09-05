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

fn cargo_arguments(profile: BuildProfile) -> Vec<String> {
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

fn frontend_arguments(real: bool) -> &'static [&'static str] {
    if real {
        &["build", "--real"]
    } else {
        &["build"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_is_the_default_build_profile_contract() {
        let release = cargo_arguments(BuildProfile::Release);
        assert!(release.iter().any(|argument| argument == "--release"));
        assert!(
            release
                .windows(2)
                .any(|pair| pair == ["--target-dir", "target/build"])
        );
        assert!(
            release
                .iter()
                .any(|argument| argument == "--no-default-features")
        );
        assert!(
            release
                .windows(2)
                .any(|pair| pair == ["--features", "bin-api,bin-worker"])
        );
    }

    #[test]
    fn explicit_dev_profile_uses_cargo_default_profile() {
        let dev = cargo_arguments(BuildProfile::Dev);
        assert!(!dev.iter().any(|argument| argument == "--release"));
        assert_eq!(frontend_arguments(false), ["build"]);
        assert_eq!(frontend_arguments(true), ["build", "--real"]);
    }
}
