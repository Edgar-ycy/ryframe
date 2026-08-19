//! 一次性文件元数据维护命令。
//!
//! 该命令只用于 FILE-A 单向切换：先校验旧 MD5 并回填 SHA-256，再清空旧版
//! `del_flag = '3'` 上传预留。常规 API 与 Worker 不启用本二进制所需的 Cargo feature。

use std::{error::Error, sync::Arc};

use chrono::{DateTime, Utc};
use ryframe_config::{AppConfig, Environment, StorageBackend};
use ryframe_db::entities::sys_file;
use ryframe_storage::{
    LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage, ScopedObjectStorage,
};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    FromQueryResult, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
    TransactionTrait, TryGetable,
    sea_query::{Expr, LockType},
};
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error + Send + Sync>;

const APPLY_CONFIRMATION: &str = "APPLY-FILE-A-MAINTENANCE";
const LEGACY_RESERVED_FLAG: &str = "3";
const DEFAULT_BATCH_SIZE: u64 = 100;
const MAX_BATCH_SIZE: u64 = 1_000;
const MIN_CLEANUP_GRACE_SECONDS: i64 = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    BackfillSha256,
    DrainLegacyReservations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    DryRun,
    Apply,
}

#[derive(Debug, Eq, PartialEq)]
struct Arguments {
    command: Command,
    mode: Mode,
    expected_database: String,
    batch_size: u64,
    start_after: i64,
}

#[derive(Default)]
struct BackfillStats {
    scanned: u64,
    updated: u64,
    already_updated: u64,
}

/// 迁移前旧表的最小读取投影，不进入任何正常运行时仓储或服务。
#[derive(Debug, FromQueryResult)]
struct LegacyDigestRow {
    id: i64,
    bucket: String,
    storage_path: String,
    file_size: i64,
    file_md5: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct ObjectDigests {
    byte_len: usize,
    legacy_md5: String,
    sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackfillCasDecision {
    AlreadyApplied,
    Conflict,
}

#[derive(Default)]
struct DrainStats {
    scanned: u64,
    normalized_ready: u64,
    moved_to_cleanup: u64,
    deleted_cleanup: u64,
    waiting: u64,
}

#[derive(Debug, Eq, PartialEq)]
enum DrainPlan {
    NormalizeReady,
    MovePendingToCleanup,
    DeleteCleanup,
    WaitUntil(DateTime<Utc>),
}

#[tokio::main]
async fn main() -> Result<(), DynError> {
    let arguments = parse_args(std::env::args().skip(1))?;
    let environment = Environment::from_required_env()?;
    let config = AppConfig::load_from_env(environment)?;
    if config.database.primary.database != arguments.expected_database {
        return Err(format!(
            "配置数据库与 --database 不一致（配置: {}，期望: {}）",
            config.database.primary.database, arguments.expected_database
        )
        .into());
    }

    let database = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    verify_connected_database(&database, &arguments.expected_database).await?;
    let storage = build_storage(&config)?;

    println!(
        "FILE-A maintenance: command={:?} mode={:?} environment={} database={} batch_size={} start_after={}",
        arguments.command,
        arguments.mode,
        environment,
        arguments.expected_database,
        arguments.batch_size,
        arguments.start_after
    );

    let operation = match arguments.command {
        Command::BackfillSha256 => backfill_sha256(&database, storage.as_ref(), &arguments).await,
        Command::DrainLegacyReservations => {
            drain_legacy_reservations(&database, storage.as_ref(), &arguments).await
        }
    };
    let close_result = database.close().await;
    operation?;
    close_result?;
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Arguments, DynError> {
    let mut arguments = arguments.into_iter();
    let command = match arguments.next().as_deref() {
        Some("backfill-sha256") => Command::BackfillSha256,
        Some("drain-legacy-reservations") => Command::DrainLegacyReservations,
        _ => return Err(usage().into()),
    };
    let mode = match arguments.next().as_deref() {
        Some("dry-run") => Mode::DryRun,
        Some("apply") => Mode::Apply,
        _ => return Err(usage().into()),
    };

    let mut expected_database = None;
    let mut batch_size = DEFAULT_BATCH_SIZE;
    let mut start_after = i64::MIN;
    let mut confirmation = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--database" => expected_database = arguments.next(),
            "--batch-size" => {
                let value = arguments
                    .next()
                    .ok_or("--batch-size 缺少数值")?
                    .parse::<u64>()
                    .map_err(|_| "--batch-size 必须是正整数")?;
                if !(1..=MAX_BATCH_SIZE).contains(&value) {
                    return Err(format!("--batch-size 必须在 1..={MAX_BATCH_SIZE} 之间").into());
                }
                batch_size = value;
            }
            "--start-after" => {
                start_after = arguments
                    .next()
                    .ok_or("--start-after 缺少文件 ID")?
                    .parse::<i64>()
                    .map_err(|_| "--start-after 必须是 i64 文件 ID")?;
            }
            "--confirm-apply" => confirmation = arguments.next(),
            _ => return Err(format!("未知参数: {argument}\n{}", usage()).into()),
        }
    }

    let expected_database = expected_database
        .filter(|value| !value.trim().is_empty())
        .ok_or("必须提供 --database <expected-name>")?;
    if mode == Mode::Apply && confirmation.as_deref() != Some(APPLY_CONFIRMATION) {
        return Err(
            format!("拒绝写入：apply 模式必须提供 --confirm-apply {APPLY_CONFIRMATION}").into(),
        );
    }

    Ok(Arguments {
        command,
        mode,
        expected_database,
        batch_size,
        start_after,
    })
}

fn usage() -> &'static str {
    "用法: ryframe-file-maintenance <backfill-sha256|drain-legacy-reservations> \
<dry-run|apply> --database <name> [--batch-size <1..1000>] [--start-after <id>] \
[--confirm-apply APPLY-FILE-A-MAINTENANCE]"
}

