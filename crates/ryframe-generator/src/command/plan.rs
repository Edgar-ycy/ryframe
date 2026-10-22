use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use ryframe_config::ToolSettings;

/// 生成操作；`Configured` 按工具配置中的 tables 选择导入或模型生成。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum GenerationOperation {
    #[default]
    Configured,
    Resource,
    Import,
    NewBusiness(String),
}

/// CLI 和 Rust 代码入口共同使用的请求，不读取运行时密码或访问数据库。
#[derive(Clone, Debug)]
pub struct GenerationBuilder {
    root: PathBuf,
    configuration_directory: Option<PathBuf>,
    operation: GenerationOperation,
    package: Option<String>,
    models: Option<Vec<String>>,
    tables: Option<Vec<String>>,
    source: Option<String>,
    database: Option<String>,
    prefix: Option<String>,
    sync_frontend: Option<bool>,
    write: bool,
}

/// 已解析的任务图。构建计划只读取工具配置，执行必须显式调用 `execute`。
#[derive(Clone, Debug)]
pub struct GenerationPlan {
    pub workspace_root: PathBuf,
    pub config_directory: PathBuf,
    pub operation: GenerationOperation,
    pub package: String,
    pub models: Vec<String>,
    pub tables: Vec<String>,
    pub source: String,
    pub database: String,
    pub strip_table_prefix: Option<String>,
    pub write: bool,
    pub sync_frontend: bool,
    pub(super) settings: ToolSettings,
}

impl GenerationBuilder {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            root: workspace_root.into(),
            configuration_directory: None,
            operation: GenerationOperation::Configured,
            package: None,
            models: None,
            tables: None,
            source: None,
            database: None,
            prefix: None,
            sync_frontend: None,
            write: false,
        }
    }

    pub fn operation(mut self, operation: GenerationOperation) -> Self {
        self.operation = operation;
        self
    }

    /// 显式配置目录优先于工具文件；相对路径以后端工作区为基准。
    pub fn configuration_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.configuration_directory = Some(directory.into());
        self
    }

    pub fn package(mut self, package: impl Into<String>) -> Self {
        self.package = Some(package.into());
        self
    }

    pub fn include_tables(mut self, tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tables
            .get_or_insert_default()
            .extend(tables.into_iter().map(Into::into));
        self
    }

    pub fn include_models(mut self, models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.models
            .get_or_insert_default()
            .extend(models.into_iter().map(Into::into));
        self
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    pub fn strip_table_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    pub fn sync_frontend(mut self, enabled: bool) -> Self {
        self.sync_frontend = Some(enabled);
        self
    }

    pub fn write(mut self, enabled: bool) -> Self {
        self.write = enabled;
        self
    }

    pub fn plan(self) -> Result<GenerationPlan, String> {
        let settings =
            ToolSettings::load(&self.root).map_err(|error| error.message().to_owned())?;
        let defaults = &settings.generator;
        let config_directory = resolve_configuration_directory(
            &self.root,
            self.configuration_directory
                .or_else(|| defaults.config_dir.clone()),
        )?;
        let tables = self.tables.unwrap_or_else(|| defaults.tables.clone());
        let explicit_models = self
            .models
            .as_ref()
            .is_some_and(|models| !models.is_empty());
        let models = self.models.unwrap_or_else(|| defaults.models.clone());
        let operation = match self.operation {
            GenerationOperation::Configured if tables.is_empty() => GenerationOperation::Resource,
            GenerationOperation::Configured => GenerationOperation::Import,
            GenerationOperation::NewBusiness(module) => GenerationOperation::NewBusiness(
                module.strip_suffix("-business").unwrap_or(&module).into(),
            ),
            operation => operation,
        };
        if operation == GenerationOperation::Import && explicit_models {
            return Err(
                "导入以 tables 作为完整批次，不能同时 include_models；模型选择用于 resource 操作"
                    .into(),
            );
        }
        let models = if operation == GenerationOperation::Import {
            Vec::new()
        } else {
            models
        };
        let package = if let GenerationOperation::NewBusiness(module) = &operation {
            format!("{module}-business")
        } else {
            self.package
                .or_else(|| defaults.package.clone())
                .unwrap_or_default()
        };
        let plan = GenerationPlan {
            workspace_root: self.root,
            config_directory,
            operation,
            package,
            models,
            tables,
            source: self
                .source
                .or_else(|| defaults.source.clone())
                .unwrap_or_else(|| "local".into()),
            database: self
                .database
                .or_else(|| defaults.storage.clone())
                .unwrap_or_default(),
            strip_table_prefix: self.prefix.or_else(|| defaults.strip_table_prefix.clone()),
            sync_frontend: self.sync_frontend.unwrap_or(defaults.sync_frontend),
            write: self.write,
            settings,
        };
        plan.validate()?;
        Ok(plan)
    }
}

