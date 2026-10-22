use std::{fs, io::ErrorKind, path::Path};

use ryframe_kernel::{AppError, AppResult};

use super::{
    defaults::apply_job_mode_default,
    diagnostics::{configuration_error, read_table, redact_values},
    security::reject_removed_secret_encoding,
    validation::reject_removed_database_fields,
};
use crate::{AppConfig, Environment};

mod connections;

impl AppConfig {
    /// 读取并深合并可缺省的 app.toml 和必需的 app.{env}.toml，数组整体替换。
    /// 仅执行文档层及旧字段、连接预检查；完整运行配置仍须由 load 校验。
    pub fn read_document(
        config_dir: impl AsRef<Path>,
        environment: Environment,
    ) -> AppResult<toml::Table> {
        let directory = config_dir.as_ref();
        let base_path = directory.join("app.toml");
        let environment_path = directory.join(format!("app.{}.toml", environment.as_str()));
        let mut table = read_optional_base(&base_path)?;
        merge_tables(&mut table, &read_table(&environment_path)?);
        let context = format!("{} 与 {}", base_path.display(), environment_path.display());
        let checked = || -> AppResult<()> {
            reject_removed_secret_encoding(&table)?;
            reject_removed_database_fields(&table)?;
            connections::validate(&table)
        };
        checked().map_err(|error| configuration_error(&context, &table, error))?;
        Ok(table)
    }

    /// 从文件读取完整运行配置，不读取字段环境覆盖。
    pub fn load(config_dir: impl AsRef<Path>, environment: Environment) -> AppResult<Self> {
        let directory = config_dir.as_ref();
        let mut table = Self::read_document(directory, environment)?;
        let context = format!(
            "{} 与 {}",
            directory.join("app.toml").display(),
            directory
                .join(format!("app.{}.toml", environment.as_str()))
                .display()
        );
        let job_mode_was_explicit = table
            .get("jobs")
            .and_then(toml::Value::as_table)
            .is_some_and(|jobs| jobs.contains_key("mode"));
        apply_job_mode_default(&mut table, environment);

        let mut config: AppConfig = table.clone().try_into().map_err(|mut error: toml::de::Error| {
            error.set_input(None);
            let rendered = error.to_string();
            let field_path = rendered.strip_prefix(error.message()).unwrap_or_default();
            AppError::Config(format!(
                "配置文件 {context} 字段反序列化失败：{}{field_path}；请按字段类型、必填项和允许值修正 TOML",
                redact_values(&table, error.message())
            ))
        })?;
        config.environment = environment;
        if let Some(redis) = &mut config.redis {
            redis.set_scope_id(config.scope_id.clone());
        }
        config
            .validate()
            .map_err(|error| configuration_error(&context, &table, error))?;
        if environment.is_production()
            && job_mode_was_explicit
            && config.jobs.mode != crate::JobWorkerMode::External
        {
            return Err(AppError::Config(format!(
                "配置文件 {context} 的 jobs.mode 必须为 \"external\"；请使用独立的 ryframe-worker 进程消费任务"
            )));
        }
        Ok(config)
    }
}

fn read_optional_base(path: &Path) -> AppResult<toml::Table> {
    match fs::symlink_metadata(path) {
        Ok(_) => read_table(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(error) => Err(AppError::Config(format!(
            "无法检查公共配置文件 {}（{}）；请检查路径、文件类型及访问权限",
            path.display(),
            error.kind()
        ))),
    }
}

fn merge_tables(base: &mut toml::Table, overlay: &toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(overlay_table)) => {
                merge_tables(base_table, overlay_table);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