fn build_storage(config: &AppConfig) -> Result<Arc<dyn ObjectStorage>, DynError> {
    let raw_storage: Arc<dyn ObjectStorage> = match config.object_storage.backend {
        StorageBackend::Local => Arc::new(LocalObjectStorage::new(
            &config.object_storage.local_base_dir,
        )),
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => {
            Arc::new(S3ObjectStorage::new(S3Config {
                endpoint: config.object_storage.endpoint.clone(),
                access_key: config.object_storage.access_key.clone(),
                secret_key: config.object_storage.secret_key.clone(),
                use_ssl: config.object_storage.use_ssl,
                region: config.object_storage.region.clone(),
            })?)
        }
    };
    Ok(Arc::new(ScopedObjectStorage::new(
        raw_storage,
        config.scope_id.as_str(),
    )))
}

async fn verify_connected_database(
    database: &DatabaseConnection,
    expected: &str,
) -> Result<(), DynError> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE()".to_owned(),
        ))
        .await?
        .ok_or("数据库身份查询没有返回结果")?;
    let actual = String::try_get_by_index(&row, 0)
        .map_err(|error| format!("无法读取当前数据库名称: {error:?}"))?;
    if actual != expected {
        return Err(format!(
            "实际连接数据库与 --database 不一致（实际: {actual}，期望: {expected}）"
        )
        .into());
    }
    Ok(())
}

