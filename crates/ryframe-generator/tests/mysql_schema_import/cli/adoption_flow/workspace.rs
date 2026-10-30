use std::{fs, path::PathBuf};

use sea_orm::DatabaseConnection;

use super::super::{CapturedConfiguration, PreparedConfiguration};
use super::{GenerationBuilder, GenerationOperation, message, snapshot};

pub(super) struct Fixture {
    pub(super) directory: PathBuf,
    pub(super) root: PathBuf,
    pub(super) original: snapshot::SourceSnapshot,
    pub(super) configuration: PreparedConfiguration,
    pub(super) migration_configuration: PathBuf,
    configuration_bytes: Vec<(PathBuf, Vec<u8>)>,
    pub(super) completed: bool,
}

impl Fixture {
    pub(super) fn new(schema: &str) -> Result<Self, String> {
        let captured =
            CapturedConfiguration::capture(&super::super::shared_configuration_directory()?)?;
        let original = snapshot::SourceSnapshot::capture(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        )?;
        let parent = original.root.join(".local-tests/cli-adoption-flow");
        fs::create_dir_all(&parent).map_err(message)?;
        // 外层 fixture 还要核验数据库清理；此前保留源码和收据，避免清理失败丢失前像。
        let directory = tempfile::tempdir_in(&parent).map_err(message)?.keep();
        eprintln!("导入接管闭环独占现场：{}", directory.display());
        let root = directory.join("backend");
        original.copy_to(&root)?;
        super::super::frontend::configure_target(&root, &original.root)?;
        GenerationBuilder::new(&root)
            .operation(GenerationOperation::NewBusiness("cli-import".into()))
            .write(true)
            .sync_frontend(false)
            .plan()?
            .execute()?;
        let configuration = captured.prepare(&root, schema)?;
        let migration_configuration = root.join(".local-tests/adoption-migration-config");
        fs::create_dir(&migration_configuration).map_err(message)?;
        let mut configuration_bytes = Vec::new();
        for name in ["app.toml", "app.test.toml"] {
            let source = configuration.directory()?.join(name);
            if !source.exists() {
                continue;
            }
            let mut bytes = fs::read(source).map_err(message)?;
            if name == "app.test.toml" {
                let mut table: toml::Table =
                    toml::from_str(std::str::from_utf8(&bytes).map_err(|_| "配置不是 UTF-8")?)
                        .map_err(|_| "已捕获 TOML 解析失败")?;
                for (section, field, value) in [
                    ("multi_tenancy", "enabled", toml::Value::Boolean(false)),
                    ("api_docs", "enabled", toml::Value::Boolean(false)),
                    (
                        "bootstrap",
                        "admin_password",
                        toml::Value::String("CliAdoption-Test-Password-42!".into()),
                    ),
                ] {
                    let section = table
                        .entry(section)
                        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                        .as_table_mut()
                        .ok_or("迁移配置字段不是 TOML 表")?;
                    section.insert(field.into(), value);
                }
                bytes = toml::to_string(&table)
                    .map_err(|_| "迁移配置序列化失败")?
                    .into_bytes();
            }
            let path = migration_configuration.join(name);
            fs::write(&path, &bytes).map_err(message)?;
            configuration_bytes.push((path, bytes));
        }
        ryframe_config::AppConfig::load(
            &migration_configuration,
            ryframe_config::Environment::Test,
        )
        .map_err(|_| "派生迁移配置加载失败；保留配置，不回显凭据")?;
        Ok(Self {
            directory,
            root,
            original,
            configuration,
            migration_configuration,
            configuration_bytes,
            completed: false,
        })
    }

    pub(super) fn business(&self) -> PathBuf {
        self.root.join("crates/cli-import-business")
    }

    pub(super) fn verify_configuration(&self) -> Result<(), String> {
        self.configuration.verify()?;
        for (path, bytes) in &self.configuration_bytes {
            if fs::read(path).map_err(message)? != *bytes {
                return Err("派生迁移配置字节漂移；保留现场，不重放".into());
            }
        }
        Ok(())
    }

    pub(super) async fn verify_migration_binding(
        &self,
        database: &DatabaseConnection,
    ) -> Result<(), String> {
        let config = ryframe_config::AppConfig::load(
            &self.migration_configuration,
            ryframe_config::Environment::Test,
        )
        .map_err(|_| "迁移配置无法复核")?;
        let candidate = ryframe_db::connection::connect(&config.database.primary)
            .await
            .map_err(|_| "迁移配置连接失败")?;
        let observed = ryframe_db::connection::physical_identity(&candidate)
            .await
            .map_err(message);
        let closed = candidate.close().await.map_err(message);
        assert_eq!(
            observed?,
            ryframe_db::connection::physical_identity(database)
                .await
                .map_err(message)?
        );
        closed?;
        self.verify_configuration()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.completed {
            eprintln!(
                "业务闭环完成；保留 {}，由主控确认外层 fixture 清理成功后精确清理",
                self.directory.display()
            );
        } else {
            eprintln!("保留导入接管失败现场：{}", self.directory.display());
        }
    }
}
