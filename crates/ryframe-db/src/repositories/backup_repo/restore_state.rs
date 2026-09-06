use crate::DbResultExt;
use ryframe_application::ports::backup::{
    RestoreRecord, RestoreStatus, validate_restore_advance, validate_restore_creation,
    validate_restore_record,
};
use ryframe_application::system::operations::validate_restore_plan;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};

#[derive(FromQueryResult)]
struct RestoreRow {
    backup_id: String,
    scope_id: String,
    status: String,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    recovered_at: chrono::DateTime<chrono::Utc>,
    payload: serde_json::Value,
}

pub(super) async fn read<C: ConnectionTrait>(
    db: &C,
    id: &str,
    lock: bool,
) -> AppResult<Option<RestoreRecord>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    let row = RestoreRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        format!(
            "SELECT backup_id, scope_id, status, started_at, completed_at, recovered_at, payload \
             FROM sys_restore_run WHERE id = ?{suffix}"
        ),
        [id.into()],
    ))
    .one(db)
    .await
    .db()?;
    row.map(|row| decode(id, row)).transpose()
}

pub(super) async fn create<C: ConnectionTrait>(
    db: &C,
    record: &RestoreRecord,
) -> AppResult<RestoreRecord> {
    // 首次检查是普通一致性读取，不在不存在的主键上持有 gap lock。
    if read(db, &record.plan.id, false).await?.is_some() {
        let existing = read(db, &record.plan.id, true)
            .await?
            .ok_or_else(|| AppError::Database("恢复演练权威记录不可读".into()))?;
        return resolve_create(existing, record);
    }
    validate_restore_creation(record)?;
    validate_backup_for_restore(db, record).await?;
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_restore_run \
             (id, backup_id, scope_id, status, started_at, completed_at, recovered_at, payload) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                record.plan.id.clone().into(),
                record.plan.backup_id.clone().into(),
                record.plan.scope_id.clone().into(),
                status_name(record.status).into(),
                record.started_at.into(),
                record.completed_at.into(),
                record.recovered_at.into(),
                super::encode(record)?.into(),
            ],
        ))
        .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => reread_created(db, record).await,
        Ok(_) => Err(AppError::Database("创建恢复演练的受影响行数异常".into())),
        Err(error) if super::is_duplicate_key_error(&error) => {
            // 并发 INSERT 会等待获胜事务结束；锁定读取能越过旧快照取得权威记录。
            let existing = read(db, &record.plan.id, true)
                .await?
                .ok_or_else(|| AppError::Database("恢复演练重复键已报告但权威记录不可读".into()))?;
            resolve_create(existing, record)
        }
        Err(error) => Err(error).db(),
    }
}

async fn validate_backup_for_restore<C: ConnectionTrait>(
    db: &C,
    record: &RestoreRecord,
) -> AppResult<()> {
    let backup = super::backup(db, &record.plan.backup_id, true)
        .await?
        .ok_or_else(|| AppError::NotFound("恢复演练绑定的备份集不存在".into()))?;
    let now = crate::repositories::database_utc_now(db).await?;
    validate_restore_plan(&backup, &record.plan, now)?;
    if record.recovered_at != backup.manifest.captured_at {
        return Err(AppError::Validation(
            "恢复演练的实际恢复点与备份采集时间不一致".into(),
        ));
    }
    Ok(())
}

pub(super) async fn advance<C: ConnectionTrait>(
    db: &C,
    expected: &RestoreRecord,
    next: &RestoreRecord,
) -> AppResult<RestoreRecord> {
    validate_restore_advance(expected, next)?;
    let current = read(db, &expected.plan.id, true)
        .await?
        .ok_or_else(|| AppError::NotFound("恢复演练不存在".into()))?;
    if current != *expected {
        return Err(AppError::Conflict("恢复演练状态已变化".into()));
    }
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE sys_restore_run SET status = ?, completed_at = ?, payload = ? WHERE id = ?",
            vec![
                status_name(next.status).into(),
                next.completed_at.into(),
                super::encode(next)?.into(),
                expected.plan.id.clone().into(),
            ],
        ))
        .await
        .db()?;
    if result.rows_affected() != 1 {
        return Err(AppError::Conflict("恢复演练状态已变化".into()));
    }
    if next.status == RestoreStatus::Succeeded {
        super::update_drill_time(db, &next.plan.backup_id, next.completed_at).await?;
    }
    let authoritative = read(db, &next.plan.id, true)
        .await?
        .ok_or_else(|| AppError::Database("恢复演练更新后权威记录不可读".into()))?;
    if authoritative != *next {
        return Err(AppError::Database("恢复演练更新后权威记录不一致".into()));
    }
    Ok(authoritative)
}

fn decode(id: &str, row: RestoreRow) -> AppResult<RestoreRecord> {
    let record: RestoreRecord = super::decode(row.payload)?;
    validate_restore_record(&record)?;
    let projection_matches = record.plan.id == id
        && record.plan.backup_id == row.backup_id
        && record.plan.scope_id == row.scope_id
        && status_name(record.status) == row.status
        && record.started_at == row.started_at
        && record.completed_at == row.completed_at
        && record.recovered_at == row.recovered_at;
    if !projection_matches {
        return Err(AppError::Validation(
            "恢复演练记录与数据库关系投影不一致".into(),
        ));
    }
    Ok(record)
}

async fn reread_created<C: ConnectionTrait>(
    db: &C,
    record: &RestoreRecord,
) -> AppResult<RestoreRecord> {
    let authoritative = read(db, &record.plan.id, true)
        .await?
        .ok_or_else(|| AppError::Database("恢复演练创建后权威记录不可读".into()))?;
    if authoritative != *record {
        return Err(AppError::Database("恢复演练创建后权威记录不一致".into()));
    }
    Ok(authoritative)
}

fn resolve_create(existing: RestoreRecord, requested: &RestoreRecord) -> AppResult<RestoreRecord> {
    if existing.plan != requested.plan || existing.plan_hash != requested.plan_hash {
        return Err(AppError::Conflict(
            "恢复演练 ID 已用于不同的恢复计划".into(),
        ));
    }
    validate_restore_creation(requested)?;
    Ok(existing)
}

fn status_name(status: RestoreStatus) -> &'static str {
    match status {
        RestoreStatus::Running => "running",
        RestoreStatus::DataVerified => "data_verified",
        RestoreStatus::Succeeded => "succeeded",
        RestoreStatus::Failed => "failed",
    }
}