async fn backfill_sha256(
    database: &DatabaseConnection,
    storage: &dyn ObjectStorage,
    arguments: &Arguments,
) -> Result<(), DynError> {
    let mut cursor = arguments.start_after;
    let mut stats = BackfillStats::default();

    loop {
        let rows = LegacyDigestRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, bucket, storage_path, file_size, file_md5 \
             FROM sys_file WHERE id > ? AND file_sha256 IS NULL \
             ORDER BY id ASC LIMIT ?",
            [cursor.into(), i64::try_from(arguments.batch_size)?.into()],
        ))
        .all(database)
        .await?;
        if rows.is_empty() {
            break;
        }

        for row in rows {
            cursor = row.id;
            stats.scanned += 1;
            let legacy_md5 = row
                .file_md5
                .as_deref()
                .ok_or_else(|| format!("文件 {} 缺少旧 MD5，拒绝猜测摘要", row.id))?;
            let normalized_md5 = normalize_legacy_md5(legacy_md5)
                .map_err(|error| format!("文件 {} 的旧 MD5 无效: {error}", row.id))?;
            let object = storage
                .get(&row.bucket, &row.storage_path)
                .await
                .map_err(|error| {
                    format!(
                        "读取文件 {} 对象失败（bucket={}, key={}）: {error}",
                        row.id, row.bucket, row.storage_path
                    )
                })?;
            let digests = tokio::task::spawn_blocking(move || calculate_object_digests(&object))
                .await
                .map_err(|error| format!("文件 {} 摘要任务失败: {error}", row.id))?;
            validate_backfill_object(row.id, row.file_size, &normalized_md5, &digests)?;
            let sha256 = digests.sha256;

            println!(
                "{} file_id={} cursor={} sha256={}",
                if arguments.mode == Mode::Apply {
                    "apply"
                } else {
                    "dry-run"
                },
                row.id,
                cursor,
                sha256
            );
            if arguments.mode == Mode::DryRun {
                continue;
            }

            let result = database
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    "UPDATE sys_file SET file_sha256 = ?, updated_at = UTC_TIMESTAMP(6) \
                     WHERE id = ? AND file_sha256 IS NULL AND file_md5 = ? \
                     AND bucket = ? AND storage_path = ? AND file_size = ?",
                    [
                        sha256.clone().into(),
                        row.id.into(),
                        legacy_md5.to_owned().into(),
                        row.bucket.clone().into(),
                        row.storage_path.clone().into(),
                        row.file_size.into(),
                    ],
                ))
                .await?;
            if result.rows_affected() == 1 {
                stats.updated += 1;
                continue;
            }

            let current = sys_file::Entity::find_by_id(row.id).one(database).await?;
            match classify_backfill_cas(
                current.as_ref().map(|file| file.file_sha256.as_str()),
                &sha256,
            ) {
                BackfillCasDecision::AlreadyApplied => stats.already_updated += 1,
                BackfillCasDecision::Conflict => {
                    return Err(
                        format!("文件 {} 的 SHA-256 CAS 失败，数据已发生变化", row.id).into(),
                    );
                }
            }
        }
    }

    let remaining = sys_file::Entity::find()
        .filter(sys_file::Column::FileSha256.is_null())
        .count(database)
        .await?;
    println!(
        "SHA-256 backfill summary: scanned={} updated={} already_updated={} remaining={}",
        stats.scanned, stats.updated, stats.already_updated, remaining
    );
    if arguments.mode == Mode::Apply && remaining != 0 {
        return Err(format!(
            "仍有 {remaining} 条文件记录缺少 SHA-256；请从更早游标重试并排除数据错误"
        )
        .into());
    }
    Ok(())
}

async fn drain_legacy_reservations(
    database: &DatabaseConnection,
    storage: &dyn ObjectStorage,
    arguments: &Arguments,
) -> Result<(), DynError> {
    let now = database_utc_now(database).await?;
    if arguments.mode == Mode::Apply {
        let active_pending = sys_file::Entity::find()
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(
                Condition::any()
                    .add(sys_file::Column::ReservationExpiresAt.is_null())
                    .add(sys_file::Column::ReservationExpiresAt.gt(now)),
            )
            .count(database)
            .await?;
        if active_pending != 0 {
            return Err(format!(
                "检测到 {active_pending} 条仍有效或无到期时间的旧上传预留；请停止旧版 API/Worker，等待租约到期后重试"
            )
            .into());
        }
    }

    let cleanup_grace = cleanup_grace(storage);
    let mut cursor = arguments.start_after;
    let mut stats = DrainStats::default();
    loop {
        let rows = sys_file::Entity::find()
            .filter(sys_file::Column::Id.gt(cursor))
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .order_by_asc(sys_file::Column::Id)
            .limit(arguments.batch_size)
            .all(database)
            .await?;
        if rows.is_empty() {
            break;
        }

        for row in rows {
            cursor = row.id;
            stats.scanned += 1;
            match plan_legacy_reservation(&row, now)? {
                DrainPlan::NormalizeReady => {
                    println!(
                        "{:?} ready file_id={} -> del_flag=0",
                        arguments.mode, row.id
                    );
                    if arguments.mode == Mode::Apply
                        && normalize_ready_reservation(database, &row).await?
                    {
                        stats.normalized_ready += 1;
                    }
                }
                DrainPlan::MovePendingToCleanup => {
                    let cleanup_after = now + cleanup_grace;
                    println!(
                        "{:?} pending file_id={} -> cleanup until {}",
                        arguments.mode,
                        row.id,
                        cleanup_after.to_rfc3339()
                    );
                    if arguments.mode == Mode::Apply
                        && move_pending_to_cleanup(database, row.id, now, cleanup_after).await?
                    {
                        stats.moved_to_cleanup += 1;
                    }
                }
                DrainPlan::DeleteCleanup => {
                    println!("{:?} cleanup file_id={} -> delete", arguments.mode, row.id);
                    if arguments.mode == Mode::Apply
                        && delete_cleanup_reservation(database, storage, row.id, now).await?
                    {
                        stats.deleted_cleanup += 1;
                    }
                }
                DrainPlan::WaitUntil(until) => {
                    stats.waiting += 1;
                    println!(
                        "{:?} waiting file_id={} until={}",
                        arguments.mode,
                        row.id,
                        until.to_rfc3339()
                    );
                }
            }
        }
    }

    let remaining = sys_file::Entity::find()
        .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
        .count(database)
        .await?;
    println!(
        "reservation drain summary: scanned={} normalized_ready={} moved_to_cleanup={} deleted_cleanup={} waiting={} remaining={}",
        stats.scanned,
        stats.normalized_ready,
        stats.moved_to_cleanup,
        stats.deleted_cleanup,
        stats.waiting,
        remaining
    );
    if arguments.mode == Mode::Apply && remaining != 0 {
        return Err(
            format!("仍有 {remaining} 条旧上传预留；等待清理宽限期结束后重新执行同一命令").into(),
        );
    }
    Ok(())
}

