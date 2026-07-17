use std::path::PathBuf;

use chrono::Utc;

use crate::{AppError, AppResult};

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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();

    if allowed.is_empty() || allowed.contains(&ext) {
        Ok(())
    } else {
        Err(AppError::Validation(format!("不支持的文件类型: .{}", ext)))
    }
}

pub fn validate_file_signature(filename: &str, data: &[u8]) -> AppResult<()> {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    let valid = match ext.as_str() {
        "jpg" | "jpeg" => data.starts_with(&[0xFF, 0xD8, 0xFF]),
        "png" => data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "gif" => data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"),
        "bmp" => data.starts_with(b"BM"),
        "webp" => data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP",
        "pdf" => data.starts_with(b"%PDF-"),
        "zip" | "docx" | "xlsx" => {
            data.starts_with(&[0x50, 0x4B, 0x03, 0x04])
                || data.starts_with(&[0x50, 0x4B, 0x05, 0x06])
                || data.starts_with(&[0x50, 0x4B, 0x07, 0x08])
        }
        "rar" => data.starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07]),
        "7z" => data.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]),
        "txt" => std::str::from_utf8(data).is_ok() && !data.contains(&0),
        "doc" | "xls" => data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]),
        _ => false,
    };

    if valid {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "文件内容与扩展名不匹配: .{}",
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
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();

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
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string()
        }
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

/// 压缩图片数据
///
/// 根据图片格式进行智能压缩：
/// - JPEG/WebP：重新编码为质量 85% 的 JPEG
/// - PNG：尝试优化压缩
/// - 其他格式：不做处理，原样返回
///
/// 返回压缩后的字节数据和新的文件名（如果格式变化）。
pub fn compress_image(data: &[u8], original_name: &str) -> AppResult<(Vec<u8>, String)> {
    use image::{ImageEncoder, ImageFormat};

    // 检测图片格式（验证是否为有效图片）
    let _format = image::guess_format(data)
        .map_err(|e| AppError::Internal(format!("无法识别图片格式: {}", e)))?;

    let img = image::load_from_memory(data)
        .map_err(|e| AppError::Internal(format!("加载图片失败: {}", e)))?;

    let ext = original_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    let ext_lower = ext.as_str();

    match ext_lower {
        "jpg" | "jpeg" => {
            // JPEG: 重新编码为质量 85%
            let mut buf = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
            encoder
                .write_image(
                    img.as_bytes(),
                    img.width(),
                    img.height(),
                    img.color().into(),
                )
                .map_err(|e| AppError::Internal(format!("JPEG 压缩失败: {}", e)))?;
            Ok((buf, original_name.to_string()))
        }
        "png" => {
            // PNG: 使用优化的编码器
            let mut buf = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
                .map_err(|e| AppError::Internal(format!("PNG 压缩失败: {}", e)))?;
            Ok((buf, original_name.to_string()))
        }
        "webp" => {
            // WebP 转为质量 85% 的 JPEG 以减小大小
            let mut buf = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
            encoder
                .write_image(
                    img.as_bytes(),
                    img.width(),
                    img.height(),
                    img.color().into(),
                )
                .map_err(|e| AppError::Internal(format!("WebP 转 JPEG 失败: {}", e)))?;
            // 更新文件后缀名为 jpg
            let new_name = if let Some(pos) = original_name.rfind('.') {
                format!("{}jpg", &original_name[..=pos])
            } else {
                format!("{}.jpg", original_name)
            };
            Ok((buf, new_name))
        }
        _ => {
            // GIF/BMP/其他：转为 JPEG 格式以压缩
            let mut buf = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
            encoder
                .write_image(
                    img.as_bytes(),
                    img.width(),
                    img.height(),
                    img.color().into(),
                )
                .map_err(|e| AppError::Internal(format!("{} 转 JPEG 失败: {}", ext_lower, e)))?;
            // 更新文件后缀名为 jpg
            let new_name = if let Some(pos) = original_name.rfind('.') {
                format!("{}jpg", &original_name[..=pos])
            } else {
                format!("{}.jpg", original_name)
            };
            Ok((buf, new_name))
        }
    }
}
