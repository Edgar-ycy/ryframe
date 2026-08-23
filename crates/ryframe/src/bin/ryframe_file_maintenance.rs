// Qodana 默认不启用 Cargo 的 required-features；目标仍由清单强制门禁。
//noinspection MissingFeatures
//! 一次性文件元数据维护命令。
//!
//! 该命令只用于 FILE-A 单向切换：先校验旧 MD5 并回填 SHA-256，再清空旧版
//! `del_flag = '3'` 上传预留。常规 API 与 Worker 不启用本二进制所需的 Cargo feature。

#![cfg(feature = "file-maintenance")]

use std::{error::Error, sync::Arc};

use chrono::{DateTime, Utc};
use ryframe_adapters::storage::{
    LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage, ScopedObjectStorage,
};
use ryframe_config::{AppConfig, Environment, StorageBackend};
use ryframe_db::entities::sys_file;
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
                request_timeout_secs: config.object_storage.request_timeout_secs,
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

#[path = "ryframe_file_maintenance/backfill.rs"]
mod backfill;
#[path = "ryframe_file_maintenance/drain.rs"]
mod drain;

use backfill::backfill_sha256;
use drain::drain_legacy_reservations;

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
