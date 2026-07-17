use std::path::PathBuf;

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
    workspace_root: PathBuf,
}

impl GeneratorService {
    pub fn new(db: DatabaseCluster, data_source: String, workspace_root: PathBuf) -> Self {
        Self {
            db,
            data_source,
            workspace_root,
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

    /// 写入磁盘
    pub async fn generate(&self, opts: GenerateOptions) -> AppResult<WriteReport> {
        let db = self.database()?;
        let files = ryframe_generator::generate(db, &opts).await?;
        ryframe_generator::write_to_disk(&files, &self.workspace_root, opts.overwrite).await
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
