use crate::{
    cli::{CliError, FixtureReviewCommand, FixtureReviewOptions},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, new_directory, output_file, require_write};

const OPTIONS: &[&str] = &[
    "--template",
    "--fixture",
    "--future-root",
    "--id",
    "--api-port",
    "--worker-port",
    "--frontend-port",
    "--rustfs-api-port",
    "--rustfs-console-port",
    "--redis-port",
    "--output",
];

pub(super) fn parse(args: &[String]) -> Result<FixtureReviewCommand, CliError> {
    if matches!(args, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(FixtureReviewCommand::Help);
    }
    let options = ParsedOptions::parse(args, OPTIONS)?;
    require_write(&options, "review renew")?;
    let ports = [
        port(&options, "--api-port")?,
        port(&options, "--worker-port")?,
        port(&options, "--frontend-port")?,
        port(&options, "--rustfs-api-port")?,
        port(&options, "--rustfs-console-port")?,
        port(&options, "--redis-port")?,
    ];
    validate_ports(&ports)?;
    Ok(FixtureReviewCommand::Renew(FixtureReviewOptions {
        template: options.path("--template", LocalTestPathKind::ExistingFile)?,
        fixture: options.path("--fixture", LocalTestPathKind::ExistingFile)?,
        future_root: new_directory(options.require("--future-root")?, "--future-root")?,
        id: review_id(options.require("--id")?)?,
        api_port: ports[0],
        worker_port: ports[1],
        frontend_port: ports[2],
        rustfs_api_port: ports[3],
        rustfs_console_port: ports[4],
        redis_port: ports[5],
        output: output_file(options.require("--output")?, "--output")?,
    }))
}

fn port(options: &ParsedOptions, name: &str) -> Result<u16, CliError> {
    let value = options
        .require(name)?
        .parse::<u16>()
        .map_err(|_| CliError::new(format!("{name} 必须是 1024 至 65533 的端口基数")))?;
    if (1024..=65533).contains(&value) {
        Ok(value)
    } else {
        Err(CliError::new(format!(
            "{name} 必须是 1024 至 65533 的端口基数"
        )))
    }
}

fn validate_ports(values: &[u16; 6]) -> Result<(), CliError> {
    let mut expanded = Vec::with_capacity(12);
    for value in &values[..3] {
        expanded.extend([*value, *value + 1, *value + 2]);
    }
    expanded.extend_from_slice(&values[3..]);
    expanded.sort_unstable();
    if expanded.windows(2).any(|pair| pair[0] == pair[1]) {
        Err(CliError::new(
            "三侧 API、Worker、前端以及 RustFS、Redis 端口不能重叠",
        ))
    } else {
        Ok(())
    }
}

fn review_id(value: &str) -> Result<String, CliError> {
    if (3..=32).contains(&value.len())
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Ok(value.to_owned())
    } else {
        Err(CliError::new(
            "--id 必须以小写字母或数字开头，仅含小写字母、数字或连字符，长度 3 至 32",
        ))
    }
}