fn plan_legacy_reservation(
    row: &sys_file::Model,
    now: DateTime<Utc>,
) -> Result<DrainPlan, DynError> {
    match row.upload_status.as_str() {
        sys_file::Model::UPLOAD_STATUS_READY => {
            validate_sha256(&row.file_sha256)
                .map_err(|error| format!("ready 文件 {} 的 SHA-256 无效: {error}", row.id))?;
            Ok(DrainPlan::NormalizeReady)
        }
        sys_file::Model::UPLOAD_STATUS_PENDING => match row.reservation_expires_at {
            Some(expires_at) if expires_at <= now => Ok(DrainPlan::MovePendingToCleanup),
            Some(expires_at) => Ok(DrainPlan::WaitUntil(expires_at)),
            None => Err(format!("pending 文件 {} 缺少预留到期时间", row.id).into()),
        },
        sys_file::Model::UPLOAD_STATUS_CLEANUP => match row.reservation_expires_at {
            Some(expires_at) if expires_at <= now => Ok(DrainPlan::DeleteCleanup),
            Some(expires_at) => Ok(DrainPlan::WaitUntil(expires_at)),
            None => Err(format!("cleanup 文件 {} 缺少清理到期时间", row.id).into()),
        },
        status => Err(format!("文件 {} 使用未知上传状态: {status}", row.id).into()),
    }
}

async fn normalize_ready_reservation(
    database: &DatabaseConnection,
    row: &sys_file::Model,
) -> Result<bool, DynError> {
    let result = sys_file::Entity::update_many()
        .col_expr(
            sys_file::Column::DelFlag,
            Expr::value(sys_file::Model::DEL_FLAG_NORMAL),
        )
        .col_expr(
            sys_file::Column::ReservationToken,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            sys_file::Column::ReservationExpiresAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        )
        .col_expr(sys_file::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(sys_file::Column::Id.eq(row.id))
        .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
        .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
        .filter(sys_file::Column::FileSha256.eq(&row.file_sha256))
        .exec(database)
        .await?;
    if result.rows_affected == 1 {
        return Ok(true);
    }
    let current = sys_file::Entity::find_by_id(row.id).one(database).await?;
    if current.as_ref().is_some_and(|file| {
        file.del_flag == sys_file::Model::DEL_FLAG_NORMAL
            && file.upload_status == sys_file::Model::UPLOAD_STATUS_READY
            && file.file_sha256 == row.file_sha256
    }) {
        return Ok(false);
    }
    Err(format!("ready 文件 {} 的状态 CAS 失败", row.id).into())
}

async fn move_pending_to_cleanup(
    database: &DatabaseConnection,
    id: i64,
    now: DateTime<Utc>,
    cleanup_after: DateTime<Utc>,
) -> Result<bool, DynError> {
    let result = sys_file::Entity::update_many()
        .col_expr(
            sys_file::Column::UploadStatus,
            Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
        )
        .col_expr(
            sys_file::Column::ReservationToken,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            sys_file::Column::ReservationExpiresAt,
            Expr::value(cleanup_after),
        )
        .col_expr(sys_file::Column::UpdatedAt, Expr::value(now))
        .filter(sys_file::Column::Id.eq(id))
        .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
        .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
        .filter(sys_file::Column::ReservationExpiresAt.lte(now))
        .exec(database)
        .await?;
    if result.rows_affected == 1 {
        return Ok(true);
    }
    let current = sys_file::Entity::find_by_id(id).one(database).await?;
    if current.as_ref().is_some_and(|file| {
        file.del_flag == LEGACY_RESERVED_FLAG
            && file.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP
    }) {
        return Ok(false);
    }
    Err(format!("pending 文件 {id} 的状态 CAS 失败").into())
}

async fn delete_cleanup_reservation(
    database: &DatabaseConnection,
    storage: &dyn ObjectStorage,
    id: i64,
    now: DateTime<Utc>,
) -> Result<bool, DynError> {
    let transaction = database.begin().await?;
    let operation: Result<bool, DynError> = async {
        let current = sys_file::Entity::find_by_id(id)
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .lock(LockType::Update)
            .one(&transaction)
            .await?;
        let Some(current) = current else {
            return Ok(false);
        };
        if current.upload_status != sys_file::Model::UPLOAD_STATUS_CLEANUP
            || !current
                .reservation_expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(format!("cleanup 文件 {id} 在锁定后已改变状态").into());
        }

        storage
            .delete(&current.bucket, &current.storage_path)
            .await
            .map_err(|error| {
                format!(
                    "删除文件 {id} 对象失败（bucket={}, key={}）: {error}",
                    current.bucket, current.storage_path
                )
            })?;
        let result = sys_file::Entity::delete_many()
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationExpiresAt.lte(now))
            .exec(&transaction)
            .await?;
        if result.rows_affected != 1 {
            return Err(format!("cleanup 文件 {id} 的删除 CAS 失败").into());
        }
        Ok(true)
    }
    .await;

    match operation {
        Ok(deleted) => {
            transaction.commit().await?;
            Ok(deleted)
        }
        Err(error) => {
            if let Err(rollback_error) = transaction.rollback().await {
                return Err(
                    format!("{error}；同时回滚 cleanup 文件 {id} 失败: {rollback_error}").into(),
                );
            }
            Err(error)
        }
    }
}

