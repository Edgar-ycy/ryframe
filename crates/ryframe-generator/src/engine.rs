use std::{collections::HashSet, path::Path};

use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_entity_dir() -> String {
    "crates/ryframe-db/src/entities".into()
}
fn default_repository_dir() -> String {
    "crates/ryframe-application/src/business".into()
}
fn default_service_dir() -> String {
    "crates/ryframe-application/src/business".into()
}
fn default_handler_dir() -> String {
    "crates/ryframe-api/src/handlers/business".into()
}
fn default_dto_dir() -> String {
    "crates/ryframe-api/src/dto/business".into()
}

/// 代码生成选项（多表支持 + 路径独立配置 + 选择性生成）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GenerateOptions {
    /// 要生成的表名列表
    pub tables: Vec<String>,

    // ── 路径配置（独立控制各类文件的输出目录） ──
    #[serde(default = "default_entity_dir")]
    pub entity_dir: String,
    #[serde(default = "default_repository_dir")]
    pub repository_dir: String,
    #[serde(default = "default_service_dir")]
    pub service_dir: String,
    #[serde(default = "default_handler_dir")]
    pub handler_dir: String,
    #[serde(default = "default_dto_dir")]
    pub dto_dir: String,

    // ── 生成策略（选择性地生成某些层） ──
    #[serde(default = "default_true")]
    pub generate_entity: bool,
    #[serde(default = "default_true")]
    pub generate_repository: bool,
    #[serde(default = "default_true")]
    pub generate_service: bool,
    #[serde(default = "default_true")]
    pub generate_handler: bool,
    #[serde(default = "default_true")]
    pub generate_dto: bool,

    /// 表名前缀过滤列表，如 ["t_"] 会将 "t_gongxv" 剥离为 "gongxv"
    #[serde(default)]
    pub table_prefixes: Vec<String>,

    /// 是否在实体中生成数据库注释（字段 + 表级别）
    #[serde(default)]
    pub generate_comments: bool,

    #[serde(default)]
    pub overwrite: bool,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            tables: Vec::new(),
            entity_dir: default_entity_dir(),
            repository_dir: default_repository_dir(),
            service_dir: default_service_dir(),
            handler_dir: default_handler_dir(),
            dto_dir: default_dto_dir(),
            generate_entity: true,
            generate_repository: true,
            generate_service: true,
            generate_handler: true,
            generate_dto: true,
            table_prefixes: Vec::new(),
            generate_comments: false,
            overwrite: false,
        }
    }
}

/// 生成的文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteReport {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

/// 验证表名合法性
fn validate_table_name(name: &str) -> AppResult<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(AppError::Validation(format!("表名包含非法字符: {}", name)));
    }
    if name.contains("..") {
        return Err(AppError::Validation(format!("非法表名: {}", name)));
    }
    Ok(())
}

fn normalize_relative_path(path: &str, label: &str) -> AppResult<String> {
    let portable = path.replace('\\', "/");
    let has_drive_prefix = portable.as_bytes().get(1) == Some(&b':');
    if portable.is_empty() || portable.starts_with('/') || has_drive_prefix {
        return Err(AppError::Validation(format!(
            "{}必须是非空的工作区相对路径",
            label
        )));
    }

    let segments = portable.split('/').collect::<Vec<_>>();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
    {
        return Err(AppError::Validation(format!("{}包含非法路径片段", label)));
    }
    Ok(segments.join("/"))
}

