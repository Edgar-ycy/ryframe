//! 数据库平台备份恢复点登记工具。
//!
//! 此命令只把外部数据库平台已完成且已校验的备份登记到控制库；不发起备份、
//! 恢复或访问 provider_ref 指向的资源。

use chrono::{DateTime, Duration, Utc};
use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetMode};
use ryframe_db::{
    ControlDatabaseCluster, RegisterTenantDataBackupPoint, TenantDataRepository,
    tenant_data_backup_point,
};
use ryframe_tenant_db::TenantDatabaseRouter;
use sea_orm::{DbBackend, FromQueryResult, Statement};

const USAGE: &str = "usage:\n  ryframe-tenant-data backup-register --target <key> --provider-ref <opaque-ref> --captured-at <RFC3339> --schema-fingerprint <64-lower-hex> --checksum <64-lower-hex> --retention-until <RFC3339> [--expires-at <RFC3339>]";

#[derive(Debug)]
struct BackupRegisterArgs {
    target: String,
    provider_ref: String,
    captured_at: DateTime<Utc>,
    schema_fingerprint: String,
    checksum: String,
    retention_until: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct DatabaseNowRow {
    now: DateTime<Utc>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args().skip(1).collect())?;
    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    ryframe_utils::snowflake::initialize(config.snowflake_worker_id)?;

    let primary = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    let result = register_backup(&config, primary.clone(), args).await;
    let close_result = primary.close().await;
    result?;
    close_result?;
    Ok(())
}

async fn register_backup(
    config: &AppConfig,
    primary: sea_orm::DatabaseConnection,
    args: BackupRegisterArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    ryframe_db_migration::verify(&primary).await?;
    let control = ControlDatabaseCluster::single(primary.clone());
    let router = TenantDatabaseRouter::new(
        control,
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )?;
    router.verify_target_now(&args.target).await?;
    if args.schema_fingerprint != ryframe_tenant_db_migration::TENANT_DATA_SCHEMA_FINGERPRINT {
        return Err("schema fingerprint does not match the compiled tenant-data catalog".into());
    }

    let mode = router
        .targets()
        .target_mode(&args.target)
        .ok_or("tenant-data target is not registered")?;
    let (scope, tenant_id, placement_generation) = match mode {
        TenantDatabaseTargetMode::Shared => (
            tenant_data_backup_point::Model::SCOPE_SHARD.to_owned(),
            None,
            None,
        ),
        TenantDatabaseTargetMode::Dedicated => {
            let occupancy = router
                .target_occupancy(&args.target)
                .await?
                .ok_or("dedicated target has no active tenant occupancy")?;
            (
                tenant_data_backup_point::Model::SCOPE_TENANT.to_owned(),
                Some(occupancy.tenant_id),
                Some(occupancy.placement_generation),
            )
        }
    };
    let now = database_now(&primary).await?;
    if args.captured_at > now + Duration::minutes(5) {
        return Err("captured-at cannot be in the future".into());
    }
    if args.retention_until < now || args.retention_until < args.captured_at {
        return Err("retention-until must reflect a currently retained provider backup".into());
    }
    if args
        .expires_at
        .is_some_and(|expires| expires < args.retention_until)
    {
        return Err("expires-at cannot be earlier than retention-until".into());
    }
    let repository = TenantDataRepository;
    if let Some(existing) = repository
        .backup_by_provider_ref(&primary, &args.provider_ref)
        .await?
    {
        let same = existing.scope == scope
            && existing.tenant_id == tenant_id
            && existing.target_key == args.target
            && existing.placement_generation == placement_generation
            && existing.schema_fingerprint == args.schema_fingerprint
            && existing.captured_at == args.captured_at
            && existing.checksum.as_deref() == Some(args.checksum.as_str())
            && existing.retention_until == args.retention_until
            && existing.expires_at == args.expires_at;
        if !same {
            return Err("provider-ref is already registered with different metadata".into());
        }
        println!(
            "backup_point_id={} target={} scope={} result=already-registered",
            existing.id, existing.target_key, existing.scope
        );
        return Ok(());
    }
    let backup = repository
        .insert_backup(
            &primary,
            RegisterTenantDataBackupPoint {
                id: ryframe_utils::snowflake::try_next_snowflake_id()?,
                scope,
                tenant_id,
                target_key: args.target,
                placement_generation,
                schema_fingerprint: args.schema_fingerprint,
                provider_ref: args.provider_ref,
                captured_at: args.captured_at,
                checksum: Some(args.checksum),
                validation_status: tenant_data_backup_point::Model::VALIDATION_VALID.to_owned(),
                retention_until: args.retention_until,
                expires_at: args.expires_at,
                created_by: None,
                now,
            },
        )
        .await?;
    println!(
        "backup_point_id={} target={} scope={} result=registered",
        backup.id, backup.target_key, backup.scope
    );
    Ok(())
}

