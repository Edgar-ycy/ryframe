use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{FixtureRetentionCommand, FixtureRetentionOptions},
    local_test_path::LocalTestPathKind,
};

use super::insert_path;

pub(super) fn fields(command: &FixtureRetentionCommand, root: &Path) -> Result<Map<String, Value>> {
    let (options, plan_sha256) = match command {
        FixtureRetentionCommand::Help => return Err("retention 帮助没有私有请求".into()),
        FixtureRetentionCommand::Inspect(options)
        | FixtureRetentionCommand::PlanHistory(options)
        | FixtureRetentionCommand::ExportBackup(options)
        | FixtureRetentionCommand::VerifyCleaned(options) => (options, None),
        FixtureRetentionCommand::HistoricalExpired {
            options,
            plan_sha256,
        } => (options, Some(plan_sha256)),
    };
    retention_fields(options, plan_sha256, root)
}

fn retention_fields(
    options: &FixtureRetentionOptions,
    plan_sha256: Option<&String>,
    root: &Path,
) -> Result<Map<String, Value>> {
    validate_values(options, plan_sha256)?;
    let mut fields = Map::new();
    insert_path(
        &mut fields,
        "runtime_dir",
        &options.runtime_dir,
        root,
        LocalTestPathKind::ExistingDirectory,
    )?;
    fields.insert("tenant".to_owned(), Value::String(options.tenant.clone()));
    fields.insert(
        "migration".to_owned(),
        Value::String(options.migration.to_string()),
    );
    if let Some(plan_sha256) = plan_sha256 {
        fields.insert("plan_sha256".to_owned(), Value::String(plan_sha256.clone()));
    }
    Ok(fields)
}

fn validate_values(options: &FixtureRetentionOptions, plan_sha256: Option<&String>) -> Result<()> {
    let valid_tenant = options.tenant.len() == 15
        && options.tenant.starts_with("tenant-")
        && options.tenant[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !valid_tenant {
        return Err("保留期租户必须符合 tenant-[a-f0-9]{8}".into());
    }
    if options.migration <= 0 {
        return Err("迁移 ID 必须是正 i64".into());
    }
    if let Some(value) = plan_sha256
        && (value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    {
        return Err("历史计划摘要必须是 64 位小写十六进制".into());
    }
    Ok(())
}