async fn database_utc_now(database: &DatabaseConnection) -> Result<DateTime<Utc>, DynError> {
    let row = database
        .query_one_raw(Statement::from_string(
            database.get_database_backend(),
            "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
        ))
        .await?
        .ok_or("数据库时钟查询没有返回结果")?;
    let now: chrono::NaiveDateTime = row.try_get("", "db_now")?;
    Ok(DateTime::from_naive_utc_and_offset(now, Utc))
}

fn cleanup_grace(storage: &dyn ObjectStorage) -> chrono::Duration {
    let late_completion_seconds =
        i64::try_from(storage.late_put_completion_bound().as_secs()).unwrap_or(i64::MAX / 2);
    chrono::Duration::seconds(
        MIN_CLEANUP_GRACE_SECONDS.max(late_completion_seconds.saturating_mul(2)),
    )
}

/// 一次遍历同时计算旧 MD5 校验值和新的 SHA-256 权威摘要。
fn calculate_object_digests(object: &[u8]) -> ObjectDigests {
    ObjectDigests {
        byte_len: object.len(),
        legacy_md5: format!("{:x}", md5::compute(object)),
        sha256: hex::encode(Sha256::digest(object)),
    }
}

/// 在写入 SHA-256 前校验旧元数据确实对应当前对象。
fn validate_backfill_object(
    file_id: i64,
    expected_size: i64,
    normalized_md5: &str,
    digests: &ObjectDigests,
) -> Result<(), DynError> {
    let expected_size =
        usize::try_from(expected_size).map_err(|_| format!("文件 {file_id} 的 file_size 非法"))?;
    if digests.byte_len != expected_size {
        return Err(format!(
            "文件 {file_id} 大小不一致（数据库: {expected_size}，对象: {}）",
            digests.byte_len
        )
        .into());
    }
    if digests.legacy_md5 != normalized_md5 {
        return Err(format!("文件 {file_id} 的旧 MD5 校验失败，拒绝写入 SHA-256").into());
    }
    Ok(())
}

/// CAS 未更新行时，只把已经写入相同 SHA-256 视为幂等成功。
fn classify_backfill_cas(
    current_sha256: Option<&str>,
    calculated_sha256: &str,
) -> BackfillCasDecision {
    if current_sha256 == Some(calculated_sha256) {
        BackfillCasDecision::AlreadyApplied
    } else {
        BackfillCasDecision::Conflict
    }
}

fn normalize_legacy_md5(value: &str) -> Result<String, &'static str> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("必须是 32 位十六进制字符串");
    }
    Ok(value.to_ascii_lowercase())
}

fn validate_sha256(value: &str) -> Result<(), &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("必须是 64 位小写十六进制字符串");
    }
    Ok(())
}
