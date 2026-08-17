use std::path::Path;

use ryframe_kernel::{AppError, AppResult};

use super::{
    defaults::{apply_job_mode_default, apply_migration_mode_default},
    environment_overrides::apply_env_overrides,
    security::{
        reject_production_file_secrets, reject_removed_secret_encoding, resolve_snowflake_worker_id,
    },
    validation::reject_removed_database_fields,
};
use crate::{AppConfig, Environment, MigrationMode};

impl AppConfig {
    /// 加载配置：app.toml → app.{env}.toml → APP_* 环境变量
    ///
    /// `config_dir` 为配置文件所在目录的路径（如 `"config"` 或 `"/app/config"`）。
    /// 环境配置文件仅需包含要覆盖的字段，不要求完整。
    pub fn load(config_dir: impl AsRef<Path>, environment: Environment) -> AppResult<Self> {
        let mut table = load_merged_table(config_dir.as_ref(), environment)?;
        if environment.is_production() {
            reject_production_file_secrets(&table)?;
        }
        apply_env_overrides(&mut table)?;
        reject_removed_secret_encoding(&table)?;
        let migration_mode_was_explicit = table
            .get("database")
            .and_then(toml::Value::as_table)
            .is_some_and(|database| database.contains_key("migration_mode"));
        let job_mode_was_explicit = table
            .get("jobs")
            .and_then(toml::Value::as_table)
            .is_some_and(|jobs| jobs.contains_key("mode"));
        apply_migration_mode_default(&mut table, environment);
        apply_job_mode_default(&mut table, environment);
        reject_removed_database_fields(&table)?;

        let mut config: AppConfig = table
            .try_into()
            .map_err(|error| AppError::Config(format!("配置反序列化失败: {error}")))?;

        config.environment = environment;
        config.snowflake_worker_id = resolve_snowflake_worker_id(environment)?;

        config.validate()?;
        if environment.is_production()
            && migration_mode_was_explicit
            && config.database.migration_mode != MigrationMode::Verify
        {
            return Err(AppError::Config(
                "production requires database.migration_mode = \"verify\"; run `ryframe-migrate control up` and `ryframe-migrate tenant-data up --all` before starting the API".into(),
            ));
        }
        if environment.is_production()
            && job_mode_was_explicit
            && config.jobs.mode != crate::JobWorkerMode::External
        {
            return Err(AppError::Config(
                "生产环境 jobs.mode 必须为 \"external\"；请使用独立的 ryframe-worker 进程消费任务"
                    .into(),
            ));
        }

        Ok(config)
    }

    /// 从 `APP_CONFIG_DIR` 加载配置，未设置时默认使用 `config`。
    ///
    /// 相对路径仍以进程工作目录为基准，既保留 `load("config")` 的既有行为，
    /// 也允许容器显式挂载配置目录。
    pub fn load_from_env(environment: Environment) -> AppResult<Self> {
        match std::env::var("APP_CONFIG_DIR") {
            Ok(config_dir) if config_dir.trim().is_empty() => Err(AppError::Config(
                "APP_CONFIG_DIR must not be empty when it is set".into(),
            )),
            Ok(config_dir) => Self::load(config_dir, environment),
            Err(std::env::VarError::NotPresent) => Self::load("config", environment),
            Err(std::env::VarError::NotUnicode(_)) => Err(AppError::Config(
                "APP_CONFIG_DIR must contain valid Unicode".into(),
            )),
        }
    }
}

fn load_merged_table(config_dir: &Path, environment: Environment) -> AppResult<toml::Table> {
    let base_path = config_dir.join("app.toml");
    let base_toml = std::fs::read_to_string(&base_path)
        .map_err(|error| AppError::Config(format!("无法读取 {}: {error}", base_path.display())))?;
    let mut table: toml::Table = toml::from_str(&base_toml)
        .map_err(|error| AppError::Config(format!("解析 {} 失败: {error}", base_path.display())))?;

    let env_path = config_dir.join(format!("app.{}.toml", environment.as_str()));
    match std::fs::read_to_string(&env_path) {
        Ok(env_toml) => {
            let env_table: toml::Table = toml::from_str(&env_toml).map_err(|error| {
                AppError::Config(format!("解析 {} 失败: {error}", env_path.display()))
            })?;
            merge_tables(&mut table, &env_table);
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && !environment.is_production() => {}
        Err(error) => {
            return Err(AppError::Config(format!(
                "无法读取环境配置 {}: {error}",
                env_path.display()
            )));
        }
    }

    Ok(table)
}

fn merge_tables(base: &mut toml::Table, env: &toml::Table) {
    for (key, value) in env {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(env_table)) => {
                merge_tables(base_table, env_table);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
