use std::path::Path;

use ryframe_common::AppResult;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_entity_dir() -> String {
    "src/entities".into()
}
fn default_repository_dir() -> String {
    "src/repositories".into()
}
fn default_service_dir() -> String {
    "src/service".into()
}
fn default_handler_dir() -> String {
    "src/handlers".into()
}
fn default_dto_dir() -> String {
    "src/dto".into()
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

/// 生成的文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// 验证表名合法性
fn validate_table_name(name: &str) -> AppResult<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ryframe_common::AppError::Validation(format!(
            "表名包含非法字符: {}",
            name
        )));
    }
    if name.contains("..") {
        return Err(ryframe_common::AppError::Validation(format!(
            "非法表名: {}",
            name
        )));
    }
    Ok(())
}

/// 生成代码（不写盘）— 支持多表批量生成 + 路径独立配置 + 选择性生成
pub async fn generate(
    db: &sea_orm::DatabaseConnection,
    opts: &GenerateOptions,
) -> AppResult<Vec<GeneratedFile>> {
    if opts.tables.is_empty() {
        return Err(ryframe_common::AppError::Validation(
            "未指定要生成的表名".into(),
        ));
    }

    let entity_base = opts.entity_dir.trim_end_matches('/');
    let repository_base = opts.repository_dir.trim_end_matches('/');
    let service_base = opts.service_dir.trim_end_matches('/');
    let handler_base = opts.handler_dir.trim_end_matches('/');
    let dto_base = opts.dto_dir.trim_end_matches('/');

    let mut all_files: Vec<GeneratedFile> = Vec::new();

    for table_name in &opts.tables {
        validate_table_name(table_name)?;

        let table = crate::schema::fetch_table(db, table_name).await?;
        let base_name = crate::naming::strip_prefixes(table_name, &opts.table_prefixes);
        let snake = crate::naming::to_snake_case(&base_name);

        if opts.generate_entity {
            let content =
                crate::template::entity::render_entity(&table, &base_name, opts.generate_comments);
            all_files.push(GeneratedFile {
                path: format!("{}/{}.rs", entity_base, snake),
                content,
            });
        }

        if opts.generate_repository {
            let content = crate::template::repository::render_repository(&table, &base_name);
            all_files.push(GeneratedFile {
                path: format!("{}/{}_repo.rs", repository_base, snake),
                content,
            });
        }

        if opts.generate_dto {
            let content = crate::template::dto::render_dto(&table, &base_name);
            all_files.push(GeneratedFile {
                path: format!("{}/{}_dto.rs", dto_base, snake),
                content,
            });
        }

        if opts.generate_service {
            let content = crate::template::service::render_service(&table, &base_name);
            all_files.push(GeneratedFile {
                path: format!("{}/{}_service.rs", service_base, snake),
                content,
            });
        }

        if opts.generate_handler {
            let content = crate::template::handler::render_handler(&table, &base_name);
            all_files.push(GeneratedFile {
                path: format!("{}/{}_handler.rs", handler_base, snake),
                content,
            });
        }
    }

    Ok(all_files)
}

/// 写入磁盘（仅在 overwrite=true 或文件不存在时）
pub async fn write_to_disk(
    files: &[GeneratedFile],
    workspace_root: &Path,
    overwrite: bool,
) -> AppResult<Vec<String>> {
    let mut written: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    // 先确保输出根目录存在，否则 canonicalize 会失败回退为相对路径
    tokio::fs::create_dir_all(workspace_root)
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("创建输出根目录失败: {}", e)))?;

    let canonical_workspace = tokio::fs::canonicalize(workspace_root)
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("解析输出目录失败: {}", e)))?;

    for f in files {
        let full_path = workspace_root.join(&f.path);

        // 安全检查：路径必须在 workspace_root 内
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ryframe_common::AppError::Internal(format!("创建目录失败: {}", e)))?;
            let canonical_parent = tokio::fs::canonicalize(parent).await.map_err(|e| {
                ryframe_common::AppError::Internal(format!("解析输出目录失败: {}", e))
            })?;
            if !canonical_parent.starts_with(&canonical_workspace) {
                return Err(ryframe_common::AppError::Internal(
                    "路径穿越攻击被阻止".into(),
                ));
            }
        }

        if full_path.exists() && !overwrite {
            skipped.push(f.path.clone());
        } else {
            tokio::fs::write(&full_path, &f.content)
                .await
                .map_err(|e| ryframe_common::AppError::Internal(format!("写文件失败: {}", e)))?;
            written.push(f.path.clone());
        }
    }

    if !skipped.is_empty() {
        log::warn!("以下文件已存在，跳过写入: {:?}", skipped);
    }

    Ok(written)
}
