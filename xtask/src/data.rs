use crate::{
    Result,
    cli::DataCommand,
    process::{command_output, run_owned_with_env},
    workspace::root_dir,
};

#[path = "data/arguments.rs"]
pub(crate) mod arguments;
#[path = "data/performance_identities.rs"]
pub(crate) mod performance_identities;

use arguments::invocation;

pub(crate) fn run(command: &DataCommand) -> Result<()> {
    if let DataCommand::PerformanceIdentities(command) = command {
        return performance_identities::run(command);
    }
    let invocation = invocation(command).ok_or("内部 data 调度收到不适用的命令")?;
    let mut cargo_args = [
        "run",
        "--locked",
        "--target-dir",
        "target/xtask-data",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        invocation.feature,
        "--bin",
        invocation.binary,
        "--",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    cargo_args.extend(invocation.arguments);
    let root = root_dir();
    let source_sha = command_output(&root, "git", &["rev-parse", "--verify", "HEAD^{commit}"])?
        .trim()
        .to_owned();
    if source_sha.len() != 40
        || !source_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("data 维护程序的后端源码提交无效".into());
    }
    run_owned_with_env(
        &root,
        "cargo",
        &cargo_args,
        &[("RYFRAME_BUILD_COMMIT", source_sha)],
    )
}
