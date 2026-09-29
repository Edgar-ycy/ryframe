use super::super::super::super::model::{
    CacheOperation, CacheOptions, CliError, CloneCommand, CloneRole, CloneRuntimeOperation,
    CloneRuntimeOptions, StorageOperation, StorageOptions,
};

use super::{optional_path, parse_options, require_write, required, run_dir, side};

pub(super) fn parse_runtime(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--run-dir", "--side", "--operation"], true)?;
    let operation = match required(&options, "--operation")? {
        "start" => CloneRuntimeOperation::Start,
        "stop" => CloneRuntimeOperation::Stop,
        "status" => CloneRuntimeOperation::Status,
        "recover" => CloneRuntimeOperation::Recover,
        value => return Err(CliError::new(format!("runtime --operation 无效：{value}"))),
    };
    require_write("runtime", &options, operation.effect().requires_write())?;
    Ok(CloneCommand::Runtime(CloneRuntimeOptions {
        run_dir: run_dir(&options)?,
        side: side(required(&options, "--side")?)?,
        operation,
        roles: options
            .roles
            .unwrap_or_else(|| vec![CloneRole::Api, CloneRole::Worker]),
    }))
}

pub(super) fn parse_storage(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(
        args,
        &["--run-dir", "--side", "--operation", "--request"],
        false,
    )?;
    let operation = match required(&options, "--operation")? {
        "restart" => StorageOperation::Restart,
        "status" => StorageOperation::Status,
        "stop" => StorageOperation::Stop,
        "recover" => StorageOperation::Recover,
        value => return Err(CliError::new(format!("storage --operation 无效：{value}"))),
    };
    require_write("storage", &options, operation.effect().requires_write())?;
    let request = optional_path(&options, "--request")?;
    if request.is_some() && !matches!(operation, StorageOperation::Restart) {
        return Err(CliError::new("只有 storage restart 可以提供 --request"));
    }
    Ok(CloneCommand::Storage(StorageOptions {
        run_dir: run_dir(&options)?,
        side: side(required(&options, "--side")?)?,
        operation,
        request,
    }))
}

pub(super) fn parse_cache(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--run-dir", "--operation", "--request"], false)?;
    let operation = match required(&options, "--operation")? {
        "restart" => CacheOperation::Restart,
        "status" => CacheOperation::Status,
        "stop" => CacheOperation::Stop,
        "recover" => CacheOperation::Recover,
        "reconcile" => CacheOperation::Reconcile,
        "resume" => CacheOperation::Resume,
        value => return Err(CliError::new(format!("cache --operation 无效：{value}"))),
    };
    require_write("cache", &options, operation.effect().requires_write())?;
    let request = optional_path(&options, "--request")?;
    if request.is_some() && !matches!(operation, CacheOperation::Restart) {
        return Err(CliError::new("只有 cache restart 可以提供 --request"));
    }
    Ok(CloneCommand::Cache(CacheOptions {
        run_dir: run_dir(&options)?,
        operation,
        request,
    }))
}
