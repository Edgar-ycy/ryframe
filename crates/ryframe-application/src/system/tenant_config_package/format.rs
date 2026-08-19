use std::io::{Cursor, Read, Write};

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde::de::DeserializeOwned;
use serde_json::Value;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::{
    APP_VERSION_MAX_CHARS, CapabilityRequirement, GeneratedTenantConfigPackage, MANIFEST_FILE_NAME,
    MAX_COMPRESSION_RATIO, MAX_JSON_DEPTH, ParsedTenantConfigPackage, RESOURCES_FILE_NAME,
    TENANT_CONFIG_PACKAGE_SCHEMA, TENANT_KEY_MAX_CHARS, TENANT_NAME_MAX_CHARS,
    TenantConfigPackageLimits, TenantConfigPackageManifest, TenantConfigPackageResources,
    required_permission_summary, required_route_summary, sha256_hex,
    validate_required_capabilities,
};

pub(super) fn build_package_blocking(
    mut resources: TenantConfigPackageResources,
    mut required_capabilities: Vec<CapabilityRequirement>,
    source_tenant_key: String,
    source_tenant_name: String,
    source_app_version: String,
    generated_at: DateTime<Utc>,
    limits: TenantConfigPackageLimits,
) -> AppResult<GeneratedTenantConfigPackage> {
    validate_source_metadata(&source_tenant_key, &source_tenant_name, &source_app_version)?;
    resources.canonicalize();
    resources.validate(limits)?;
    required_capabilities.sort();
    validate_required_capabilities(&required_capabilities, &resources)?;
    let canonical_resources = serde_json::to_vec(&resources)
        .map_err(|error| AppError::Internal(format!("配置资源序列化失败: {error}")))?;
    let counts = resources.counts();
    let item_count = counts.total()?;
    let manifest = TenantConfigPackageManifest {
        schema: TENANT_CONFIG_PACKAGE_SCHEMA.to_owned(),
        source_app_version,
        source_tenant_key,
        source_tenant_name,
        generated_at,
        resource_counts: counts,
        item_count,
        resources_sha256: sha256_hex(&canonical_resources),
        required_capabilities,
        required_permissions: required_permission_summary(&resources),
        required_page_routes: required_route_summary(&resources),
    };
    let manifest_data = serde_json::to_vec(&manifest)
        .map_err(|error| AppError::Internal(format!("配置包清单序列化失败: {error}")))?;
    validate_uncompressed_size(manifest_data.len(), canonical_resources.len(), limits)?;

    let cursor = Cursor::new(Vec::new());
    let mut archive = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    archive
        .start_file(MANIFEST_FILE_NAME, options)
        .map_err(internal_zip_error)?;
    archive
        .write_all(&manifest_data)
        .map_err(internal_io_error)?;
    archive
        .start_file(RESOURCES_FILE_NAME, options)
        .map_err(internal_zip_error)?;
    archive
        .write_all(&canonical_resources)
        .map_err(internal_io_error)?;
    let data = archive.finish().map_err(internal_zip_error)?.into_inner();
    if data.len() > limits.max_package_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "生成的配置包超过限制（最大 {} 字节）",
            limits.max_package_bytes
        )));
    }
    let package_sha256 = sha256_hex(&data);
    Ok(GeneratedTenantConfigPackage {
        manifest,
        resources,
        canonical_resources,
        data,
        package_sha256,
    })
}

