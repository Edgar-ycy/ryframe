use crate::ports::backup::*;
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use std::collections::BTreeSet;

fn require(condition: bool, message: &str) -> AppResult<()> {
    if condition {
        Ok(())
    } else {
        Err(AppError::Validation(message.into()))
    }
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn validate_backup_manifest(
    manifest: &BackupManifest,
    required: &[String],
    now: DateTime<Utc>,
) -> AppResult<()> {
    require(
        identifier(&manifest.id, 64) && identifier(&manifest.scope_id, 48),
        "备份集或 scope 标识无效",
    )?;
    require(hex(&manifest.source_sha, 40), "备份源码必须使用精确 SHA")?;
    require(
        hex(&manifest.control_schema_fingerprint, 16)
            && hex(&manifest.tenant_schema_fingerprint, 64),
        "schema 指纹格式无效",
    )?;
    require(
        manifest.quiesced_at <= manifest.captured_at
            && manifest.captured_at <= manifest.completed_at
            && manifest.completed_at <= now,
        "暂停写入、采集、备份完成与当前时间的顺序无效",
    )?;
    require(
        manifest.retention_until >= manifest.captured_at + Duration::days(7)
            && manifest.retention_until > now,
        "外部备份必须仍有效，并至少保留采集后的 7 天",
    )?;
    let keys = manifest.resource_keys();
    let unique = keys.iter().cloned().collect::<BTreeSet<_>>();
    require(
        !required.is_empty()
            && keys.len() == unique.len()
            && unique == required.iter().cloned().collect(),
        "备份必须完整覆盖全部必需目标和对象范围，且不能重复",
    )?;
    require(
        manifest.databases.len() <= 1024
            && !manifest.databases.is_empty()
            && manifest
                .databases
                .iter()
                .filter(|db| {
                    matches!(
                        db.kind,
                        BackupDatabaseKind::Control | BackupDatabaseKind::Combined
                    )
                })
                .count()
                == 1,
        "备份必须恰好包含一个控制库",
    )?;
    for database in &manifest.databases {
        validate_database(database)?;
    }
    for objects in &manifest.objects {
        validate_objects(objects, &manifest.scope_id)?;
    }
    validate_artifacts(&manifest.artifacts, &unique)
}

fn validate_database(database: &DatabaseBackup) -> AppResult<()> {
    let mut tenants = BTreeSet::new();
    require(
        database.shared || database.placements.len() <= 1,
        "独立目标只能包含一个租户",
    )?;
    for placement in &database.placements {
        require(
            identifier(&placement.tenant_id, 64)
                && placement.generation > 0
                && identifier(&placement.switch_token, 64)
                && tenants.insert(&placement.tenant_id),
            "备份租户 placement 无效或重复",
        )?;
    }
    require(
        identifier(&database.key, 64)
            && identifier(&database.database, 64)
            && identifier(&database.server_uuid, 64),
        "数据库物理身份无效",
    )?;
    require(
        !database.tables.is_empty() && database.tables.len() <= 4096,
        "数据库表清单为空或过大",
    )?;
    let mut names = BTreeSet::new();
    for table in &database.tables {
        require(
            identifier(&table.table, 64) && names.insert(&table.table) && hex(&table.sha256, 64),
            "数据表名称重复或校验信息无效",
        )?;
    }
    Ok(())
}

fn validate_objects(objects: &ObjectBackup, scope_id: &str) -> AppResult<()> {
    require(
        identifier(&objects.bucket, 63) && objects.prefix == format!("{scope_id}/"),
        "对象范围必须绑定备份 scope",
    )?;
    let mut keys = BTreeSet::new();
    require(
        objects.entries.len() <= 100_000,
        "对象清单超过单个备份集上限",
    )?;
    for entry in &objects.entries {
        require(
            entry.key.starts_with(&objects.prefix)
                && entry.key.len() <= 1024
                && !entry.key.chars().any(char::is_control)
                && keys.insert(&entry.key)
                && hex(&entry.sha256, 64),
            "对象键越界、重复或校验信息无效",
        )?;
    }
    Ok(())
}

fn validate_artifacts(artifacts: &[BackupArtifact], required: &BTreeSet<String>) -> AppResult<()> {
    let mut resources = BTreeSet::new();
    let mut paths = BTreeSet::new();
    require(artifacts.len() <= 8192, "备份文件清单过大")?;
    for artifact in artifacts {
        require(
            required.contains(&artifact.resource)
                && artifact.bytes > 0
                && hex(&artifact.sha256, 64),
            "备份文件缺少有效范围、大小或校验信息",
        )?;
        let path = &artifact.relative_path;
        require(
            !path.is_empty()
                && path.len() <= 512
                && !path.contains(['\\', ':'])
                && !path.chars().any(char::is_control)
                && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                && paths.insert(path),
            "备份文件必须使用唯一、不可越界的相对路径",
        )?;
        resources.insert(artifact.resource.clone());
    }
    require(&resources == required, "部分必需资源没有备份文件")
}

pub fn validate_restore_plan(
    backup: &BackupRecord,
    plan: &RestorePlan,
    now: DateTime<Utc>,
) -> AppResult<()> {
    require(
        identifier(&plan.id, 64)
            && identifier(&plan.scope_id, 48)
            && plan.scope_id != backup.manifest.scope_id
            && plan.backup_id == backup.manifest.id,
        "恢复必须使用独立 scope 并绑定已登记备份集",
    )?;
    require(
        backup.valid && backup.manifest.retention_until > now,
        "备份已失效或不在保留期内",
    )?;
    require(
        plan.fault_at >= backup.manifest.captured_at
            && plan.fault_at <= now
            && (plan.fault_at - backup.manifest.captured_at).num_seconds() <= 86_400,
        "备份恢复点不满足故障前 24 小时目标",
    )?;
    require(hex(&plan.frontend_sha, 40), "恢复验收需要精确前端 SHA")?;
    require(
        plan.object_prefix == format!("{}/", plan.scope_id) && !plan.object_endpoint.is_empty(),
        "恢复对象范围必须使用独立 scope",
    )?;
    require(
        !plan.api_ready_url.is_empty()
            && !plan.worker_ready_url.is_empty()
            && plan.api_ready_url != plan.worker_ready_url,
        "必须分别提供 API 与 external Worker 就绪端点",
    )?;
    validate_restore_databases(&backup.manifest.databases, &plan.databases)
}

fn validate_restore_databases(
    source: &[DatabaseBackup],
    targets: &[RestoreDatabase],
) -> AppResult<()> {
    require(
        source.len() == targets.len(),
        "恢复目标必须完整覆盖备份数据库",
    )?;
    let mut keys = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut target_keys = BTreeSet::new();
    for target in targets {
        require(
            identifier(&target.target_key, 64)
                && identifier(&target.database, 64)
                && identifier(&target.server_uuid, 64)
                && target_keys.insert(&target.target_key)
                && keys.insert(&target.source_key)
                && identities.insert((&target.server_uuid, &target.database)),
            "恢复目标身份无效或重复",
        )?;
        require(
            source.iter().any(|db| db.key == target.source_key),
            "恢复目标引用了未知备份数据库",
        )?;
        require(
            source
                .iter()
                .all(|db| db.server_uuid != target.server_uuid || db.database != target.database),
            "恢复目标不能指向任何原业务数据库",
        )?;
    }
    Ok(())
}

pub const REQUIRED_RESTORE_SCENARIOS: &[&str] = &[
    "login",
    "session",
    "post",
    "notice",
    "tenant",
    "export",
    "message",
    "schedule",
    "restored-data",
];

pub fn validate_restore_proof(
    backup: &BackupRecord,
    record: &RestoreRecord,
    proof: &RestoreBusinessProof,
    now: DateTime<Utc>,
) -> AppResult<()> {
    require(
        record.status == RestoreStatus::DataVerified
            && proof.restore_id == record.plan.id
            && proof.plan_hash == record.plan_hash
            && proof.backend_sha == backup.manifest.source_sha
            && proof.frontend_sha == record.plan.frontend_sha
            && proof.scope_id == record.plan.scope_id
            && hex(&proof.runtime_receipt_sha256, 64),
        "业务验收证据与当前演练、scope 或源码不一致",
    )?;
    require(
        record
            .data_verified_at
            .is_some_and(|time| proof.started_at >= time)
            && proof.completed_at >= proof.started_at
            && proof.completed_at <= now,
        "业务验收必须在数据验证之后完成",
    )?;
    require(
        proof.unexpected_console_messages == 0
            && proof.unexpected_network_failures == 0
            && proof.axe_serious_or_critical == 0,
        "浏览器错误、网络失败或无障碍检查未通过",
    )?;
    let mut names = BTreeSet::new();
    let required = REQUIRED_RESTORE_SCENARIOS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    require(
        proof
            .scenarios
            .iter()
            .all(|item| item.succeeded && names.insert(item.name.as_str()))
            && names == required,
        "核心业务恢复验收存在缺失、额外、重复或失败场景",
    )
}
