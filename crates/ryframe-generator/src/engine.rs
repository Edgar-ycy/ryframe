use ryframe_common::AppResult;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 代码生成选项
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GenerateOptions {
    pub table_name: String,
    pub module: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

/// 生成的文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// 生成代码（不写盘）
pub async fn generate(
    db: &sea_orm::DatabaseConnection,
    opts: &GenerateOptions,
) -> AppResult<Vec<GeneratedFile>> {
    // 安全检查：表名白名单
    if !opts
        .table_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
        || opts.table_name.is_empty()
    {
        return Err(ryframe_common::AppError::Validation(
            "表名包含非法字符".into(),
        ));
    }

    if opts.table_name.contains("..") {
        return Err(ryframe_common::AppError::Validation("非法表名".into()));
    }

    let table = crate::schema::fetch_table(db, &opts.table_name).await?;

    let entity = crate::template::entity::render_entity(&table);
    let repository = crate::template::repository::render_repository(&table, &opts.module);
    let dto = crate::template::dto::render_dto(&table);
    let service = crate::template::service::render_service(&table, &opts.module);
    let handler = crate::template::handler::render_handler(&table, &opts.module);

    let module_path = format!("crates/ryframe-{}/src/", opts.module);
    let snake = crate::naming::to_snake_case(&opts.table_name);

    Ok(vec![
        GeneratedFile {
            path: format!("{}entities/{}.rs", module_path, snake),
            content: entity,
        },
        GeneratedFile {
            path: format!("{}repositories/{}_repo.rs", module_path, snake),
            content: repository,
        },
        GeneratedFile {
            path: format!("{}dto/{}_dto.rs", module_path, snake),
            content: dto,
        },
        GeneratedFile {
            path: format!("{}service/{}_service.rs", module_path, snake),
            content: service,
        },
        GeneratedFile {
            path: format!("{}handlers/{}_handler.rs", module_path, snake),
            content: handler,
        },
    ])
}

/// 写入磁盘（仅在 overwrite=true 或文件不存在时）
pub async fn write_to_disk(
    files: &[GeneratedFile],
    workspace_root: &Path,
) -> AppResult<Vec<String>> {
    let mut written: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for f in files {
        let full_path = workspace_root.join(&f.path);

        // 安全检查：路径必须在 workspace_root 内
        let canonical_workspace = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let canonical_target = full_path
            .parent()
            .map(|p| {
                tokio::runtime::Handle::current().block_on(async {
                    std::fs::create_dir_all(p).ok();
                });
                p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
            })
            .unwrap_or_else(|| full_path.clone());

        if !canonical_target.starts_with(&canonical_workspace) {
            return Err(ryframe_common::AppError::Internal(
                "路径穿越攻击被阻止".into(),
            ));
        }

        if full_path.exists() {
            skipped.push(f.path.clone());
        } else {
            std::fs::create_dir_all(full_path.parent().unwrap())
                .map_err(|e| ryframe_common::AppError::Internal(format!("创建目录失败: {}", e)))?;
            std::fs::write(&full_path, &f.content)
                .map_err(|e| ryframe_common::AppError::Internal(format!("写文件失败: {}", e)))?;
            written.push(f.path.clone());
        }
    }

    if !skipped.is_empty() {
        log::warn!("以下文件已存在，跳过写入: {:?}", skipped);
    }

    Ok(written)
}
