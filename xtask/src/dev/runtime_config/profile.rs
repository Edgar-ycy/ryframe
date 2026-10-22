use std::path::Path;

use ryframe_config::{AppConfig, JobWorkerMode, LoggerOutput, MigrationMode};

use super::RuntimeProfile;
use crate::Result;

pub(super) fn validate(profile: RuntimeProfile, locales: &Path) -> Result<()> {
    if profile.api_port == 0 || profile.worker_port == 0 || profile.api_port == profile.worker_port
    {
        return Err("角色 API 与 Worker 端口必须非零且互异；请修正 api_port 和 worker_port".into());
    }
    if profile.snowflake_worker_id > 1023 {
        return Err("角色 snowflake_worker_id 必须在 0..=1023 范围内；请指定有效实例编号".into());
    }
    let text = locales
        .to_str()
        .ok_or("角色 locales 路径必须是有效 UTF-8 文本；请指定可编码的绝对路径")?;
    if !locales.is_absolute()
        || text
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\r' | b'\n'))
    {
        return Err("角色 locales 路径必须绝对且不包含 NUL、CR 或 LF；请修正最终本地化路径".into());
    }
    Ok(())
}

pub(super) fn apply(
    overlay: &mut toml::Table,
    profile: RuntimeProfile,
    locales: &Path,
) -> Result<()> {
    put(
        overlay,
        &[],
        "snowflake_worker_id",
        i64::from(profile.snowflake_worker_id).into(),
    )?;
    put(
        overlay,
        &["app"],
        "port",
        i64::from(profile.api_port).into(),
    )?;
    put(
        overlay,
        &["jobs"],
        "health_port",
        i64::from(profile.worker_port).into(),
    )?;
    put(
        overlay,
        &["app"],
        "locales_dir",
        locales.to_str().ok_or("locales 路径不是 UTF-8")?.into(),
    )?;
    put(overlay, &["database"], "migration_mode", "verify".into())?;
    put(overlay, &["jobs"], "mode", "external".into())?;
    if profile.worker || profile.probe {
        let worker_id = if profile.probe {
            "ryframe-dev-probe"
        } else {
            "ryframe-dev-worker"
        };
        put(overlay, &["jobs"], "worker_id", worker_id.into())?;
    }
    if profile.probe {
        put(overlay, &["jobs"], "health_host", "127.0.0.1".into())?;
        put(overlay, &["app"], "host", "127.0.0.1".into())?;
        put(overlay, &["telemetry"], "enabled", false.into())?;
        put(overlay, &["logger"], "output", "stdout".into())?;
    }
    Ok(())
}

fn put(table: &mut toml::Table, path: &[&str], key: &str, value: toml::Value) -> Result<()> {
    let mut current = table;
    for segment in path {
        current = current
            .entry((*segment).to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or("角色覆盖要求配置分组为 TOML 表")?;
    }
    current.insert(key.to_owned(), value);
    Ok(())
}

pub(super) fn verify(app: &AppConfig, profile: RuntimeProfile, locales: &Path) -> Result<()> {
    if app.app.port != profile.api_port
        || app.jobs.health_port != profile.worker_port
        || app.snowflake_worker_id != i64::from(profile.snowflake_worker_id)
        || app.app.locales_dir != locales
        || app.database.migration_mode != MigrationMode::Verify
        || app.jobs.mode != JobWorkerMode::External
    {
        return Err("角色配置最终值与指定端口、编号、路径或运行模式不一致".into());
    }
    if (profile.worker || profile.probe)
        && app.jobs.worker_id.as_deref()
            != Some(if profile.probe {
                "ryframe-dev-probe"
            } else {
                "ryframe-dev-worker"
            })
    {
        return Err("角色 Worker 标识与指定运行模式不一致".into());
    }
    if profile.probe
        && (app.app.host != "127.0.0.1"
            || app.jobs.health_host != "127.0.0.1"
            || app.telemetry.enabled
            || app.logger.output != LoggerOutput::Stdout)
    {
        return Err("probe 角色配置未保持隔离的监听、遥测及日志设置".into());
    }
    Ok(())
}
