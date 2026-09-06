use crate::DbResultExt;
use chrono::{DateTime, Utc};
use ryframe_application::ports::backup::*;
use ryframe_application::system::operations::validate_backup_record;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};

mod health;
mod restore_state;
mod tenant_points;
pub use health::backup_health;

#[derive(FromQueryResult)]
struct RequiredTarget {
    current_target_key: String,
}

pub async fn required_resources<C: ConnectionTrait>(db: &C) -> AppResult<Vec<String>> {
    let targets = RequiredTarget::find_by_statement(Statement::from_string(DbBackend::MySql,
        "SELECT DISTINCT current_target_key FROM sys_tenant_data_placement ORDER BY current_target_key",
    )).all(db).await.db()?;
    let mut resources = std::collections::BTreeSet::from([format!(
        "db:{}",
        ryframe_config::SHARED_CONTROL_TARGET_KEY
    )]);
    resources.extend(
        targets
            .into_iter()
            .map(|row| format!("db:{}", row.current_target_key)),
    );
    resources.extend(
        ryframe_application::system::operations::BACKUP_OBJECT_BUCKETS
            .iter()
            .map(|bucket| format!("objects:{bucket}")),
    );
    Ok(resources.into_iter().collect())
}

#[derive(FromQueryResult)]
struct BackupRow {
    id: String,
    scope_id: String,
    manifest_hash: String,
    captured_at: DateTime<Utc>,
    retention_until: DateTime<Utc>,
    checked_at: DateTime<Utc>,
    valid: bool,
    payload: serde_json::Value,
}

pub async fn backup<C: ConnectionTrait>(
    db: &C,
    id: &str,
    lock: bool,
) -> AppResult<Option<BackupRecord>> {
    let Some(record) = backup_header(db, id, lock).await? else {
        return Ok(None);
    };
    validate_resource_projection(db, id, &record, lock).await?;
    Ok(Some(record))
}

async fn backup_header<C: ConnectionTrait>(
    db: &C,
    id: &str,
    lock: bool,
) -> AppResult<Option<BackupRecord>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    let row = BackupRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        format!(
            "SELECT id, scope_id, manifest_hash, captured_at, retention_until, checked_at, valid, payload \
             FROM sys_backup_set WHERE id = ?{suffix}"
        ),
        [id.into()],
    ))
    .one(db)
    .await
    .db()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = decode_backup_row(row)?;
    if record.manifest.id != id {
        return Err(AppError::Validation("备份集主键投影不一致".into()));
    }
    Ok(Some(record))
}

async fn validate_resource_projection<C: ConnectionTrait>(
    db: &C,
    id: &str,
    record: &BackupRecord,
    lock: bool,
) -> AppResult<()> {
    let resource_suffix = if lock { " FOR UPDATE" } else { "" };
    let rows = RequiredResource::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        format!(
            "SELECT resource_key FROM sys_backup_resource \
             WHERE backup_id = ? ORDER BY resource_key{resource_suffix}"
        ),
        [id.into()],
    ))
    .all(db)
    .await
    .db()?;
    let actual = rows
        .into_iter()
        .map(|row| row.resource_key)
        .collect::<Vec<_>>();
    validate_resource_keys(record, &actual)
}

pub async fn restore<C: ConnectionTrait>(
    db: &C,
    id: &str,
    lock: bool,
) -> AppResult<Option<RestoreRecord>> {
    restore_state::read(db, id, lock).await
}

#[derive(FromQueryResult)]
struct RequiredResource {
    resource_key: String,
}

#[derive(FromQueryResult)]
struct BackupResourceRow {
    backup_id: String,
    resource_key: String,
}

pub(super) fn decode<T: DeserializeOwned>(payload: serde_json::Value) -> AppResult<T> {
    serde_json::from_value(payload).map_err(|_| AppError::Internal("备份状态数据格式无效".into()))
}

pub(super) fn encode(record: &impl serde::Serialize) -> AppResult<serde_json::Value> {
    serde_json::to_value(record).map_err(|_| AppError::Internal("备份状态无法序列化".into()))
}