impl GenerationPlan {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.operation == GenerationOperation::Configured {
            return Err("未解析的生成操作；请通过 GenerationBuilder::plan 创建计划".into());
        }
        if let GenerationOperation::NewBusiness(module) = &self.operation {
            return crate::business::validate_module(module).map_err(|error| error.to_string());
        }
        if self.package.is_empty() {
            return Err("缺少业务包：请设置 package 或配置 generator.package".into());
        }
        validate_names(&self.models, "模型")?;
        if self.operation == GenerationOperation::Import {
            if !self.models.is_empty() {
                return Err("导入以 tables 作为完整批次，不能同时选择 models".into());
            }
            if self.source.is_empty()
                || !self
                    .source
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err("结构来源必须是配置名称，不能是连接串；使用 primary、sources.<名称> 或 tenant_data.targets.<键>".into());
            }
            if self.tables.is_empty() {
                return Err("缺少导入表：请使用 --tables 或配置 generator.tables".into());
            }
            validate_names(&self.tables, "表")?;
            if !matches!(self.database.as_str(), "control" | "tenant") {
                return Err(
                    "目标类型只支持 control 或 tenant；请设置 --database 或 generator.storage"
                        .into(),
                );
            }
        }
        Ok(())
    }

    pub fn tasks(&self) -> Vec<&'static str> {
        let mut tasks = match self.operation {
            GenerationOperation::NewBusiness(_) => vec![
                "定位工作区",
                "校验业务模块",
                "创建隔离候选工作区",
                "渲染骨架与进程装配",
                "离线解析并核对锁定依赖",
                "预检所有目标文件",
            ],
            GenerationOperation::Resource => vec![
                "创建隔离候选工作区",
                "定位业务包",
                "读取锁定的资源描述符",
                "校验全部模型",
                "渲染 CRUD 与迁移",
                "预检所有目标文件",
            ],
            GenerationOperation::Import => vec![
                "定位业务包",
                "只读读取选中表结构",
                "创建隔离候选工作区",
                "校验整批表与模型",
                "渲染模型并注册描述符",
                "编译候选模型并生成 CRUD 与迁移",
                "预检所有目标文件",
            ],
            GenerationOperation::Configured => unreachable!("构建计划已解析配置操作"),
        };
        if self.sync_frontend
            && matches!(
                self.operation,
                GenerationOperation::Resource | GenerationOperation::Import
            )
        {
            tasks.extend([
                "导出本次候选 OpenAPI",
                "同步前端契约与派生文件",
                "生成业务列表表单与路由",
                "校验候选前端类型",
            ]);
        }
        tasks.push(if self.write {
            "事务写入文件"
        } else {
            "输出文件预览"
        });
        tasks
    }

    pub fn describe(&self) -> String {
        if let GenerationOperation::NewBusiness(module) = &self.operation {
            return format!(
                "生成计划：创建业务模块={module}，业务包={}，配置目录={}，写入={}\n{}\n计划未执行任务，不访问数据库、不编译描述符、不创建缓存或报告。",
                self.package,
                self.config_directory.display(),
                self.write,
                self.tasks().join(" → "),
            );
        }
        format!(
            "生成计划：操作={:?}，业务包={}，配置目录={}，模型={:?}，只读结构来源={}，表={:?}，模型目标={}，写入={}\n{}\n计划未执行任务，不访问数据库、不编译描述符、不创建缓存或报告。",
            self.operation,
            self.package,
            self.config_directory.display(),
            self.models,
            self.source,
            self.tables,
            self.database,
            self.write,
            self.tasks().join(" → "),
        )
    }
}

fn resolve_configuration_directory(
    root: &Path,
    selected: Option<PathBuf>,
) -> Result<PathBuf, String> {
    let directory = selected.unwrap_or_else(|| PathBuf::from("config"));
    if directory.to_str().is_none_or(|value| {
        value.trim().is_empty()
            || value
                .chars()
                .any(|character| matches!(character, '\0' | '\r' | '\n'))
    }) {
        return Err("生成器 config_dir 必须是非空 UTF-8 路径且不得包含 NUL、CR 或 LF".into());
    }
    Ok(if directory.is_absolute() {
        directory
    } else {
        root.join(directory)
    })
}

fn validate_names(names: &[String], kind: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for name in names {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!("{kind}名 {name:?} 只能包含字母、数字和下划线"));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("{kind}名重复：{name}；请删除重复选择"));
        }
    }
    Ok(())
}
