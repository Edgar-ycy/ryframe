use crate::{
    Result,
    build_source::{with_restore_verifier, with_source_commit},
    cli::{DataCommand, RestoreCommand},
    process::{command_output, run_owned},
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
    let run = || run_owned(&root, "cargo", &cargo_args);
    if matches!(command, DataCommand::Restore(RestoreCommand::Verify(_))) {
        let (verifier, guard) = restore_verifier(&root)?;
        return guard.finish(with_restore_verifier(&root, &source_sha, &verifier, run));
    }
    with_source_commit(&root, &source_sha, run)
}

fn restore_verifier(
    root: &std::path::Path,
) -> Result<(
    serde_json::Value,
    crate::recovery::monitoring::execution::ProtectedInputs,
)> {
    use crate::recovery::{clone::native::digest, monitoring::execution::ProtectedInputs};
    use serde_json::json;
    use std::path::PathBuf;

    let compiled_root = crate::frontend_origin::compiled_root()?;
    if compiled_root.canonicalize()? != root.canonicalize()? {
        return Err("恢复证明要求使用由当前协调工作树编译的 xtask".into());
    }
    let description = crate::frontend_origin::describe()?;
    let executable = &description["executable"];
    let path = PathBuf::from(executable["path"].as_str().ok_or("当前核验程序路径无效")?);
    let guard = ProtectedInputs::open([path.clone()])?;
    let actual = digest::file(&path)?;
    if actual.bytes == 0 || executable["sha256"] != actual.sha256 {
        return Err("当前核验程序在发放前变化".into());
    }
    Ok((
        json!({"path":path,"bytes":actual.bytes,"sha256":actual.sha256}),
        guard,
    ))
}
