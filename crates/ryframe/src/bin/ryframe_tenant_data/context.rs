use crate::args::Command;
use ryframe_adapters::{
    backup,
    storage::{LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage},
};
use ryframe_application::{ports::backup::*, system::operations::BackupService};
use ryframe_config::{AppConfig, JobWorkerMode, StorageBackend};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::DatabaseConnection;
use std::{path::PathBuf, sync::Arc};

pub struct Context {
    pub repository: Arc<dyn BackupRepository>,
    pub databases: Arc<dyn BackupDatabaseVerifier>,
    pub objects: Arc<dyn BackupObjectVerifier>,
    pub service: BackupService,
    target_database: Option<DatabaseConnection>,
}

pub async fn build(
    source: &AppConfig,
    database: DatabaseConnection,
    command: &Command,
) -> AppResult<Context> {
    let registry = ControlDatabaseCluster::single(database.clone());
    let target = command
        .restore_config()
        .map(|directory| AppConfig::load(directory, source.environment))
        .transpose()?;
    if let Some(target) = &target {
        validate_isolation(source, target)?;
    }
    let target_database = match &target {
        Some(config) => {
            ryframe_db::connection::connect_with_sql_logging(
                &config.database.primary,
                config.database.sql_log_level,
                config.database.sql_slow_threshold_ms,
            )
            .await?
        }
        None => database,
    };
    let config = target.as_ref().unwrap_or(source);
    let databases = database_verifier(config, target_database.clone())?;
    let objects = backup::object_verifier(
        storage(config)?,
        endpoint(config)?,
        config.scope_id.as_str().into(),
    );
    let repository = ryframe_db::application_ports::backup::port(registry);
    let runtime = backup::runtime_verifier(
        ready_url(&config.app.host, config.app.port),
        ready_url(&config.jobs.health_host, config.jobs.health_port),
    )?;
    let service = BackupService::new(
        repository.clone(),
        backup::artifact_verifier(command.backup_root()),
        databases.clone(),
        objects.clone(),
        runtime,
    );
    Ok(Context {
        repository,
        databases,
        objects,
        service,
        target_database: target.is_some().then_some(target_database),
    })
}

pub fn database_verifier(
    config: &AppConfig,
    database: DatabaseConnection,
) -> AppResult<Arc<dyn BackupDatabaseVerifier>> {
    let target_control = ControlDatabaseCluster::single(database);
    let router = ryframe_tenant_db::TenantDatabaseRouter::new(
        target_control.clone(),
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .map_err(|error| AppError::Config(error.to_string()))?;
    Ok(ryframe_tenant_db::application_ports::backup::verifier(
        target_control,
        Arc::new(router),
        config.scope_id.as_str().into(),
    ))
}

impl Context {
    pub async fn close(self) -> AppResult<()> {
        use ryframe_db::DbResultExt;
        if let Some(database) = self.target_database {
            database.close().await.db()?;
        }
        Ok(())
    }
}

pub fn validate_isolation(source: &AppConfig, target: &AppConfig) -> AppResult<()> {
    if target.scope_id == source.scope_id || target.auth.jwt_secret == source.auth.jwt_secret {
        return Err(AppError::Config(
            "恢复必须使用独立 scope 和新的 JWT 密钥，旧会话不能恢复为有效状态".into(),
        ));
    }
    if target.jobs.mode != JobWorkerMode::External {
        return Err(AppError::Config("恢复演练必须启用 external Worker".into()));
    }
    if let (Some(source), Some(target)) = (&source.redis, &target.redis)
        && source.namespace() == target.namespace()
    {
        return Err(AppError::Config("恢复 Redis 必须使用独立 namespace".into()));
    }
    Ok(())
}

fn ready_url(host: &str, port: u16) -> String {
    let host = match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        value => value,
    };
    if host.contains(':') {
        format!("http://[{host}]:{port}/readyz")
    } else {
        format!("http://{host}:{port}/readyz")
    }
}

fn endpoint(config: &AppConfig) -> AppResult<String> {
    if config.object_storage.backend == StorageBackend::Local {
        std::fs::canonicalize(PathBuf::from(&config.object_storage.local_base_dir))
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|_| AppError::Config("对象根目录必须已显式初始化".into()))
    } else {
        Ok(config.object_storage.endpoint.clone())
    }
}

fn storage(config: &AppConfig) -> AppResult<Arc<dyn ObjectStorage>> {
    let settings = &config.object_storage;
    match settings.backend {
        StorageBackend::Local => Ok(Arc::new(LocalObjectStorage::new(&settings.local_base_dir))),
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => Ok(Arc::new(
            S3ObjectStorage::new(S3Config {
                endpoint: settings.endpoint.clone(),
                access_key: settings.access_key.clone(),
                secret_key: settings.secret_key.clone(),
                use_ssl: settings.use_ssl,
                root_ca_pem: None,
                region: settings.region.clone(),
                request_timeout_secs: settings.request_timeout_secs,
            })
            .map_err(|_| AppError::Config("对象存储备份客户端配置无效".into()))?,
        )),
    }
}
