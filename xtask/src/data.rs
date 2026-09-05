use crate::{Result, cli::DataCommand, process::run_owned, workspace::root_dir};

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
    run_owned(&root_dir(), "cargo", &cargo_args)
}

fn tenant_data_arguments(kind: &str, arguments: &[String]) -> Result<Vec<String>> {
    let (operation, rest) = arguments
        .split_first()
        .ok_or("数据维护命令缺少明确子操作")?;
    let command = format!("{kind}-{operation}");
    let mut result = Vec::with_capacity(arguments.len());
    result.push(command);
    result.extend(rest.iter().cloned());
    Ok(result)
}

fn target_inventory_arguments(arguments: &[String]) -> Result<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn maintenance_operations_map_to_existing_private_binary_commands() {
        assert_eq!(
            tenant_data_arguments("backup", &strings(&["status", "--id", "b1"])).unwrap(),
            strings(&["backup-status", "--id", "b1"])
        );
        assert_eq!(
            tenant_data_arguments("restore", &strings(&["verify", "--id", "r1"])).unwrap(),
            strings(&["restore-verify", "--id", "r1"])
        );
        assert_eq!(
            target_inventory_arguments(&strings(&["inventory", "--target", "tenant-a"])).unwrap(),
            strings(&["target-inventory", "--target", "tenant-a"])
        );
    }
}