/// 生成代码（不写盘）— 支持多表批量生成 + 路径独立配置 + 选择性生成
pub async fn generate(
    db: &sea_orm::DatabaseConnection,
    opts: &GenerateOptions,
) -> AppResult<Vec<GeneratedFile>> {
    if opts.tables.is_empty() {
        return Err(AppError::Validation("未指定要生成的表名".into()));
    }

    let entity_base = normalize_relative_path(&opts.entity_dir, "实体输出目录")?;
    let repository_base = normalize_relative_path(&opts.repository_dir, "Repository 输出目录")?;
    let service_base = normalize_relative_path(&opts.service_dir, "Service 输出目录")?;
    let handler_base = normalize_relative_path(&opts.handler_dir, "Handler 输出目录")?;
    let dto_base = normalize_relative_path(&opts.dto_dir, "DTO 输出目录")?;
    for (label, path, required_prefix) in [
        (
            "Repository",
            repository_base.as_str(),
            "crates/ryframe-application/src/business",
        ),
        (
            "Service",
            service_base.as_str(),
            "crates/ryframe-application/src/business",
        ),
        (
            "Handler",
            handler_base.as_str(),
            "crates/ryframe-api/src/handlers/business",
        ),
        (
            "DTO",
            dto_base.as_str(),
            "crates/ryframe-api/src/dto/business",
        ),
    ] {
        if path != required_prefix && !path.starts_with(&format!("{required_prefix}/")) {
            return Err(AppError::Validation(format!(
                "biz_ {label} 输出必须位于 {required_prefix} 边界内"
            )));
        }
    }

    let mut all_files: Vec<GeneratedFile> = Vec::new();
    let mut generated_paths = HashSet::new();

    let mut tables = Vec::with_capacity(opts.tables.len());
    for table_name in &opts.tables {
        validate_table_name(table_name)?;
        let table = crate::schema::fetch_table(db, table_name).await?;
        validate_business_table(&table)?;
        tables.push(table);
    }
    let catalog_tables = order_catalog_tables(&tables)?;

    for table in &tables {
        let table_name = &table.table_name;
        let base_name = crate::naming::strip_prefixes(table_name, &opts.table_prefixes);
        if base_name.is_empty() {
            return Err(AppError::Validation(format!(
                "表 {} 去除前缀后名称为空",
                table_name
            )));
        }
        let snake = crate::naming::to_snake_case(&base_name);

        if opts.generate_entity {
            let content =
                crate::template::entity::render_entity(table, &base_name, opts.generate_comments);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}.rs", entity_base, snake),
                content,
            )?;
        }

        if opts.generate_repository {
            let content = crate::template::repository::render_repository(table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_repo.rs", repository_base, snake),
                content,
            )?;
        }

        if opts.generate_dto {
            let content = crate::template::dto::render_dto(table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_dto.rs", dto_base, snake),
                content,
            )?;
        }

        if opts.generate_service {
            let content = crate::template::service::render_service(table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_service.rs", service_base, snake),
                content,
            )?;
        }

        if opts.generate_handler {
            let content = crate::template::handler::render_handler(table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_handler.rs", handler_base, snake),
                content,
            )?;
        }
    }

    let catalog = crate::template::catalog::render_catalog(&catalog_tables);
    push_generated_file(
        &mut all_files,
        &mut generated_paths,
        "crates/ryframe-tenant-db-migration/src/generated_catalog.rs".into(),
        catalog,
    )?;

    Ok(all_files)
}