pub async fn save_backup<C: ConnectionTrait>(
    db: &C,
    record: &BackupRecord,
) -> AppResult<BackupRecord> {
    validate_backup_record(record)?;
    if insert_backup_header(db, record).await? {
        insert_resource_projection(db, record).await?;
        tenant_points::save(db, record).await?;
        return require_authoritative_backup(db, record).await;
    }

    let existing = backup(db, &record.manifest.id, true)
        .await?
        .ok_or_else(|| AppError::Database("备份集重复键已报告但权威记录不可读".into()))?;
    if existing.manifest != record.manifest || existing.manifest_hash != record.manifest_hash {
        return Err(AppError::Conflict("备份集 ID 已用于不同清单".into()));
    }
    if existing.checked_at >= record.checked_at {
        tenant_points::save(db, &existing).await?;
        return Ok(existing);
    }

    update_backup_verification(db, record).await?;
    tenant_points::save(db, record).await?;
    require_authoritative_backup(db, record).await
}

async fn insert_backup_header<C: ConnectionTrait>(
    db: &C,
    record: &BackupRecord,
) -> AppResult<bool> {
    let manifest = &record.manifest;
    let result = db.execute_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "INSERT INTO sys_backup_set (id, scope_id, manifest_hash, captured_at, retention_until, checked_at, valid, payload) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        vec![manifest.id.clone().into(), manifest.scope_id.clone().into(), record.manifest_hash.clone().into(),
            manifest.captured_at.into(), manifest.retention_until.into(), record.checked_at.into(), record.valid.into(), encode(record)?.into()],
    )).await;
    match result {
        Ok(result) if result.rows_affected() == 1 => Ok(true),
        Ok(_) => Err(AppError::Database("创建备份集的受影响行数异常".into())),
        Err(error) if is_duplicate_key_error(&error) => Ok(false),
        Err(error) => Err(error).db(),
    }
}

async fn insert_resource_projection<C: ConnectionTrait>(
    db: &C,
    record: &BackupRecord,
) -> AppResult<()> {
    for key in record.manifest.resource_keys() {
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_backup_resource (backup_id, resource_key) VALUES (?, ?)",
            [record.manifest.id.clone().into(), key.into()],
        ))
        .await
        .db()?;
    }
    Ok(())
}

async fn update_backup_verification<C: ConnectionTrait>(
    db: &C,
    record: &BackupRecord,
) -> AppResult<()> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE sys_backup_set SET checked_at = ?, valid = ?, payload = ? \
             WHERE id = ? AND manifest_hash = ?",
            vec![
                record.checked_at.into(),
                record.valid.into(),
                encode(record)?.into(),
                record.manifest.id.clone().into(),
                record.manifest_hash.clone().into(),
            ],
        ))
        .await
        .db()?;
    if result.rows_affected() != 1 {
        return Err(AppError::Conflict("备份集校验状态已变化".into()));
    }
    Ok(())
}

async fn require_authoritative_backup<C: ConnectionTrait>(
    db: &C,
    expected: &BackupRecord,
) -> AppResult<BackupRecord> {
    let authoritative = backup(db, &expected.manifest.id, true)
        .await?
        .ok_or_else(|| AppError::Database("备份集写入后权威记录不可读".into()))?;
    if authoritative != *expected {
        return Err(AppError::Database("备份集写入后权威记录不一致".into()));
    }
    Ok(authoritative)
}

pub async fn create_restore<C: ConnectionTrait>(
    db: &C,
    record: &RestoreRecord,
) -> AppResult<RestoreRecord> {
    restore_state::create(db, record).await
}

pub async fn advance_restore<C: ConnectionTrait>(
    db: &C,
    expected: &RestoreRecord,
    next: &RestoreRecord,
) -> AppResult<RestoreRecord> {
    restore_state::advance(db, expected, next).await
}

