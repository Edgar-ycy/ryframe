use crate::{
    cli::{CliError, FixtureServiceOperation, FixtureServicesCommand, FixtureServicesOptions},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, reject_write, require_write};

pub(super) fn parse(args: &[String]) -> Result<FixtureServicesCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("fixture services 缺少明确操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureServicesCommand::Help);
    }
    if matches!(values, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(FixtureServicesCommand::Help);
    }
    let operation = service_operation(operation)?;
    let options = ParsedOptions::parse(values, &["--review", "--environment", "--owner-binding"])?;
    if matches!(operation, FixtureServiceOperation::Status) {
        reject_write(&options, "services status")?;
    } else {
        require_write(&options, "services")?;
    }
    let owner_binding =
        options.optional_path("--owner-binding", LocalTestPathKind::ExistingFile)?;
    if matches!(
        operation,
        FixtureServiceOperation::Recover | FixtureServiceOperation::Restart
    ) != owner_binding.is_some()
    {
        return Err(CliError::new(
            "services recover/restart 必须指定 --owner-binding，其他操作不接受该参数",
        ));
    }
    Ok(FixtureServicesCommand::Run(FixtureServicesOptions {
        operation,
        review: options.path("--review", LocalTestPathKind::ExistingFile)?,
        environment: options.path("--environment", LocalTestPathKind::ExistingFile)?,
        owner_binding,
    }))
}

fn service_operation(value: &str) -> Result<FixtureServiceOperation, CliError> {
    match value {
        "rustfs" => Ok(FixtureServiceOperation::Rustfs),
        "redis" => Ok(FixtureServiceOperation::Redis),
        "buckets" => Ok(FixtureServiceOperation::Buckets),
        "status" => Ok(FixtureServiceOperation::Status),
        "close" => Ok(FixtureServiceOperation::Close),
        "recover" => Ok(FixtureServiceOperation::Recover),
        "restart" => Ok(FixtureServiceOperation::Restart),
        _ => Err(CliError::new(format!(
            "未知 fixture services 操作：{value}"
        ))),
    }
}
