use crate::DbResultExt;
use chrono::{DateTime, Utc};
use ryframe_application::ports::backup::{BackupHealth, RestoreStatus};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use std::collections::BTreeMap;
use time::{OffsetDateTime, PrimitiveDateTime};

const CANDIDATE_BATCH_SIZE: usize = 64;

#[derive(FromQueryResult)]
struct ResourceHealth {
    resource_key: String,
    valid_id: Option<String>,
    valid_capture: Option<PrimitiveDateTime>,
    invalid_capture: Option<PrimitiveDateTime>,
    expired_capture: Option<PrimitiveDateTime>,
}

#[derive(FromQueryResult)]
struct ActiveRestores {
    running: i64,
    overdue: i64,
}

#[derive(FromQueryResult)]
struct RestoreId {
    id: String,
}

#[derive(FromQueryResult)]
struct BackupId {
    id: String,
}

pub async fn backup_health<C: ConnectionTrait>(
    db: &C,
    scope_id: &str,
    required: &[String],
    now: DateTime<Utc>,
) -> AppResult<BackupHealth> {
    let rows = ResourceHealth::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT r.resource_key, \
         SUBSTRING_INDEX(GROUP_CONCAT(CASE WHEN b.valid = 1 AND b.retention_until > ? THEN b.id END \
           ORDER BY b.captured_at DESC, b.id DESC SEPARATOR ','), ',', 1) AS valid_id, \
         MAX(CASE WHEN b.valid = 1 AND b.retention_until > ? THEN b.captured_at END) AS valid_capture, \
         MAX(CASE WHEN b.valid = 0 AND b.retention_until > ? THEN b.captured_at END) AS invalid_capture, \
         MAX(CASE WHEN b.retention_until <= ? THEN b.captured_at END) AS expired_capture \
         FROM sys_backup_resource r JOIN sys_backup_set b ON b.id = r.backup_id \
         WHERE b.scope_id = ? GROUP BY r.resource_key",
        vec![now.into(), now.into(), now.into(), now.into(), scope_id.into()],
    )).all(db).await.db()?;
    let valid_ids = rows
        .iter()
        .filter(|row| required.contains(&row.resource_key))
        .filter_map(|row| row.valid_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let (mut health, oldest_capture) = aggregate_resources(required, rows);
    health.oldest_capture = oldest_capture.map(PrimitiveDateTime::assume_utc);
    if health.missing_resources == 0
        && health.invalid_resources == 0
        && health.expired_resources == 0
    {
        let oldest = health
            .oldest_capture
            .ok_or_else(|| AppError::Validation("健康备份缺少实际采集时间".into()))?;
        let oldest = DateTime::from_timestamp(oldest.unix_timestamp(), oldest.nanosecond())
            .ok_or_else(|| AppError::Database("备份健康时间超出 UTC 表示范围".into()))?;
        validate_healthy_candidates(db, scope_id, now, oldest, &valid_ids).await?;
    }
    let last = RestoreId::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT r.id FROM sys_restore_run r JOIN sys_backup_set b ON b.id = r.backup_id \
         WHERE b.scope_id = ? AND r.completed_at IS NOT NULL ORDER BY r.completed_at DESC, r.id DESC LIMIT 1",
        [scope_id.into()],
    )).one(db).await.db()?;
    if let Some(last) = last {
        let record = super::restore_state::read(db, &last.id, false)
            .await?
            .ok_or_else(|| {
                ryframe_kernel::AppError::Database("恢复健康记录在读取期间消失".into())
            })?;
        health.last_restore_completed = record.completed_at.map(health_timestamp).transpose()?;
        health.last_restore_succeeded = record.status == RestoreStatus::Succeeded;
        health.restore_duration_seconds = record
            .completed_at
            .map(|time| (time - record.started_at).num_seconds());
        health.recovery_point_age_seconds =
            Some((record.plan.fault_at - record.recovered_at).num_seconds());
    }
    let active = ActiveRestores::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT CAST(COUNT(*) AS SIGNED) AS running, \
         CAST(COALESCE(SUM(TIMESTAMPDIFF(SECOND, r.started_at, ?) > 3600), 0) AS SIGNED) AS overdue \
         FROM sys_restore_run r JOIN sys_backup_set b ON b.id = r.backup_id \
         WHERE b.scope_id = ? AND r.completed_at IS NULL", [now.into(), scope_id.into()],
    )).one(db).await.db()?;
    if let Some(active) = active {
        health.restore_running = active.running.max(0) as u64;
        health.restore_overdue = active.overdue.max(0) as u64;
    }
    Ok(health)
}

