use crate::{
    Result,
    build_source::{with_restore_verifier, with_source_commit},
    cli::{DataCommand, RestoreCommand, RestoreVerifyOptions},
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
    let root = root_dir();
    let cargo_args = cargo_arguments(command, &root)?;
    if let DataCommand::Restore(RestoreCommand::Verify(options)) = command {
        return run_restore_verify(options, &cargo_args);
    }
    let source_sha = command_output(&root, "git", &["rev-parse", "--verify", "HEAD^{commit}"])?
        .trim()
        .to_owned();
    with_source_commit(&root, &source_sha, || {
        run_owned(&root, "cargo", &cargo_args)
    })
}

fn cargo_arguments(
    command: &DataCommand,
    invocation_root: &std::path::Path,
) -> Result<Vec<String>> {
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
    if let DataCommand::Restore(RestoreCommand::Verify(options)) = command
        && options.config_dir.is_none()
    {
        cargo_args.extend([
            "--config-dir".into(),
            invocation_root
                .join("config")
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    cargo_args.extend(invocation.arguments);
    Ok(cargo_args)
}

fn run_restore_verify(options: &RestoreVerifyOptions, cargo_args: &[String]) -> Result<()> {
    let root = crate::frontend_origin::compiled_root()?;
    let verifier_source = clean_source(&root, "恢复证明核验源码")?;
    let coordinator = clean_source(&options.coordinator_root, "恢复证明协调源码")?;
    if coordinator["sha"] != options.coordinator_sha {
        return Err("恢复证明协调工作树与 --coordinator-sha 不一致".into());
    }
    let source_sha = verifier_source["sha"].as_str().ok_or("核验源码 SHA 无效")?;
    let (verifier, guard) = restore_verifier(&root)?;
    let result = with_restore_verifier(&root, source_sha, &coordinator, &verifier, || {
        let result = run_owned(&root, "cargo", cargo_args);
        let recheck = (|| -> Result<()> {
            let verifier_after = clean_source(&root, "恢复证明核验源码");
            let coordinator_after = clean_source(&options.coordinator_root, "恢复证明协调源码");
            if verifier_after? != verifier_source || coordinator_after? != coordinator {
                return Err("恢复证明执行期间协调或核验源码变化".into());
            }
            let description = crate::frontend_origin::describe()?;
            crate::frontend_origin::verify_compiled_sources(&root, &description["sources"])?;
            Ok(())
        })();
        match (result, recheck) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(recheck)) => {
                Err(format!("{error}；源码收尾复核失败：{recheck}").into())
            }
        }
    });
    guard.finish(result)
}

fn clean_source(root: &std::path::Path, label: &str) -> Result<serde_json::Value> {
    let top = command_output(root, "git", &["rev-parse", "--show-toplevel"])?;
    if !root.is_absolute()
        || std::path::Path::new(top.trim()).canonicalize()? != root.canonicalize()?
    {
        return Err(format!("{label}必须是精确 Git 工作树的绝对根目录").into());
    }
    let inventory = crate::recovery::clone::native::source::capture(root)?;
    if !inventory.source.snapshot.clean {
        return Err(format!("{label}必须是干净工作树").into());
    }
    Ok(serde_json::json!({"root":root,"sha":inventory.source.snapshot.head}))
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

    let description = crate::frontend_origin::describe()?;
    crate::frontend_origin::verify_compiled_sources(root, &description["sources"])?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_cargo_keeps_registry_configuration_at_the_invocation_root() {
        let root = root_dir();
        let coordinator = root.join("separate-coordinator");
        let mut options = RestoreVerifyOptions {
            id: "drill-1".into(),
            proof: "proof.json".into(),
            tests_receipt: "tests.json".into(),
            runtime_receipt: "runtime.json".into(),
            target_plan: coordinator.join(".local-tests/target.json"),
            coordinator_root: coordinator,
            coordinator_sha: "a".repeat(40),
            runner_root: root.join("runner"),
            config_dir: None,
            restore_config_dir: root.join("isolated-config"),
        };
        let args = cargo_arguments(
            &DataCommand::Restore(RestoreCommand::Verify(Box::new(options.clone()))),
            &root,
        )
        .unwrap();
        let separator = args.iter().position(|value| value == "--").unwrap();
        assert_eq!(args[separator + 1], "--config-dir");
        assert_eq!(args[separator + 2], root.join("config").to_string_lossy());
        assert_eq!(args[separator + 3], "restore-verify");
        let explicit = root.join("private-registry");
        options.config_dir = Some(explicit.clone());
        let args = cargo_arguments(
            &DataCommand::Restore(RestoreCommand::Verify(Box::new(options))),
            &root,
        )
        .unwrap();
        assert_eq!(
            args.iter().filter(|value| *value == "--config-dir").count(),
            1
        );
        assert_eq!(args[separator + 2], explicit.to_string_lossy());
    }
}
