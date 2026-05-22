use ryframe_common::AppResult;
use ryframe_generator::{GenerateOptions, GeneratedFile};
use sea_orm::DatabaseConnection;
use std::path::PathBuf;

pub struct GeneratorServiceImpl {
    pub workspace_root: PathBuf,
}

impl GeneratorServiceImpl {
    /// 列出数据库所有表（供前端选择）
    pub async fn list_tables(
        &self,
        db: &DatabaseConnection,
    ) -> AppResult<Vec<ryframe_generator::TableInfo>> {
        let table_names = ryframe_generator::list_tables(db).await?;
        let mut tables = Vec::new();
        for name in table_names {
            if let Ok(info) = ryframe_generator::fetch_table(db, &name).await {
                tables.push(info);
            }
        }
        Ok(tables)
    }

    /// 预览生成内容（不写盘）
    pub async fn preview(
        &self,
        db: &DatabaseConnection,
        opts: GenerateOptions,
    ) -> AppResult<Vec<GeneratedFile>> {
        ryframe_generator::generate(db, &opts).await
    }

    /// 写入磁盘
    pub async fn generate(
        &self,
        db: &DatabaseConnection,
        opts: GenerateOptions,
    ) -> AppResult<Vec<String>> {
        let files = ryframe_generator::generate(db, &opts).await?;
        ryframe_generator::write_to_disk(&files, &self.workspace_root).await
    }

    /// 打包 zip 下载（不写盘）
    pub async fn download_zip(
        &self,
        db: &DatabaseConnection,
        opts: GenerateOptions,
    ) -> AppResult<Vec<u8>> {
        let files = ryframe_generator::generate(db, &opts).await?;
        let mut zip = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip);
            let options =
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);

            for file in &files {
                writer
                    .start_file(&file.path, options)
                    .map_err(|e| {
                        ryframe_common::AppError::Internal(format!(
                            "创建 zip 条目失败: {}",
                            e
                        ))
                    })?;
                std::io::Write::write_all(&mut writer, file.content.as_bytes()).map_err(
                    |e| {
                        ryframe_common::AppError::Internal(format!(
                            "写入 zip 内容失败: {}",
                            e
                        ))
                    },
                )?;
            }
            writer.finish().map_err(|e| {
                ryframe_common::AppError::Internal(format!("完成 zip 打包失败: {}", e))
            })?;
        }
        Ok(zip.into_inner())
    }
}
