use chrono::Utc;
use crate::{AppError, AppResult};
use std::path::PathBuf;

/// 文件上传配置
#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// 上传根目录
    pub upload_dir: String,
    /// 允许的最大文件大小（字节）
    pub max_file_size: u64,
    /// 允许的文件扩展名
    pub allowed_extensions: Vec<String>,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            upload_dir: "uploads".to_string(),
            max_file_size: 10 * 1024 * 1024, // 10MB
            allowed_extensions: vec![
                // 图片
                "jpg".to_string(),
                "jpeg".to_string(),
                "png".to_string(),
                "gif".to_string(),
                "bmp".to_string(),
                "webp".to_string(),
                // 文档
                "pdf".to_string(),
                "doc".to_string(),
                "docx".to_string(),
                "xls".to_string(),
                "xlsx".to_string(),
                "txt".to_string(),
                // 压缩文件
                "zip".to_string(),
                "rar".to_string(),
                "7z".to_string(),
            ],
        }
    }
}

/// 上传文件信息
#[derive(Debug, Clone, serde::Serialize)]
pub struct UploadFileInfo {
    /// 原始文件名
    pub original_name: String,
    /// 存储文件名（UUID + 扩展名）
    pub storage_name: String,
    /// 文件路径（相对路径）
    pub file_path: String,
    /// 文件大小（字节）
    pub file_size: u64,
    /// 文件 MIME 类型
    pub content_type: String,
    /// 上传时间
    pub upload_time: String,
}

/// 验证文件扩展名
pub fn validate_extension(filename: &str, allowed: &[String]) -> AppResult<()> {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    if allowed.is_empty() || allowed.contains(&ext) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "不支持的文件类型: .{}",
            ext
        )))
    }
}

/// 生成存储文件名（UUID + 原始扩展名）
pub fn generate_storage_filename(original_name: &str) -> String {
    let ext = original_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    let uuid = uuid::Uuid::new_v4();
    if ext.is_empty() {
        format!("{}", uuid)
    } else {
        format!("{}.{}", uuid, ext)
    }
}

/// 获取按日期组织的上传目录路径
pub fn get_upload_dir(upload_dir: &str) -> PathBuf {
    let now = Utc::now();
    let date_str = now.format("%Y/%m/%d");
    PathBuf::from(upload_dir).join(date_str.to_string())
}

/// 获取文件的 MIME 类型
pub fn get_content_type(filename: &str) -> String {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // 图片
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "png" => "image/png".to_string(),
        "gif" => "image/gif".to_string(),
        "bmp" => "image/bmp".to_string(),
        "webp" => "image/webp".to_string(),
        // 文档
        "pdf" => "application/pdf".to_string(),
        "doc" => "application/msword".to_string(),
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        "xls" => "application/vnd.ms-excel".to_string(),
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
        "txt" => "text/plain".to_string(),
        // 压缩文件
        "zip" => "application/zip".to_string(),
        "rar" => "application/x-rar-compressed".to_string(),
        "7z" => "application/x-7z-compressed".to_string(),
        // 默认
        _ => "application/octet-stream".to_string(),
    }
}

/// 格式化文件大小
pub fn format_file_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.2} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.2} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
