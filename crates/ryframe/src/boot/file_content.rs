use std::sync::Arc;

use ryframe_adapters::file_upload::{compress_image, get_content_type, validate_file_signature};
use ryframe_application::{FileContentFuture, FileContentProcessor, ProcessedFileContent};

struct FileContentBridge;

impl FileContentProcessor for FileContentBridge {
    fn process(
        &self,
        original_name: String,
        data: Vec<u8>,
        compress: bool,
    ) -> FileContentFuture<'_> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || process_blocking(original_name, data, compress))
                .await
                .map_err(|error| {
                    ryframe_kernel::AppError::Internal(format!("文件内容处理任务失败: {error}"))
                })?
        })
    }
}

pub fn processor() -> Arc<dyn FileContentProcessor> {
    Arc::new(FileContentBridge)
}

fn process_blocking(
    original_name: String,
    data: Vec<u8>,
    compress: bool,
) -> ryframe_kernel::AppResult<ProcessedFileContent> {
    validate_file_signature(&original_name, &data)?;
    let original_size = data.len();
    let (data, file_name) = if compress {
        match compress_image(&data, &original_name) {
            Ok((compressed, compressed_name)) => {
                if compressed.len() < original_size {
                    let saved_pct = (1.0 - compressed.len() as f64 / original_size as f64) * 100.0;
                    tracing::info!(
                        original_size,
                        compressed_size = compressed.len(),
                        saved_pct,
                        "图片压缩完成"
                    );
                }
                (compressed, compressed_name)
            }
            Err(error) => {
                tracing::warn!(%error, "图片压缩失败，保留原始内容");
                (data, original_name.clone())
            }
        }
    } else {
        (data, original_name.clone())
    };
    let content_type = get_content_type(&file_name);
    Ok(ProcessedFileContent {
        original_name,
        data,
        file_name,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use ryframe_kernel::AppError;

    use super::*;

    #[tokio::test]
    async fn preserves_valid_text_without_compression() {
        let processed = processor()
            .process("note.txt".into(), b"hello".to_vec(), false)
            .await
            .expect("合法文本应完成处理");

        assert_eq!(processed.original_name, "note.txt");
        assert_eq!(processed.file_name, "note.txt");
        assert_eq!(processed.data, b"hello");
        assert_eq!(processed.content_type, "text/plain");
    }

    #[tokio::test]
    async fn rejects_content_that_does_not_match_extension() {
        let result = processor()
            .process("image.png".into(), b"not-a-png".to_vec(), false)
            .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }
}
