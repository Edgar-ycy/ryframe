use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::DateTime;

#[path = "data/path.rs"]
mod path;

use path::{DataPathRole, validate_data_path};

use super::super::model::{
    BackupCommand, BackupInventoryOptions, BackupObservation, BackupRegisterOptions, CliError,
    DataCommand, FILE_APPLY_CONFIRMATION, FILE_DEFAULT_BATCH_SIZE, FILE_DEFAULT_START_AFTER,
    FILE_MAX_BATCH_SIZE, FileMaintenanceMode, FileMaintenanceOperation, FileMaintenanceOptions,
    ResetCommand, ResetExecuteOptions, RestoreBeginOptions, RestoreCommand, RestoreDataOptions,
    RestoreVerifyOptions, TargetInventoryOptions,
};

pub(super) fn parse_maintenance_data(args: &[String]) -> Result<DataCommand, CliError> {
    let Some((kind, rest)) = args.split_first() else {
        return Ok(DataCommand::Help);
    };
    match kind.as_str() {
        "backup" => parse_backup(rest).map(DataCommand::Backup),
        "restore" => parse_restore(rest).map(DataCommand::Restore),
        "target" => parse_target(rest).map(DataCommand::TargetInventory),
        "file" => parse_file(rest).map(DataCommand::File),
        "reset" => parse_reset(rest).map(DataCommand::Reset),
        _ => Err(CliError::new(format!("未知 data 子任务：{kind}"))),
    }
}

fn parse_backup(args: &[String]) -> Result<BackupCommand, CliError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(CliError::new("data backup 缺少明确子操作"));
    };
    match operation.as_str() {
        "inventory" => {
            let mut options = NamedOptions::parse(rest)?;
            let output = options.take_output("--output")?;
            let source_sha = options.take_scalar("--source-sha")?;
            validate_commit_sha(&source_sha)?;
            let quiesced_at = options.take_optional_scalar("--quiesced-at")?;
            let observed_at = options.take_optional_scalar("--observed-at")?;
            let observation = match (quiesced_at, observed_at) {
                (Some(value), None) => {
                    validate_rfc3339(&value)?;
                    BackupObservation::QuiescedAt(value)
                }
                (None, Some(value)) => {
                    validate_rfc3339(&value)?;
                    BackupObservation::ObservedAt(value)
                }
                _ => {
                    return Err(CliError::new(
                        "--quiesced-at 与 --observed-at 必须且只能提供一个",
                    ));
                }
            };
            options.finish("data backup inventory")?;
            Ok(BackupCommand::Inventory(BackupInventoryOptions {
                output,
                source_sha,
                observation,
            }))
        }
        "register" => {
            let mut options = NamedOptions::parse(rest)?;
            let manifest = options.take_file("--manifest")?;
            let backup_root = options.take_directory("--backup-root")?;
            options.require_write()?;
            options.finish("data backup register")?;
            Ok(BackupCommand::Register(BackupRegisterOptions {
                manifest,
                backup_root,
            }))
        }
        "status" => {
            require_no_arguments(rest, "data backup status")?;
            Ok(BackupCommand::Status)
        }
        _ => Err(CliError::new(format!(
            "未知 data backup 子操作：{operation}"
        ))),
    }
}

fn parse_restore(args: &[String]) -> Result<RestoreCommand, CliError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(CliError::new("data restore 缺少明确子操作"));
    };
    let mut options = NamedOptions::parse(rest)?;
    match operation.as_str() {
        "begin" => {
            let command = RestoreCommand::Begin(RestoreBeginOptions {
                plan: options.take_file("--plan")?,
                output: options.take_output("--output")?,
                restore_config_dir: options.take_directory("--restore-config-dir")?,
            });
            options.require_write()?;
            options.finish("data restore begin")?;
            Ok(command)
        }
        "verify-data" => {
            let command = RestoreCommand::VerifyData(RestoreDataOptions {
                id: options.take_scalar("--id")?,
                backup_root: options.take_directory("--backup-root")?,
                output: options.take_output("--output")?,
                restore_config_dir: options.take_directory("--restore-config-dir")?,
            });
            options.require_write()?;
            options.finish("data restore verify-data")?;
            Ok(command)
        }
        "verify" => {
            let command = RestoreCommand::Verify(RestoreVerifyOptions {
                id: options.take_scalar("--id")?,
                proof: options.take_file("--proof")?,
                tests_receipt: options.take_file("--tests-receipt")?,
                runtime_receipt: options.take_file("--runtime-receipt")?,
                target_plan: options.take_file("--target-plan")?,
                runner_root: options.take_directory("--runner-root")?,
                restore_config_dir: options.take_directory("--restore-config-dir")?,
            });
            options.require_write()?;
            options.finish("data restore verify")?;
            Ok(command)
        }
        _ => Err(CliError::new(format!(
            "未知 data restore 子操作：{operation}"
        ))),
    }
}

