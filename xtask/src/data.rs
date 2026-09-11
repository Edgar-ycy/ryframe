use crate::{
    Result,
    cli::DataCommand,
    process::{command_output, run_owned_with_env},
    workspace::root_dir,
};

pub(crate) fn run(command: &DataCommand) -> Result<()> {
    let (feature, binary, arguments) = match command {
        DataCommand::Backup(arguments) => (
            "bin-tenant-data",
            "ryframe-tenant-data",
            tenant_data_arguments("backup", arguments)?,
        ),
        DataCommand::Restore(arguments) => (
            "bin-tenant-data",
            "ryframe-tenant-data",
            tenant_data_arguments("restore", arguments)?,
        ),
        DataCommand::TargetInventory(arguments) => (
            "bin-tenant-data",
            "ryframe-tenant-data",
            target_inventory_arguments(arguments)?,
        ),
        DataCommand::File(arguments) => (
            "bin-file-maintenance",
            "ryframe-file-maintenance",
            arguments.clone(),
        ),
        DataCommand::Reset(arguments) => ("bin-reset", "ryframe-reset", arguments.clone()),
        DataCommand::Help | DataCommand::Migrate(_) => {
            return Err("内部 data 调度收到不适用的命令".into());
        }
    };
    let mut cargo_args = [
        "run",
        "--locked",
        "--target-dir",
        "target/xtask-data",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        feature,
        "--bin",
        binary,
        "--",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    cargo_args.extend(arguments);
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

pub(crate) fn tenant_data_arguments(kind: &str, arguments: &[String]) -> Result<Vec<String>> {
    let (operation, rest) = arguments
        .split_first()
        .ok_or("数据维护命令缺少明确子操作")?;
    let command = format!("{kind}-{operation}");
    let mut result = Vec::with_capacity(arguments.len());
    result.push(command);
    result.extend(rest.iter().cloned());
    Ok(result)
}

pub(crate) fn target_inventory_arguments(arguments: &[String]) -> Result<Vec<String>> {
    let (operation, rest) = arguments
        .split_first()
        .ok_or("目标库存命令缺少明确子操作")?;
    if operation != "inventory" {
        return Err("目标库存只支持 inventory".into());
    }
    let mut result = Vec::with_capacity(arguments.len());
    result.push("target-inventory".to_owned());
    result.extend(rest.iter().cloned());
    Ok(result)
}
