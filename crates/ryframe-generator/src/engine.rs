use std::{collections::HashSet, path::Path};

use ryframe_common::AppResult;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

fn default_true() -> bool {
    true
}

fn default_entity_dir() -> String {
    "crates/ryframe-db/src/entities".into()
}
fn default_repository_dir() -> String {
    "crates/ryframe-db/src/repositories".into()
}
fn default_service_dir() -> String {
    "crates/ryframe-service/src/system".into()
}
fn default_handler_dir() -> String {
    "crates/ryframe-api/src/handlers".into()
}
fn default_dto_dir() -> String {
    "crates/ryframe-api/src/dto".into()
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct WriteReport {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
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

fn normalize_relative_path(path: &str, label: &str) -> AppResult<String> {
    let portable = path.replace('\\', "/");
    let has_drive_prefix = portable.as_bytes().get(1) == Some(&b':');
    if portable.is_empty() || portable.starts_with('/') || has_drive_prefix {
        return Err(ryframe_common::AppError::Validation(format!(
            "{}必须是非空的工作区相对路径",
            label
        )));
    }

    let segments = portable.split('/').collect::<Vec<_>>();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
    {
        return Err(ryframe_common::AppError::Validation(format!(
            "{}包含非法路径片段",
            label
        )));
    }
    Ok(segments.join("/"))
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

    let entity_base = normalize_relative_path(&opts.entity_dir, "实体输出目录")?;
    let repository_base = normalize_relative_path(&opts.repository_dir, "Repository 输出目录")?;
    let service_base = normalize_relative_path(&opts.service_dir, "Service 输出目录")?;
    let handler_base = normalize_relative_path(&opts.handler_dir, "Handler 输出目录")?;
    let dto_base = normalize_relative_path(&opts.dto_dir, "DTO 输出目录")?;

    let mut all_files: Vec<GeneratedFile> = Vec::new();
    let mut generated_paths = HashSet::new();

    for table_name in &opts.tables {
        validate_table_name(table_name)?;

        let table = crate::schema::fetch_table(db, table_name).await?;
        let primary_key_count = table
            .columns
            .iter()
            .filter(|column| column.is_primary_key)
            .count();
        if primary_key_count != 1 {
            return Err(ryframe_common::AppError::Validation(format!(
                "表 {} 必须且只能包含一个主键，当前为 {} 个",
                table_name, primary_key_count
            )));
        }
        let base_name = crate::naming::strip_prefixes(table_name, &opts.table_prefixes);
        if base_name.is_empty() {
            return Err(ryframe_common::AppError::Validation(format!(
                "表 {} 去除前缀后名称为空",
                table_name
            )));
        }
        let snake = crate::naming::to_snake_case(&base_name);

        if opts.generate_entity {
            let content =
                crate::template::entity::render_entity(&table, &base_name, opts.generate_comments);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}.rs", entity_base, snake),
                content,
            )?;
        }

        if opts.generate_repository {
            let content = crate::template::repository::render_repository(&table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_repo.rs", repository_base, snake),
                content,
            )?;
        }

        if opts.generate_dto {
            let content = crate::template::dto::render_dto(&table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_dto.rs", dto_base, snake),
                content,
            )?;
        }

        if opts.generate_service {
            let content = crate::template::service::render_service(&table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_service.rs", service_base, snake),
                content,
            )?;
        }

        if opts.generate_handler {
            let content = crate::template::handler::render_handler(&table, &base_name);
            push_generated_file(
                &mut all_files,
                &mut generated_paths,
                format!("{}/{}_handler.rs", handler_base, snake),
                content,
            )?;
        }
    }

    Ok(all_files)
}

fn push_generated_file(
    files: &mut Vec<GeneratedFile>,
    paths: &mut HashSet<String>,
    path: String,
    content: String,
) -> AppResult<()> {
    if !paths.insert(path.clone()) {
        return Err(ryframe_common::AppError::Validation(format!(
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
        .map_err(|e| ryframe_common::AppError::Internal(format!("创建输出根目录失败: {}", e)))?;

    let canonical_workspace = tokio::fs::canonicalize(workspace_root)
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("解析输出目录失败: {}", e)))?;

    for f in files {
        let relative_path = normalize_relative_path(&f.path, "生成文件路径")?;
        let full_path = canonical_workspace.join(&relative_path);

        if let Some(parent) = full_path.parent() {
            let mut existing_ancestor = parent;
            while !existing_ancestor.exists() {
                existing_ancestor = existing_ancestor.parent().ok_or_else(|| {
                    ryframe_common::AppError::Validation("生成文件路径无有效父目录".into())
                })?;
            }
            let canonical_ancestor =
                tokio::fs::canonicalize(existing_ancestor)
                    .await
                    .map_err(|e| {
                        ryframe_common::AppError::Internal(format!("解析输出目录失败: {}", e))
                    })?;
            if !canonical_ancestor.starts_with(&canonical_workspace) {
                return Err(ryframe_common::AppError::Validation(
                    "生成文件路径超出工作区".into(),
                ));
            }

            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ryframe_common::AppError::Internal(format!("创建目录失败: {}", e)))?;
            let canonical_parent = tokio::fs::canonicalize(parent).await.map_err(|e| {
                ryframe_common::AppError::Internal(format!("解析输出目录失败: {}", e))
            })?;
            if !canonical_parent.starts_with(&canonical_workspace) {
                return Err(ryframe_common::AppError::Validation(
                    "生成文件路径超出工作区".into(),
                ));
            }
        }

        if full_path.exists() && !overwrite {
            skipped.push(relative_path);
        } else {
            tokio::fs::write(&full_path, &f.content)
                .await
                .map_err(|e| ryframe_common::AppError::Internal(format!("写文件失败: {}", e)))?;
            written.push(relative_path);
        }
    }

    Ok(WriteReport { written, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_paths_must_stay_relative_to_the_workspace() {
        for invalid in [
            "",
            "/tmp/out",
            "C:/tmp/out",
            "../out",
            "a/../out",
            "a\\..\\out",
        ] {
            assert!(
                normalize_relative_path(invalid, "test").is_err(),
                "{invalid}"
            );
        }
        assert_eq!(
            normalize_relative_path("crates\\app\\src", "test").unwrap(),
            "crates/app/src"
        );
    }

    #[test]
    fn duplicate_generated_paths_are_rejected() {
        let mut files = Vec::new();
        let mut paths = HashSet::new();
        push_generated_file(&mut files, &mut paths, "src/user.rs".into(), "one".into()).unwrap();
        assert!(
            push_generated_file(&mut files, &mut paths, "src/user.rs".into(), "two".into())
                .is_err()
        );
    }

    #[tokio::test]
    async fn write_report_distinguishes_written_and_skipped_files() {
        let workspace = tempfile::tempdir().unwrap();
        let files = vec![GeneratedFile {
            path: "src/generated.rs".into(),
            content: "pub struct Generated;\n".into(),
        }];

        let first = write_to_disk(&files, workspace.path(), false)
            .await
            .unwrap();
        assert_eq!(first.written, vec!["src/generated.rs"]);
        assert!(first.skipped.is_empty());

        let second = write_to_disk(&files, workspace.path(), false)
            .await
            .unwrap();
        assert!(second.written.is_empty());
        assert_eq!(second.skipped, vec!["src/generated.rs"]);
    }
}
