use std::{
    io::{Cursor, Read, Write},
    sync::Arc,
};

use ryframe_application::{TenantConfigArchiveContents, TenantConfigArchivePort};
use ryframe_kernel::{AppError, AppResult};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

struct ZipTenantConfigArchive;

pub fn codec() -> Arc<dyn TenantConfigArchivePort> {
    Arc::new(ZipTenantConfigArchive)
}

impl TenantConfigArchivePort for ZipTenantConfigArchive {
    fn build(
        &self,
        manifest_name: &str,
        manifest: &[u8],
        resources_name: &str,
        resources: &[u8],
        max_package_bytes: usize,
    ) -> AppResult<Vec<u8>> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o600);
        archive
            .start_file(manifest_name, options)
            .map_err(internal_zip_error)?;
        archive.write_all(manifest).map_err(internal_io_error)?;
        archive
            .start_file(resources_name, options)
            .map_err(internal_zip_error)?;
        archive.write_all(resources).map_err(internal_io_error)?;
        let data = archive.finish().map_err(internal_zip_error)?.into_inner();
        if data.len() > max_package_bytes {
            return Err(AppError::PayloadTooLarge(format!(
                "生成的配置包超过限制（最大 {max_package_bytes} 字节）"
            )));
        }
        Ok(data)
    }

    fn parse(
        &self,
        data: &[u8],
        manifest_name: &str,
        resources_name: &str,
        max_uncompressed_bytes: usize,
        max_compression_ratio: u64,
    ) -> AppResult<TenantConfigArchiveContents> {
        let mut archive = ZipArchive::new(Cursor::new(data)).map_err(validation_zip_error)?;
        if archive.len() != 2 {
            return Err(AppError::Validation(format!(
                "配置包只能包含 {manifest_name} 和 {resources_name}"
            )));
        }

        let mut manifest_index = None;
        let mut resources_index = None;
        let mut total_size = 0u64;
        for index in 0..archive.len() {
            let file = archive.by_index(index).map_err(validation_zip_error)?;
            validate_entry(&file, max_compression_ratio)?;
            total_size = total_size
                .checked_add(file.size())
                .ok_or_else(|| AppError::Validation("配置包解压大小溢出".into()))?;
            if total_size > max_uncompressed_bytes as u64 {
                return Err(AppError::PayloadTooLarge(format!(
                    "配置包解压后超过限制（最大 {max_uncompressed_bytes} 字节）"
                )));
            }
            match file.name_raw() {
                name if name == manifest_name.as_bytes() => {
                    if manifest_index.replace(index).is_some() {
                        return Err(AppError::Validation("配置包清单文件重复".into()));
                    }
                }
                name if name == resources_name.as_bytes() => {
                    if resources_index.replace(index).is_some() {
                        return Err(AppError::Validation("配置包资源文件重复".into()));
                    }
                }
                _ => {
                    return Err(AppError::Validation(
                        "配置包包含未允许的文件或非法文件名".into(),
                    ));
                }
            }
        }

        let manifest_index =
            manifest_index.ok_or_else(|| AppError::Validation("配置包缺少清单文件".into()))?;
        let resources_index =
            resources_index.ok_or_else(|| AppError::Validation("配置包缺少资源文件".into()))?;
        let manifest = read_bounded_entry(&mut archive, manifest_index, max_uncompressed_bytes)?;
        let resources = read_bounded_entry(
            &mut archive,
            resources_index,
            max_uncompressed_bytes.saturating_sub(manifest.len()),
        )?;
        Ok(TenantConfigArchiveContents {
            manifest,
            resources,
        })
    }
}

fn validate_entry<R: Read>(
    file: &zip::read::ZipFile<'_, R>,
    max_compression_ratio: u64,
) -> AppResult<()> {
    if file.encrypted() {
        return Err(AppError::Validation("配置包不能使用 ZIP 加密".into()));
    }
    if file.is_dir() || file.is_symlink() {
        return Err(AppError::Validation("配置包不能包含目录或符号链接".into()));
    }
    if !matches!(
        file.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err(AppError::Validation("配置包使用了不支持的压缩算法".into()));
    }
    let raw_name = file.name_raw();
    if raw_name.contains(&b'\\') || raw_name.contains(&b'/') {
        return Err(AppError::Validation("配置包文件必须位于 ZIP 根目录".into()));
    }
    let enclosed = file
        .enclosed_name()
        .ok_or_else(|| AppError::Validation("配置包包含非法路径".into()))?;
    if enclosed.to_str() != Some(file.name()) {
        return Err(AppError::Validation("配置包包含非法路径".into()));
    }
    let compressed = file.compressed_size().max(1);
    if file.size() > compressed.saturating_mul(max_compression_ratio) {
        return Err(AppError::Validation("配置包压缩比超过安全限制".into()));
    }
    Ok(())
}

fn read_bounded_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    index: usize,
    limit: usize,
) -> AppResult<Vec<u8>> {
    let mut file = archive.by_index(index).map_err(validation_zip_error)?;
    let read_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut data = Vec::with_capacity(file.size().min(limit as u64) as usize);
    file.by_ref()
        .take(read_limit)
        .read_to_end(&mut data)
        .map_err(|_| AppError::Validation("配置包文件无法安全解压".into()))?;
    if data.len() > limit {
        return Err(AppError::PayloadTooLarge("配置包解压后超过限制".into()));
    }
    Ok(data)
}

fn validation_zip_error(_: zip::result::ZipError) -> AppError {
    AppError::Validation("配置包不是有效的受控 ZIP 文件".into())
}

fn internal_zip_error(error: zip::result::ZipError) -> AppError {
    AppError::Internal(format!("配置包 ZIP 生成失败: {error}"))
}

fn internal_io_error(error: std::io::Error) -> AppError {
    AppError::Internal(format!("配置包写入失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controlled_archive_round_trip() {
        let codec = ZipTenantConfigArchive;
        let data = codec
            .build(
                "manifest.json",
                br#"{"schema":"v1"}"#,
                "resources.json",
                br#"{"items":[]}"#,
                4096,
            )
            .expect("受控归档应可生成");

        let contents = codec
            .parse(&data, "manifest.json", "resources.json", 4096, 100)
            .expect("受控归档应可解析");

        assert_eq!(contents.manifest, br#"{"schema":"v1"}"#);
        assert_eq!(contents.resources, br#"{"items":[]}"#);
    }

    #[test]
    fn archive_with_unexpected_entry_is_rejected() {
        let cursor = Cursor::new(Vec::new());
        let mut archive = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file("manifest.json", options).unwrap();
        archive.write_all(b"{}").unwrap();
        archive.start_file("unexpected.json", options).unwrap();
        archive.write_all(b"{}").unwrap();
        let data = archive.finish().unwrap().into_inner();

        let error = ZipTenantConfigArchive
            .parse(&data, "manifest.json", "resources.json", 4096, 100)
            .expect_err("额外文件必须被拒绝");
        assert!(matches!(error, AppError::Validation(_)));
    }
}