fn parse_target(args: &[String]) -> Result<TargetInventoryOptions, CliError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(CliError::new("data target 缺少明确子操作"));
    };
    if operation != "inventory" {
        return Err(CliError::new(format!(
            "未知 data target 子操作：{operation}"
        )));
    }
    let mut options = NamedOptions::parse(rest)?;
    let target = options.take_scalar("--target")?;
    validate_target_key(&target)?;
    let command = TargetInventoryOptions {
        target,
        output: options.take_output("--output")?,
    };
    options.finish("data target inventory")?;
    Ok(command)
}

fn parse_file(args: &[String]) -> Result<FileMaintenanceOptions, CliError> {
    let Some((operation, args)) = args.split_first() else {
        return Err(CliError::new(file_usage()));
    };
    let operation = match operation.as_str() {
        "backfill-sha256" => FileMaintenanceOperation::BackfillSha256,
        "drain-legacy-reservations" => FileMaintenanceOperation::DrainLegacyReservations,
        _ => return Err(CliError::new(format!("未知 data file 子操作：{operation}"))),
    };
    let Some((mode, rest)) = args.split_first() else {
        return Err(CliError::new(file_usage()));
    };
    let mode = match mode.as_str() {
        "dry-run" => FileMaintenanceMode::DryRun,
        "apply" => FileMaintenanceMode::Apply,
        _ => return Err(CliError::new(file_usage())),
    };
    let mut options = NamedOptions::parse(rest)?;
    let database = options.take_scalar("--database")?;
    let batch_size = options
        .take_optional_scalar("--batch-size")?
        .map(|value| parse_batch_size(&value))
        .transpose()?
        .unwrap_or(FILE_DEFAULT_BATCH_SIZE);
    let start_after = options
        .take_optional_signed("--start-after")?
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| CliError::new("--start-after 必须是 i64 文件 ID"))
        })
        .transpose()?
        .unwrap_or(FILE_DEFAULT_START_AFTER);
    let confirmation = options.take_optional_scalar("--confirm-apply")?;
    let write = options.take_flag("--write");
    match (mode, confirmation.as_deref(), write) {
        (FileMaintenanceMode::DryRun, None, false) => {}
        (FileMaintenanceMode::DryRun, _, _) => {
            return Err(CliError::new("dry-run 不接受 --write 或 --confirm-apply"));
        }
        (FileMaintenanceMode::Apply, Some(FILE_APPLY_CONFIRMATION), true) => {}
        (FileMaintenanceMode::Apply, _, _) => {
            return Err(CliError::new(format!(
                "apply 必须同时提供 --write 和 --confirm-apply {FILE_APPLY_CONFIRMATION}"
            )));
        }
    }
    options.finish("data file")?;
    Ok(FileMaintenanceOptions {
        operation,
        mode,
        database,
        batch_size,
        start_after,
    })
}

fn parse_reset(args: &[String]) -> Result<ResetCommand, CliError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(CliError::new("data reset 缺少明确子操作"));
    };
    match operation.as_str() {
        "plan" => {
            require_no_arguments(rest, "data reset plan")?;
            Ok(ResetCommand::Plan)
        }
        "execute" => {
            let mut options = NamedOptions::parse(rest)?;
            let plan_hash = options.take_scalar("--plan-hash")?;
            validate_sha256(&plan_hash)?;
            let confirmation = options.take_scalar("--confirm-reset")?;
            options.require_write()?;
            options.finish("data reset execute")?;
            Ok(ResetCommand::Execute(ResetExecuteOptions {
                plan_hash,
                confirmation,
            }))
        }
        _ => Err(CliError::new(format!(
            "未知 data reset 子操作：{operation}"
        ))),
    }
}

fn parse_batch_size(value: &str) -> Result<u64, CliError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| CliError::new("--batch-size 必须是正整数"))?;
    if !(1..=FILE_MAX_BATCH_SIZE).contains(&parsed) {
        return Err(CliError::new(format!(
            "--batch-size 必须在 1..={FILE_MAX_BATCH_SIZE} 之间"
        )));
    }
    Ok(parsed)
}