pub(super) fn parse_package_blocking(
    data: Vec<u8>,
    limits: TenantConfigPackageLimits,
) -> AppResult<ParsedTenantConfigPackage> {
    if data.is_empty() {
        return Err(AppError::Validation("配置包不能为空".into()));
    }
    if data.len() > limits.max_package_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "配置包超过限制（最大 {} 字节）",
            limits.max_package_bytes
        )));
    }
    let package_sha256 = sha256_hex(&data);
    let mut archive = ZipArchive::new(Cursor::new(data)).map_err(validation_zip_error)?;
    if archive.len() != 2 {
        return Err(AppError::Validation(
            "配置包只能包含 manifest.json 和 resources.json".into(),
        ));
    }

    let mut manifest_index = None;
    let mut resources_index = None;
    let mut total_size = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(validation_zip_error)?;
        validate_zip_entry(&file)?;
        total_size = total_size
            .checked_add(file.size())
            .ok_or_else(|| AppError::Validation("配置包解压大小溢出".into()))?;
        if total_size > limits.max_uncompressed_bytes as u64 {
            return Err(AppError::PayloadTooLarge(format!(
                "配置包解压后超过限制（最大 {} 字节）",
                limits.max_uncompressed_bytes
            )));
        }
        match file.name_raw() {
            name if name == MANIFEST_FILE_NAME.as_bytes() => {
                if manifest_index.replace(index).is_some() {
                    return Err(AppError::Validation("配置包清单文件重复".into()));
                }
            }
            name if name == RESOURCES_FILE_NAME.as_bytes() => {
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
        manifest_index.ok_or_else(|| AppError::Validation("配置包缺少 manifest.json".into()))?;
    let resources_index =
        resources_index.ok_or_else(|| AppError::Validation("配置包缺少 resources.json".into()))?;
    let manifest_data =
        read_bounded_entry(&mut archive, manifest_index, limits.max_uncompressed_bytes)?;
    let resources_data = read_bounded_entry(
        &mut archive,
        resources_index,
        limits
            .max_uncompressed_bytes
            .saturating_sub(manifest_data.len()),
    )?;
    validate_uncompressed_size(manifest_data.len(), resources_data.len(), limits)?;

    let manifest: TenantConfigPackageManifest = parse_bounded_json(&manifest_data, "配置包清单")?;
    validate_manifest(&manifest, &resources_data, limits)?;
    let mut resources: TenantConfigPackageResources =
        parse_bounded_json(&resources_data, "配置包资源")?;
    resources.validate(limits)?;
    let actual_counts = resources.counts();
    if manifest.resource_counts != actual_counts || manifest.item_count != actual_counts.total()? {
        return Err(AppError::Validation("配置包资源计数与清单不一致".into()));
    }
    if manifest.required_permissions != required_permission_summary(&resources)
        || manifest.required_page_routes != required_route_summary(&resources)
    {
        return Err(AppError::Validation("配置包目录摘要与资源不一致".into()));
    }
    validate_required_capabilities(&manifest.required_capabilities, &resources)?;
    resources.canonicalize();
    let canonical_resources = serde_json::to_vec(&resources)
        .map_err(|error| AppError::Internal(format!("规范化配置资源失败: {error}")))?;
    Ok(ParsedTenantConfigPackage {
        manifest,
        resources,
        canonical_resources,
        package_sha256,
    })
}

fn validate_zip_entry<R: Read>(file: &zip::read::ZipFile<'_, R>) -> AppResult<()> {
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
    if file.size() > compressed.saturating_mul(MAX_COMPRESSION_RATIO) {
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

fn parse_bounded_json<T: DeserializeOwned>(data: &[u8], label: &str) -> AppResult<T> {
    let value: Value = serde_json::from_slice(data)
        .map_err(|_| AppError::Validation(format!("{label}不是有效 JSON")))?;
    validate_json_depth(&value)?;
    serde_json::from_value(value)
        .map_err(|error| AppError::Validation(format!("{label}结构无效: {error}")))
}

fn validate_json_depth(root: &Value) -> AppResult<()> {
    let mut stack = vec![(root, 1usize)];
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_JSON_DEPTH {
            return Err(AppError::Validation("配置包 JSON 嵌套层级过深".into()));
        }
        match value {
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                stack.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_manifest(
    manifest: &TenantConfigPackageManifest,
    resources_data: &[u8],
    limits: TenantConfigPackageLimits,
) -> AppResult<()> {
    if manifest.schema != TENANT_CONFIG_PACKAGE_SCHEMA {
        return Err(AppError::Validation("配置包协议版本不受支持".into()));
    }
    validate_source_metadata(
        &manifest.source_tenant_key,
        &manifest.source_tenant_name,
        &manifest.source_app_version,
    )?;
    if manifest.item_count > limits.max_items {
        return Err(AppError::Validation(format!(
            "配置包项目数量超过限制（最大 {} 项）",
            limits.max_items
        )));
    }
    let expected_sha256 = sha256_hex(resources_data);
    if manifest.resources_sha256.len() != 64
        || !manifest
            .resources_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !manifest
            .resources_sha256
            .eq_ignore_ascii_case(&expected_sha256)
    {
        return Err(AppError::Validation("配置包资源完整性校验失败".into()));
    }
    Ok(())
}

fn validate_source_metadata(
    tenant_key: &str,
    tenant_name: &str,
    app_version: &str,
) -> AppResult<()> {
    if ryframe_kernel::TenantId::parse(tenant_key).is_err()
        || tenant_key.len() > TENANT_KEY_MAX_CHARS
        || tenant_name.trim().is_empty()
        || tenant_name.trim() != tenant_name
        || tenant_name.contains('\0')
        || tenant_name.chars().count() > TENANT_NAME_MAX_CHARS
        || app_version.trim().is_empty()
        || app_version.trim() != app_version
        || app_version.len() > APP_VERSION_MAX_CHARS
        || app_version
            .bytes()
            .any(|byte| !(0x21..=0x7e).contains(&byte))
    {
        return Err(AppError::Validation("配置包来源信息不完整".into()));
    }
    Ok(())
}

fn validate_uncompressed_size(
    manifest_size: usize,
    resources_size: usize,
    limits: TenantConfigPackageLimits,
) -> AppResult<()> {
    let total = manifest_size
        .checked_add(resources_size)
        .ok_or_else(|| AppError::PayloadTooLarge("配置包解压大小溢出".into()))?;
    if total > limits.max_uncompressed_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "配置包解压后超过限制（最大 {} 字节）",
            limits.max_uncompressed_bytes
        )));
    }
    Ok(())
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