pub(super) async fn update_drill_time<C: ConnectionTrait>(
    db: &C,
    backup_id: &str,
    completed: Option<DateTime<Utc>>,
) -> AppResult<()> {
    let completed =
        completed.ok_or_else(|| AppError::Validation("成功恢复演练缺少完成时间".into()))?;
    let Some(backup) = backup(db, backup_id, true).await? else {
        return Err(AppError::NotFound("恢复演练的备份集不存在".into()));
    };
    if !backup.valid || backup.manifest.retention_until <= completed {
        return Err(AppError::Validation(
            "恢复演练完成时备份已经无效或过期".into(),
        ));
    }
    for target in backup.manifest.databases.iter().filter(|target| {
        target.kind != BackupDatabaseKind::Control
            && (target.shared || !target.placements.is_empty())
    }) {
        let (point, expected) = tenant_points::lock_projection(db, &backup, target).await?;
        if point.validation_status != expected.validation_status
            || point.validation_detail != expected.validation_detail
        {
            return Err(AppError::Validation(
                "恢复演练绑定的备份点校验状态不一致".into(),
            ));
        }
        let result = db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE sys_tenant_data_backup_point \
             SET last_restore_drill_at = CASE \
                   WHEN last_restore_drill_at IS NULL OR last_restore_drill_at < ? THEN ? \
                   ELSE last_restore_drill_at END, \
                 updated_at = GREATEST(updated_at, UTC_TIMESTAMP(6)) \
             WHERE id = ? AND provider_ref = ?",
                [
                    completed.into(),
                    completed.into(),
                    point.id.into(),
                    tenant_points::provider_ref(backup_id, &target.key).into(),
                ],
            ))
            .await
            .db()?;
        if result.rows_affected() != 1 {
            return Err(AppError::Database(
                "恢复演练终态投影的受影响行数异常".into(),
            ));
        }
    }
    Ok(())
}

fn decode_backup_row(row: BackupRow) -> AppResult<BackupRecord> {
    let record: BackupRecord = decode(row.payload.clone())?;
    validate_backup_record(&record)?;
    let hash = backup_content_hash(&record.manifest)?;
    let projection_matches = record.manifest.id == row.id
        && record.manifest.scope_id == row.scope_id
        && record.manifest_hash == hash
        && record.manifest_hash == row.manifest_hash
        && record.manifest.captured_at == row.captured_at
        && record.manifest.retention_until == row.retention_until
        && record.checked_at == row.checked_at
        && record.valid == row.valid;
    if !projection_matches {
        return Err(AppError::Validation(
            "备份清单摘要或数据库投影不一致".into(),
        ));
    }
    Ok(record)
}

fn validate_resource_keys(record: &BackupRecord, actual: &[String]) -> AppResult<()> {
    let mut expected = record.manifest.resource_keys();
    expected.sort();
    let mut actual = actual.to_vec();
    actual.sort();
    if actual != expected {
        return Err(AppError::Validation("备份清单与资源关系投影不一致".into()));
    }
    Ok(())
}

pub(super) async fn validated_backups<C: ConnectionTrait>(
    db: &C,
    ids: &BTreeSet<String>,
) -> AppResult<BTreeMap<String, BackupRecord>> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let values = || ids.iter().cloned().map(Into::into).collect::<Vec<_>>();
    let rows = BackupRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        format!(
            "SELECT id, scope_id, manifest_hash, captured_at, retention_until, checked_at, valid, payload \
             FROM sys_backup_set WHERE id IN ({placeholders}) ORDER BY id"
        ),
        values(),
    ))
    .all(db)
    .await
    .db()?;
    let resource_rows = BackupResourceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        format!(
            "SELECT backup_id, resource_key FROM sys_backup_resource \
             WHERE backup_id IN ({placeholders}) ORDER BY backup_id, resource_key"
        ),
        values(),
    ))
    .all(db)
    .await
    .db()?;
    let mut resources = BTreeMap::<String, Vec<String>>::new();
    for row in resource_rows {
        resources
            .entry(row.backup_id)
            .or_default()
            .push(row.resource_key);
    }
    let mut records = BTreeMap::new();
    for row in rows {
        let id = row.id.clone();
        let record = decode_backup_row(row)?;
        validate_resource_keys(&record, &resources.remove(&id).unwrap_or_default())?;
        records.insert(id, record);
    }
    if records.len() != ids.len() || !resources.is_empty() {
        return Err(AppError::Validation(
            "健康候选备份或资源关系集合不完整".into(),
        ));
    }
    Ok(records)
}

pub(super) fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("duplicate entry") || message.contains("1062")
}
