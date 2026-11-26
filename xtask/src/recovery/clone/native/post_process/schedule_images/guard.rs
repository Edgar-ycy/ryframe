//! 两侧数据库身份、owner 和原管理员使用同一受管只读连接。

use std::path::Path;

use super::{
    CloneSide, Query, Result, StorageInputs, Transport, Value, document, model, temporary,
    verify_database,
};

pub(in crate::recovery::clone::native) fn verify_administrator(
    source: &StorageInputs<'_>,
    target: &StorageInputs<'_>,
    administrator: &Value,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    let id = document::text(&administrator["subject_id"], "原系统管理员 ID")?;
    let parsed: i64 = id.parse().map_err(|_| "原系统管理员 ID 无效")?;
    let username = document::text(&administrator["username"], "原系统管理员用户名")?;
    if parsed <= 0
        || parsed.to_string() != id
        || username.is_empty()
        || username.chars().any(char::is_control)
        || source.backend != target.backend
        || source.target_scope()? == target.target_scope()?
    {
        return Err("管理员只读核验必须绑定原管理员和两侧独立控制库".into());
    }
    let source_databases = model::databases(source)?;
    let target_databases = model::databases(target)?;
    let source_control = &source_databases["shared-control"];
    let target_control = &target_databases["shared-control"];
    if source_control["server_uuid"] == target_control["server_uuid"]
        && source_control["database"] == target_control["database"]
    {
        return Err("管理员两侧共享控制库的物理身份重叠".into());
    }
    let temporary = temporary(target)?;
    for (side, inputs, control) in [
        (CloneSide::Source, source, source_control),
        (CloneSide::Target, target, target_control),
    ] {
        let execution = inputs.execution_backend()?;
        let scope = inputs.target_scope()?;
        let query = Query::new(inputs, &execution, scope, side, temporary)?;
        let check = || {
            checkpoint()?;
            query.check(inputs, &execution)
        };
        verify_database(inputs, control, &query, scope, transport, &check)?;
        let sql = format!(
            "SELECT id,username,status,del_flag FROM sys_user WHERE tenant_id='system' AND id={parsed};"
        );
        let row = query.read(
            inputs,
            control,
            ("administrator-row", sql),
            transport,
            &check,
        )?;
        if row.split('\t').collect::<Vec<_>>() != [id, username, "1", "0"] {
            return Err("源实际管理员与复制后目标账号不匹配".into());
        }
        check()?;
    }
    Ok(())
}

pub(in crate::recovery::clone::native) fn require_disabled_schedules(
    target: &StorageInputs<'_>,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<()> {
    read_target_control(
        target,
        (
            "seed-enabled-schedules",
            "SELECT COUNT(*) FROM sys_job_schedule WHERE enabled=1 AND del_flag='0';",
        ),
        transport,
        checkpoint,
        |count| {
            if count != "0" {
                return Err("seed 仍有启用或未明确核清的调度，禁止 Worker".into());
            }
            Ok(())
        },
    )
}

/// 使用固定目标共享控制库和原 MySQL 只读窗口，SQL 由受管 Rust 调用点提供。
pub(in crate::recovery::clone::native) fn read_target_control<T>(
    target: &StorageInputs<'_>,
    (label, sql): (&'static str, &str),
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
    decode: impl FnOnce(&str) -> Result<T>,
) -> Result<T> {
    checkpoint()?;
    let execution = target.execution_backend()?;
    let scope = target.target_scope()?;
    let query = Query::new(
        target,
        &execution,
        scope,
        CloneSide::Target,
        temporary(target)?,
    )?;
    let check = || {
        checkpoint()?;
        query.check(target, &execution)
    };
    let databases = model::databases(target)?;
    let control = &databases["shared-control"];
    verify_database(target, control, &query, scope, transport, &check)?;
    let raw = query.read(target, control, (label, sql.to_owned()), transport, &check)?;
    let decoded = decode(&raw)?;
    check()?;
    Ok(decoded)
}

pub(in crate::recovery::clone::native) fn verify_source_and_target(
    source: &StorageInputs<'_>,
    target: &StorageInputs<'_>,
    transport: &impl Transport,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    let temporary = temporary(target)?;
    for (side, inputs) in [(CloneSide::Source, source), (CloneSide::Target, target)] {
        verify_databases(inputs, side, temporary, transport, checkpoint)?;
    }
    Ok(())
}

pub(in crate::recovery::clone::native) fn verify_target_databases(
    target: &StorageInputs<'_>,
    transport: &impl Transport,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    verify_databases(
        target,
        CloneSide::Target,
        temporary(target)?,
        transport,
        checkpoint,
    )
}

fn verify_databases(
    inputs: &StorageInputs<'_>,
    side: CloneSide,
    temporary: &Path,
    transport: &impl Transport,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    let execution = inputs.execution_backend()?;
    let scope = inputs.target_scope()?;
    let query = Query::new(inputs, &execution, scope, side, temporary)?;
    let check = || {
        checkpoint()?;
        query.check(inputs, &execution)
    };
    for database in model::databases(inputs)?.values() {
        verify_database(inputs, database, &query, scope, transport, &check)?;
    }
    check()
}