async fn database_now(
    database: &sea_orm::DatabaseConnection,
) -> Result<DateTime<Utc>, sea_orm::DbErr> {
    DatabaseNowRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT UTC_TIMESTAMP(6) AS now",
    ))
    .one(database)
    .await?
    .map(|row| row.now)
    .ok_or_else(|| sea_orm::DbErr::Custom("database clock query returned no row".into()))
}

fn parse_args(args: Vec<String>) -> Result<BackupRegisterArgs, Box<dyn std::error::Error>> {
    if args.first().map(String::as_str) != Some("backup-register") {
        return Err(USAGE.into());
    }
    let mut target = None;
    let mut provider_ref = None;
    let mut captured_at = None;
    let mut schema_fingerprint = None;
    let mut checksum = None;
    let mut retention_until = None;
    let mut expires_at = None;
    let mut values = args.into_iter().skip(1);
    while let Some(flag) = values.next() {
        let value = values.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--target" => target = Some(value),
            "--provider-ref" => provider_ref = Some(value),
            "--captured-at" => captured_at = Some(value),
            "--schema-fingerprint" => schema_fingerprint = Some(value),
            "--checksum" => checksum = Some(value),
            "--retention-until" => retention_until = Some(value),
            "--expires-at" => expires_at = Some(value),
            _ => return Err(USAGE.into()),
        }
    }
    let target = required_text(target, "target", 64)?;
    let provider_ref = required_text(provider_ref, "provider-ref", 512)?;
    if provider_ref.chars().any(char::is_control) {
        return Err("provider-ref cannot contain control characters".into());
    }
    let captured_at =
        DateTime::parse_from_rfc3339(&required_text(captured_at, "captured-at", 64)?)?
            .with_timezone(&Utc);
    let schema_fingerprint = required_lower_sha256(schema_fingerprint, "schema-fingerprint")?;
    let checksum = required_lower_sha256(checksum, "checksum")?;
    let retention_until = parse_rfc3339(
        &required_text(retention_until, "retention-until", 64)?,
        "retention-until",
    )?;
    let expires_at = expires_at
        .map(|value| parse_rfc3339(&value, "expires-at"))
        .transpose()?;
    Ok(BackupRegisterArgs {
        target,
        provider_ref,
        captured_at,
        schema_fingerprint,
        checksum,
        retention_until,
        expires_at,
    })
}

fn parse_rfc3339(value: &str, name: &str) -> Result<DateTime<Utc>, Box<dyn std::error::Error>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| format!("--{name} must be RFC3339").into())
}

fn required_text(
    value: Option<String>,
    name: &str,
    max_len: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    value
        .filter(|value| !value.trim().is_empty() && value.len() <= max_len)
        .ok_or_else(|| format!("--{name} is required and must be at most {max_len} bytes").into())
}

fn required_lower_sha256(
    value: Option<String>,
    name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let value = required_text(value, name, 64)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("--{name} must be 64 lowercase hexadecimal characters").into());
    }
    Ok(value)
}