async fn validate_healthy_candidates<C: ConnectionTrait>(
    db: &C,
    scope_id: &str,
    now: DateTime<Utc>,
    oldest: DateTime<Utc>,
    valid_ids: &std::collections::BTreeSet<String>,
) -> AppResult<()> {
    let recent = BackupId::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT id FROM sys_backup_set \
         WHERE scope_id = ? AND retention_until > ? AND captured_at >= ? \
         ORDER BY captured_at DESC, id DESC",
        [scope_id.into(), now.into(), oldest.into()],
    ))
    .all(db)
    .await
    .db()?;
    let mut ids = recent
        .iter()
        .map(|row| row.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    ids.extend(valid_ids.iter().cloned());
    let ids = ids.into_iter().collect::<Vec<_>>();
    for chunk in ids.chunks(CANDIDATE_BATCH_SIZE) {
        let batch = chunk.iter().cloned().collect();
        let records = super::validated_backups(db, &batch).await?;
        for (id, record) in records {
            if record.manifest.scope_id != scope_id
                || record.manifest.retention_until <= now
                || record.manifest.captured_at < oldest
                || valid_ids.contains(&id) && !record.valid
            {
                return Err(AppError::Validation("健康候选备份状态已变化".into()));
            }
        }
    }
    Ok(())
}

fn aggregate_resources(
    required: &[String],
    rows: Vec<ResourceHealth>,
) -> (BackupHealth, Option<PrimitiveDateTime>) {
    let rows = rows
        .into_iter()
        .map(|row| (row.resource_key.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let mut health = BackupHealth {
        required_resources: required.len() as u64,
        ..Default::default()
    };
    let mut oldest_capture: Option<PrimitiveDateTime> = None;
    for resource in required {
        let Some(row) = rows.get(resource) else {
            health.missing_resources += 1;
            continue;
        };
        if let Some(captured) = row.valid_capture {
            oldest_capture = Some(oldest_capture.map_or(captured, |old| old.min(captured)));
        } else if row.expired_capture.is_some() {
            health.expired_resources += 1;
        } else if row.invalid_capture.is_none() {
            health.missing_resources += 1;
        }
        if row
            .invalid_capture
            .is_some_and(|invalid| row.valid_capture.is_none_or(|valid| invalid >= valid))
        {
            health.invalid_resources += 1;
        }
    }
    (health, oldest_capture)
}

fn health_timestamp(value: DateTime<Utc>) -> AppResult<OffsetDateTime> {
    let nanos = value.timestamp_subsec_nanos();
    if nanos >= 1_000_000_000 {
        return Err(AppError::Database(
            "备份健康时间包含 MySQL 不支持的闰秒".into(),
        ));
    }
    let epoch_nanos = i128::from(value.timestamp()) * 1_000_000_000 + i128::from(nanos);
    OffsetDateTime::from_unix_timestamp_nanos(epoch_nanos)
        .map_err(|_| AppError::Database("备份健康时间超出 UTC 表示范围".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn health_uses_oldest_actual_capture_and_keeps_failure_categories() {
        let captured = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let time = PrimitiveDateTime::new(captured.date(), captured.time());
        let required = ["a", "b", "missing", "invalid", "expired"].map(String::from);
        let row = |key: &str, valid, invalid, expired| ResourceHealth {
            resource_key: key.into(),
            valid_id: None,
            valid_capture: valid,
            invalid_capture: invalid,
            expired_capture: expired,
        };
        let (health, oldest_capture) = aggregate_resources(
            &required,
            vec![
                row("a", Some(time), None, None),
                row("b", Some(time - Duration::hours(23)), None, None),
                row("invalid", None, Some(time), None),
                row("expired", None, None, Some(time)),
                row("not-required", Some(time - Duration::days(5)), None, None),
            ],
        );
        assert_eq!(oldest_capture, Some(time - Duration::hours(23)));
        assert_eq!(
            (
                health.required_resources,
                health.missing_resources,
                health.invalid_resources,
                health.expired_resources
            ),
            (5, 1, 1, 1)
        );
    }

    #[test]
    fn health_projection_preserves_fraction_and_rejects_unrepresentable_times() {
        let before_epoch = DateTime::from_timestamp(-1, 123_456_000).unwrap();
        assert_eq!(
            health_timestamp(before_epoch)
                .unwrap()
                .unix_timestamp_nanos(),
            -876_544_000
        );
        let leap = DateTime::from_timestamp(59, 1_000_000_000).unwrap();
        assert!(health_timestamp(leap).is_err());
        let outside_range = DateTime::from_timestamp(253_402_300_800, 0).unwrap();
        assert!(health_timestamp(outside_range).is_err());
    }
}