fn validate_commit_sha(value: &str) -> Result<(), CliError> {
    if value.len() == 40
        && value.bytes().any(|byte| byte != b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(CliError::new(
            "--source-sha 必须是 40 位非零小写十六进制 Git SHA",
        ))
    }
}

fn validate_sha256(value: &str) -> Result<(), CliError> {
    if value.len() == 64
        && value.bytes().any(|byte| byte != b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(CliError::new(
            "--plan-hash 必须是 64 位非零小写十六进制 SHA-256",
        ))
    }
}

fn validate_rfc3339(value: &str) -> Result<(), CliError> {
    DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| CliError::new("库存观察时间必须是 RFC3339"))
}

fn validate_target_key(value: &str) -> Result<(), CliError> {
    let bytes = value.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_alphanumeric();
    if (2..=64).contains(&bytes.len())
        && bytes.first().is_some_and(|byte| is_alphanumeric(*byte))
        && bytes.last().is_some_and(|byte| is_alphanumeric(*byte))
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(CliError::new(
            "--target 必须是 2..=64 位 ASCII 字母、数字、- 或 _，且首尾为字母或数字",
        ))
    }
}

fn require_no_arguments(args: &[String], command: &str) -> Result<(), CliError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CliError::new(format!("{command} 不接受额外参数")))
    }
}

fn file_usage() -> &'static str {
    "cargo xtask data file <backfill-sha256|drain-legacy-reservations> <dry-run|apply> --database <名称> [--batch-size <1..1000>] [--start-after <ID>] [--write --confirm-apply APPLY-FILE-A-MAINTENANCE]"
}

struct NamedOptions {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl NamedOptions {
    fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let mut index = 0;
        while index < args.len() {
            let option = &args[index];
            if !option.starts_with("--") {
                return Err(CliError::new(format!("参数必须使用明确选项：{option}")));
            }
            if option == "--write" {
                if !flags.insert(option.clone()) {
                    return Err(CliError::new("--write 不能重复"));
                }
                index += 1;
                continue;
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
            validate_scalar(value, option)?;
            if values.insert(option.clone(), value.clone()).is_some() {
                return Err(CliError::new(format!("{option} 不能重复")));
            }
            index += 2;
        }
        Ok(Self { values, flags })
    }

    fn take_scalar(&mut self, option: &str) -> Result<String, CliError> {
        self.take_optional_scalar(option)?
            .ok_or_else(|| CliError::new(format!("缺少 {option}")))
    }

    fn take_optional_scalar(&mut self, option: &str) -> Result<Option<String>, CliError> {
        let value = self.values.remove(option);
        if value.as_deref().is_some_and(|value| value.starts_with('-')) {
            return Err(CliError::new(format!("{option} 缺少取值")));
        }
        Ok(value)
    }

    fn take_optional_signed(&mut self, option: &str) -> Result<Option<String>, CliError> {
        Ok(self.values.remove(option))
    }

    fn take_path(&mut self, option: &str, role: DataPathRole) -> Result<PathBuf, CliError> {
        let path = self.take_scalar(option).map(PathBuf::from)?;
        validate_data_path(path, option, role)
    }

    fn take_file(&mut self, option: &str) -> Result<PathBuf, CliError> {
        self.take_path(option, DataPathRole::ControlledInputFile)
    }

    fn take_directory(&mut self, option: &str) -> Result<PathBuf, CliError> {
        self.take_path(option, DataPathRole::ExternalDirectory)
    }

    fn take_output(&mut self, option: &str) -> Result<PathBuf, CliError> {
        self.take_path(option, DataPathRole::ControlledNewOutput)
    }

    fn take_flag(&mut self, option: &str) -> bool {
        self.flags.remove(option)
    }

    fn require_write(&mut self) -> Result<(), CliError> {
        if self.take_flag("--write") {
            Ok(())
        } else {
            Err(CliError::new(
                "该操作会发布记录或修改业务资源，必须显式提供 --write",
            ))
        }
    }

    fn finish(self, command: &str) -> Result<(), CliError> {
        let unknown = self.values.into_keys().chain(self.flags).next();
        match unknown {
            None => Ok(()),
            Some(option) => Err(CliError::new(format!("{command} 存在不适用参数：{option}"))),
        }
    }
}

fn validate_scalar(value: &str, option: &str) -> Result<(), CliError> {
    if value.trim().is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n'))
    {
        Err(CliError::new(format!("{option} 包含非法取值")))
    } else {
        Ok(())
    }
}
