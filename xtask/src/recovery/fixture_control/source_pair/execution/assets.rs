use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use serde_json::Value;

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{json, source::FileRecord},
};

use super::super::super::{file_binding, valid_sha256};

pub(super) const MARKER: &str = "openapi/candidate.json";

pub(super) fn load(backend: &Path, frontend: &Path) -> Result<Value> {
    let (backend_bytes, _) = document(&backend.join("openapi/openapi.json"))?;
    let (frontend_bytes, contract) = document(&frontend.join("openapi/openapi.json"))?;
    let (metadata_bytes, metadata) = document(&frontend.join("openapi/source.json"))?;
    if crate::contract::canonical_contract(&backend_bytes)? != backend_bytes
        || frontend_bytes != backend_bytes
    {
        return Err(
            "前后端 OpenAPI 必须是同批规范契约；请通过正式 generator 同步，勿改写 metadata".into(),
        );
    }
    let version = contract["openapi"].as_str().ok_or("OpenAPI 版本字段缺失")?;
    let formal_sha = metadata["sha256"]
        .as_str()
        .ok_or("正式 metadata 摘要缺失")?
        .to_ascii_lowercase();
    valid_sha256(&formal_sha, "正式 metadata 摘要")?;
    if metadata["schema_version"].as_f64() != Some(1.0)
        || !metadata["openapi_version"]
            .as_str()
            .is_some_and(|value| value.starts_with("3."))
    {
        return Err("正式 metadata 版本无效；请使用正式契约同步入口".into());
    }
    let path = frontend.join(MARKER);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
            super::git::require_ignored(frontend, MARKER)?;
            let binding = file_binding(&path, "候选契约 marker")?;
            let (bytes, marker) = document(&path)?;
            require_marker(&marker, &bytes, &frontend_bytes, &metadata_bytes, version)?;
            if file_binding(&path, "候选契约 marker")? != binding {
                return Err("候选契约 marker 在核验期间变化".into());
            }
            Ok(binding)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_absolute_path(&path, LocalTestPathKind::OutputFile)?;
            if formal_sha != crate::contract::sha256_hex(&frontend_bytes)
                || metadata["openapi_version"].as_str() != Some(version)
            {
                return Err(
                    "无候选 marker 时正式 metadata 摘要或版本与契约不一致；请执行正式契约同步"
                        .into(),
                );
            }
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
                Ok(_) => Err("正式契约核验期间候选 marker 被创建".into()),
                Err(error) => {
                    Err(format!("无法复核候选 marker 缺失状态：{error}；检查目录权限").into())
                }
            }
        }
        Err(error) => Err(format!("无法核验候选 marker 存在性：{error}；检查契约目录权限").into()),
    }
}

fn require_marker(
    marker: &Value,
    bytes: &[u8],
    openapi: &[u8],
    metadata: &[u8],
    version: &str,
) -> Result<()> {
    json::exact(
        marker,
        &[
            "candidate_sha256",
            "formal_source_sha256",
            "mode",
            "openapi_version",
            "schema_version",
        ],
        "候选 marker",
    )?;
    let mut canonical = serde_json::to_vec_pretty(marker)?;
    canonical.push(b'\n');
    for field in ["candidate_sha256", "formal_source_sha256"] {
        valid_sha256(
            marker[field].as_str().ok_or("候选摘要必须是字符串")?,
            "候选摘要",
        )?;
    }
    if marker["schema_version"].as_u64() != Some(1)
        || marker["mode"] != "candidate"
        || marker["openapi_version"].as_str() != Some(version)
        || canonical != bytes
        || marker["candidate_sha256"] != crate::contract::sha256_hex(openapi)
        || marker["formal_source_sha256"] != crate::contract::sha256_hex(metadata)
    {
        return Err(
            "候选 marker 的规范格式、摘要、模式或版本不匹配；请通过正式 generator 重新同步".into(),
        );
    }
    Ok(())
}

fn document(path: &Path) -> Result<(Vec<u8>, Value)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let before = file_binding(path, "机器契约")?;
    if before["bytes"].as_u64().ok_or("契约字节数无效")? > json::MAX_JSON_BYTES {
        return Err("机器契约超过 16 MiB 文档限额".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(json::MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > json::MAX_JSON_BYTES
        || before["bytes"].as_u64() != Some(bytes.len() as u64)
        || before["sha256"] != crate::contract::sha256_hex(&bytes)
    {
        return Err("机器契约超过限额或读取字节与文件绑定不一致".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "机器契约必须是 UTF-8 JSON")?;
    let value = json::parse_fragment(text, "机器契约")?;
    if !value.is_object() || file_binding(path, "机器契约")? != before {
        return Err("机器契约不是对象或读取期间变化".into());
    }
    Ok((bytes, value))
}

pub(super) fn expected(files: &[FileRecord], asset: &Value) -> Result<Vec<FileRecord>> {
    let mut expected = files.to_vec();
    if !asset.is_null() {
        if expected.iter().any(|file| file.path == MARKER) {
            return Err("原候选 marker 已被 Git 跟踪，不符合 ignored 机器契约输入".into());
        }
        expected.push(FileRecord {
            path: MARKER.into(),
            sha256: asset["sha256"]
                .as_str()
                .ok_or("候选文件绑定摘要缺失")?
                .into(),
        });
        expected.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(expected)
}

pub(super) fn copy(frontend: &Path, output: &Path, asset: &Value) -> Result<()> {
    if asset.is_null() {
        return Ok(());
    }
    let input = frontend.join(MARKER);
    validate_absolute_path(&input, LocalTestPathKind::ExistingFile)?;
    if file_binding(&input, "原候选 marker")? != *asset {
        return Err("原候选 marker 绑定漂移；禁止发布混合来源".into());
    }
    let (bytes, _) = document(&input)?;
    if crate::contract::sha256_hex(&bytes) != asset["sha256"].as_str().ok_or("候选摘要缺失")?
        || asset["bytes"].as_u64() != Some(bytes.len() as u64)
    {
        return Err("候选 marker 读取结果与原绑定不一致，禁止复制未核验字节".into());
    }
    let destination = output.join(MARKER);
    validate_absolute_path(&destination, LocalTestPathKind::OutputFile)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    if file_binding(&input, "原候选 marker")? != *asset {
        return Err("复制候选 marker 期间原文件发生变化；保留输出，不重放".into());
    }
    Ok(())
}
