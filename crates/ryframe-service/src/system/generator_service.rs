use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult};
use ryframe_db::DatabaseCluster;
pub use ryframe_generator::{ColumnInfo, GenerateOptions, GeneratedFile, TableInfo, WriteReport};
use sea_orm::DatabaseConnection;

#[derive(Debug)]
pub struct TableListParams {
    pub page: PageQuery,
    pub table_name: Option<String>,
    pub table_comment: Option<String>,
}

pub struct GeneratorService {
    db: DatabaseCluster,
    data_source: String,
    project_root: PathBuf,
}

impl GeneratorService {
    pub fn new(db: DatabaseCluster, data_source: String, project_root: PathBuf) -> Self {
        Self {
            db,
            data_source,
            project_root,
        }
    }

    /// 列出数据库所有表（供前端选择）
    pub async fn list_tables(&self, params: TableListParams) -> AppResult<PageResult<TableInfo>> {
        let db = self.database()?;
        let table_names = ryframe_generator::list_tables(db).await?;
        let mut tables = Vec::new();
        for name in table_names {
            tables.push(ryframe_generator::fetch_table(db, &name).await?);
        }
        let name = params.table_name.as_deref().map(str::to_lowercase);
        let comment = params.table_comment.as_deref().map(str::to_lowercase);
        let filtered = tables
            .into_iter()
            .filter(|table| {
                name.as_ref()
                    .is_none_or(|value| table.table_name.to_lowercase().contains(value))
                    && comment.as_ref().is_none_or(|value| {
                        table
                            .comment
                            .as_deref()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(value)
                    })
            })
            .collect::<Vec<_>>();
        let page = params.page.normalize(100);
        let total = filtered.len() as u64;
        let records = filtered
            .into_iter()
            .skip(page.offset() as usize)
            .take(page.page_size as usize)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }

    /// 预览生成内容（不写盘）
    pub async fn preview(&self, opts: GenerateOptions) -> AppResult<Vec<GeneratedFile>> {
        let db = self.database()?;
        ryframe_generator::generate(db, &opts).await
    }

    /// 将生成代码写入项目目录之外的指定根目录。
    pub async fn generate(
        &self,
        opts: GenerateOptions,
        output_root: PathBuf,
    ) -> AppResult<WriteReport> {
        let output_root = prepare_output_root(&self.project_root, &output_root).await?;
        let db = self.database()?;
        let files = ryframe_generator::generate(db, &opts).await?;
        ryframe_generator::write_to_disk(&files, &output_root, opts.overwrite).await
    }

    /// 打包 zip 下载（不写盘）
    pub async fn download_zip(&self, opts: GenerateOptions) -> AppResult<Vec<u8>> {
        let db = self.database()?;
        let files = ryframe_generator::generate(db, &opts).await?;
        let mut zip = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            for file in &files {
                writer.start_file(&file.path, options).map_err(|e| {
                    ryframe_common::AppError::Internal(format!("创建 zip 条目失败: {}", e))
                })?;
                std::io::Write::write_all(&mut writer, file.content.as_bytes()).map_err(|e| {
                    ryframe_common::AppError::Internal(format!("写入 zip 内容失败: {}", e))
                })?;
            }
            writer.finish().map_err(|e| {
                ryframe_common::AppError::Internal(format!("完成 zip 打包失败: {}", e))
            })?;
        }
        Ok(zip.into_inner())
    }

    fn database(&self) -> AppResult<&DatabaseConnection> {
        if self.data_source == "primary" {
            return Ok(self.db.write());
        }
        self.db.source(&self.data_source).ok_or_else(|| {
            AppError::Config(format!("代码生成器数据源未连接: {}", self.data_source))
        })
    }
}

async fn prepare_output_root(project_root: &Path, requested_root: &Path) -> AppResult<PathBuf> {
    if !requested_root.is_absolute() {
        return Err(AppError::Validation("代码输出根目录必须是绝对路径".into()));
    }
    if requested_root
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(AppError::Validation(
            "代码输出根目录不能包含 . 或 .. 路径段".into(),
        ));
    }

    let project_root = tokio::fs::canonicalize(project_root)
        .await
        .map_err(|error| AppError::Internal(format!("解析当前项目目录失败: {error}")))?;
    let output_root = resolve_pending_path(requested_root).await?;
    ensure_external_output_root(&project_root, &output_root)?;

    tokio::fs::create_dir_all(&output_root)
        .await
        .map_err(|error| AppError::Internal(format!("创建代码输出根目录失败: {error}")))?;
    let output_root = tokio::fs::canonicalize(&output_root)
        .await
        .map_err(|error| AppError::Internal(format!("解析代码输出根目录失败: {error}")))?;
    ensure_external_output_root(&project_root, &output_root)?;
    Ok(output_root)
}

async fn resolve_pending_path(path: &Path) -> AppResult<PathBuf> {
    let mut candidate = path.to_path_buf();
    let mut pending_segments = Vec::<OsString>::new();

    loop {
        match tokio::fs::canonicalize(&candidate).await {
            Ok(mut resolved) => {
                for segment in pending_segments.iter().rev() {
                    resolved.push(segment);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let segment = candidate.file_name().ok_or_else(|| {
                    AppError::Validation("代码输出根目录没有有效的已存在父目录".into())
                })?;
                pending_segments.push(segment.to_owned());
                candidate = candidate
                    .parent()
                    .ok_or_else(|| {
                        AppError::Validation("代码输出根目录没有有效的已存在父目录".into())
                    })?
                    .to_path_buf();
            }
            Err(error) => {
                return Err(AppError::Internal(format!(
                    "解析代码输出根目录失败: {error}"
                )));
            }
        }
    }
}

fn ensure_external_output_root(project_root: &Path, output_root: &Path) -> AppResult<()> {
    if output_root.starts_with(project_root) || project_root.starts_with(output_root) {
        return Err(AppError::Validation(
            "代码输出根目录不能是当前项目目录及其父目录或子目录".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_root_must_be_absolute() {
        let project = tempfile::tempdir().unwrap();

        let result = prepare_output_root(project.path(), Path::new("generated")).await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn output_root_cannot_overlap_the_current_project() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join("project");
        tokio::fs::create_dir_all(&project).await.unwrap();
        let child = project.join("generated");

        let child_result = prepare_output_root(&project, &child).await;
        let parent_result = prepare_output_root(&project, workspace.path()).await;

        assert!(matches!(child_result, Err(AppError::Validation(_))));
        assert!(matches!(parent_result, Err(AppError::Validation(_))));
        assert!(!child.exists(), "校验失败时不应创建项目内目录");
    }

    #[tokio::test]
    async fn external_output_root_is_created_and_canonicalized() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join("project");
        let output = workspace.path().join("generated").join("module");
        tokio::fs::create_dir_all(&project).await.unwrap();

        let resolved = prepare_output_root(&project, &output).await.unwrap();

        assert_eq!(resolved, tokio::fs::canonicalize(&output).await.unwrap());
        assert!(output.is_dir());
    }
}
