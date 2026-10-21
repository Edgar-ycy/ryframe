use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::Path,
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Result, devex::DevexSuite};

const MAX_OPENAPI_BYTES: u64 = 16 * 1024 * 1024;
const CONFIG: &str = "运行时测量配置无效";
const CONCURRENCY: &str = "运行时并发数必须是 10 到 100 的无符号十进制整数";
const IDENTITY: &str = "运行时身份池或租户分布校验失败";
const AUTHORITY: &str = "无法读取运行时测量的实际限流配置";
const OPENAPI: &str = "运行时 OpenAPI 必须是可读取的普通文件";
const OPENAPI_LIMIT: &str = "运行时 OpenAPI 超过 16 MiB";
const OPENAPI_JSON: &str = "运行时 OpenAPI 不是有效 JSON";
const CATALOG: &str = "运行时 OpenAPI 操作目录无效";
const PACING: &str = "运行时固定节奏不满足实际限流";
const AUTHORITY_DIGEST: &str = "实际限流配置与固定节奏摘要不一致";

/// 正式测量前只读核验配置、身份分布、实际限流和当前 OpenAPI，不生成收据。
pub(crate) fn validate(
    root: &Path,
    config: &Value,
    suite: DevexSuite,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    let suite = suite
        .as_str()
        .strip_prefix("runtime-")
        .ok_or("只有运行时场景可以核验测量输入")?;
    super::runtime_configuration::validate(config, suite)
        .map_err(|reason| format!("{CONFIG}：{reason}"))?;
    let count = concurrency(suite, environment)?;
    super::runtime_selection::validate_suite(config, suite, count)
        .map_err(|reason| format!("{IDENTITY}：{reason}"))?;
    let authority = crate::rate_limit::authority(root, environment)
        .map_err(|reason| format!("{AUTHORITY}：{}：{}", reason.code(), reason.message()))?;
    let authority = serde_json::to_value(authority).map_err(|_| AUTHORITY)?;
    let document = read_openapi(root)?;
    let catalog = super::runtime_configuration::operation_catalog(&document)
        .map_err(|reason| format!("{CATALOG}：{reason}"))?;
    let pacing = &config["contract"]["pacing"];
    super::runtime_pacing::validate_authority(pacing, &authority, &catalog)
        .map_err(|reason| format!("{PACING}：{reason}"))?;
    let bytes = serde_json::to_vec(&authority).map_err(|_| AUTHORITY)?;
    let digest = hex::encode(Sha256::digest(bytes));
    if pacing["authority_sha256"].as_str() != Some(digest.as_str()) {
        return Err(AUTHORITY_DIGEST.into());
    }
    Ok(())
}

fn concurrency(suite: &str, environment: &BTreeMap<String, String>) -> Result<u64> {
    if suite == "homepage" {
        return Ok(0);
    }
    let value = environment
        .get("RYFRAME_DEVEX_CONCURRENCY")
        .ok_or(CONCURRENCY)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CONCURRENCY.into());
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|count| (10..=100).contains(count))
        .ok_or_else(|| CONCURRENCY.into())
}

fn read_openapi(root: &Path) -> Result<Value> {
    let path = root.join("openapi/openapi.json");
    let metadata = fs::symlink_metadata(&path).map_err(|_| OPENAPI)?;
    ordinary(&metadata)?;
    let file = File::open(&path).map_err(|_| OPENAPI)?;
    ordinary(&file.metadata().map_err(|_| OPENAPI)?)?;
    let mut bytes = Vec::new();
    file.take(MAX_OPENAPI_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OPENAPI)?;
    if bytes.len() as u64 > MAX_OPENAPI_BYTES {
        return Err(OPENAPI_LIMIT.into());
    }
    ordinary(&fs::symlink_metadata(path).map_err(|_| OPENAPI)?)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        let category = match error.classify() {
            serde_json::error::Category::Io => "读取错误",
            serde_json::error::Category::Syntax => "语法错误",
            serde_json::error::Category::Data => "数据类型错误",
            serde_json::error::Category::Eof => "输入不完整",
        };
        format!(
            "{OPENAPI_JSON}：第 {} 行第 {} 列，{category}",
            error.line(),
            error.column()
        )
        .into()
    })
}

fn ordinary(metadata: &fs::Metadata) -> Result<()> {
    if !metadata.is_file() || linked(metadata) {
        return Err(OPENAPI.into());
    }
    if metadata.len() > MAX_OPENAPI_BYTES {
        return Err(OPENAPI_LIMIT.into());
    }
    Ok(())
}

#[cfg(windows)]
fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
