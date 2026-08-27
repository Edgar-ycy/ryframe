use std::sync::Arc;

#[cfg(feature = "bin-api")]
use ryframe_adapters::file_upload::compress_image;
use ryframe_adapters::file_upload::{get_content_type, validate_file_signature};
use ryframe_application::ports::files::{FileContentProcessor, ProcessedFileContent};

struct FileContentBridge;

#[async_trait::async_trait]
impl FileContentProcessor for FileContentBridge {
    async fn process(
        &self,
        original_name: String,
        data: Vec<u8>,
        compress: bool,
    ) -> ryframe_kernel::AppResult<ProcessedFileContent> {
        tokio::task::spawn_blocking(move || process_blocking(original_name, data, compress))
            .await
            .map_err(|error| {
                ryframe_kernel::AppError::Internal(format!("文件内容处理任务失败: {error}"))
            })?
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
        compress_content(data, &original_name, original_size)?
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

#[cfg(feature = "bin-api")]
fn compress_content(
    data: Vec<u8>,
    original_name: &str,
    original_size: usize,
) -> ryframe_kernel::AppResult<(Vec<u8>, String)> {
    match compress_image(&data, original_name) {
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
            Ok((compressed, compressed_name))
        }
        Err(error) => {
            tracing::warn!(%error, "图片压缩失败，保留原始内容");
            Ok((data, original_name.to_owned()))
        }
    }
}

#[cfg(not(feature = "bin-api"))]
fn compress_content(
    _data: Vec<u8>,
    _original_name: &str,
    _original_size: usize,
) -> ryframe_kernel::AppResult<(Vec<u8>, String)> {
    Err(ryframe_kernel::AppError::CapabilityUnavailable(
        "当前进程未启用图片压缩能力".into(),
    ))
}

#[cfg(all(test, feature = "bin-worker", not(feature = "bin-api")))]
mod tests {
    use super::*;
    use ryframe_kernel::AppError;

    #[test]
    fn worker_preserves_spreadsheet_content_without_compression() {
        let data = vec![0x50, 0x4B, 0x03, 0x04];
        let processed = process_blocking("report.xlsx".into(), data.clone(), false)
            .expect("工作进程应允许未压缩的表格内容");

        assert_eq!(processed.file_name, "report.xlsx");
        assert_eq!(processed.data, data);
        assert_eq!(
            processed.content_type,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
    }

    #[test]
    fn worker_rejects_image_compression_when_capability_is_absent() {
        let png_signature = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let error = match process_blocking("avatar.png".into(), png_signature, true) {
            Err(error) => error,
            Ok(_) => panic!("工作进程不得静默忽略压缩请求"),
        };

        assert!(matches!(
            error,
            AppError::CapabilityUnavailable(message) if message.contains("未启用图片压缩能力")
        ));
    }
}