fn validate_business_table(table: &crate::schema::TableInfo) -> AppResult<()> {
    if !table.table_name.starts_with("biz_")
        || !table
            .table_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || matches!(
            table.table_name.as_str(),
            "biz_tenant_fence" | "biz_tenant_target_slot"
        )
    {
        return Err(AppError::Validation(format!(
            "代码生成器只允许直接受租户数据路由保护的 biz_ 业务表，拒绝 {}",
            table.table_name
        )));
    }
    let tenant_column = table
        .columns
        .iter()
        .find(|column| column.name == "tenant_id")
        .ok_or_else(|| {
            AppError::Validation(format!(
                "业务表 {} 必须直接包含 tenant_id 列",
                table.table_name
            ))
        })?;
    if tenant_column.is_nullable {
        return Err(AppError::Validation(format!(
            "业务表 {} 的 tenant_id 不得为 NULL",
            table.table_name
        )));
    }
    if table.columns.iter().any(|column| {
        column.name.is_empty()
            || !column
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return Err(AppError::Validation(format!(
            "业务表 {} 的列名必须是安全 ASCII 标识符",
            table.table_name
        )));
    }
    let primary_key_count = table
        .columns
        .iter()
        .filter(|column| column.is_primary_key)
        .count();
    if primary_key_count < 2 {
        return Err(AppError::Validation(format!(
            "表 {} 必须使用 tenant_id 开头且至少包含一个业务标识的复合主键，当前主键列为 {} 个",
            table.table_name, primary_key_count
        )));
    }
    let primary = table
        .indexes
        .iter()
        .find(|index| index.name == "PRIMARY")
        .ok_or_else(|| AppError::Validation(format!("表 {} 缺少主键索引", table.table_name)))?;
    if primary.columns.len() < 2 || primary.columns[0] != "tenant_id" {
        return Err(AppError::Validation(format!(
            "表 {} 的 PRIMARY 必须以 tenant_id 开头并保留全部有序业务键列",
            table.table_name
        )));
    }
    if table
        .columns
        .iter()
        .filter(|column| column.is_primary_key && column.name != "tenant_id")
        .any(|column| column.is_auto_increment)
    {
        return Err(AppError::Validation(format!(
            "业务表 {} 的复合租户主键不得使用 auto_increment，请使用分布式 ID",
            table.table_name
        )));
    }
    for index in table.indexes.iter().filter(|index| index.unique) {
        if !index.columns.iter().any(|column| column == "tenant_id") {
            return Err(AppError::Validation(format!(
                "业务表 {} 的唯一索引 {} 必须包含 tenant_id",
                table.table_name, index.name
            )));
        }
    }
    for foreign_key in &table.foreign_keys {
        let local_tenant = foreign_key
            .columns
            .iter()
            .position(|column| column == "tenant_id");
        let referenced_tenant = foreign_key
            .referenced_columns
            .iter()
            .position(|column| column == "tenant_id");
        if local_tenant.is_none() || local_tenant != referenced_tenant {
            return Err(AppError::Validation(format!(
                "业务表 {} 的外键 {} 必须在相同序位包含 tenant_id→tenant_id",
                table.table_name, foreign_key.name
            )));
        }
    }
    for dependency in &table.foreign_key_dependencies {
        if !dependency.starts_with("biz_") || dependency == "biz_tenant_fence" {
            return Err(AppError::Validation(format!(
                "业务表 {} 不得跨租户数据 catalog 外键引用 {}",
                table.table_name, dependency
            )));
        }
    }
    Ok(())
}

fn order_catalog_tables(
    tables: &[crate::schema::TableInfo],
) -> AppResult<Vec<&crate::schema::TableInfo>> {
    let selected = tables
        .iter()
        .map(|table| table.table_name.as_str())
        .collect::<HashSet<_>>();
    for table in tables {
        if let Some(missing) = table
            .foreign_key_dependencies
            .iter()
            .find(|dependency| !selected.contains(dependency.as_str()))
        {
            return Err(AppError::Validation(format!(
                "生成 {} 时必须同时选择其 catalog 依赖 {}",
                table.table_name, missing
            )));
        }
    }

    let mut remaining = tables.iter().collect::<Vec<_>>();
    let mut ordered = Vec::with_capacity(tables.len());
    while !remaining.is_empty() {
        let completed = ordered
            .iter()
            .map(|table: &&crate::schema::TableInfo| table.table_name.as_str())
            .collect::<HashSet<_>>();
        let Some(index) = remaining.iter().position(|table| {
            table
                .foreign_key_dependencies
                .iter()
                .all(|dependency| completed.contains(dependency.as_str()))
        }) else {
            return Err(AppError::Validation(
                "所选业务表的外键依赖存在环，无法生成 TenantDataCatalog".into(),
            ));
        };
        ordered.push(remaining.remove(index));
    }
    Ok(ordered)
}

fn push_generated_file(
    files: &mut Vec<GeneratedFile>,
    paths: &mut HashSet<String>,
    path: String,
    content: String,
) -> AppResult<()> {
    if !paths.insert(path.clone()) {
        return Err(AppError::Validation(format!(
            "多个表生成了相同文件路径: {}",
            path
        )));
    }
    files.push(GeneratedFile { path, content });
    Ok(())
}

/// 写入磁盘（仅在 overwrite=true 或文件不存在时）
pub async fn write_to_disk(
    files: &[GeneratedFile],
    workspace_root: &Path,
    overwrite: bool,
) -> AppResult<WriteReport> {
    let mut written: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    tokio::fs::create_dir_all(workspace_root)
        .await
        .map_err(|e| AppError::Internal(format!("创建输出根目录失败: {}", e)))?;

    let canonical_workspace = tokio::fs::canonicalize(workspace_root)
        .await
        .map_err(|e| AppError::Internal(format!("解析输出目录失败: {}", e)))?;

    for f in files {
        let relative_path = normalize_relative_path(&f.path, "生成文件路径")?;
        let full_path = canonical_workspace.join(&relative_path);

        if let Some(parent) = full_path.parent() {
            let mut existing_ancestor = parent;
            while !existing_ancestor.exists() {
                existing_ancestor = existing_ancestor
                    .parent()
                    .ok_or_else(|| AppError::Validation("生成文件路径无有效父目录".into()))?;
            }
            let canonical_ancestor = tokio::fs::canonicalize(existing_ancestor)
                .await
                .map_err(|e| AppError::Internal(format!("解析输出目录失败: {}", e)))?;
            if !canonical_ancestor.starts_with(&canonical_workspace) {
                return Err(AppError::Validation("生成文件路径超出工作区".into()));
            }

            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Internal(format!("创建目录失败: {}", e)))?;
            let canonical_parent = tokio::fs::canonicalize(parent)
                .await
                .map_err(|e| AppError::Internal(format!("解析输出目录失败: {}", e)))?;
            if !canonical_parent.starts_with(&canonical_workspace) {
                return Err(AppError::Validation("生成文件路径超出工作区".into()));
            }
        }

        if full_path.exists() && !overwrite {
            skipped.push(relative_path);
        } else {
            tokio::fs::write(&full_path, &f.content)
                .await
                .map_err(|e| AppError::Internal(format!("写文件失败: {}", e)))?;
            written.push(relative_path);
        }
    }

    Ok(WriteReport { written, skipped })
}
