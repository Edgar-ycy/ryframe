use super::reject;
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) const COMMON: &[&str] = &[
    "format_version",
    "kind",
    "operation_id",
    "runtime_directory",
    "source_sha256",
];

pub(super) fn fields(value: &Value, extra: &[&str], message: &'static str) -> Result<()> {
    let all = COMMON.iter().chain(extra).copied().collect::<Vec<_>>();
    if strict::exact(value, &all, "Worker 收据").is_err() {
        return reject(message);
    }
    Ok(())
}

pub(super) fn hex(value: &Value, length: usize) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == length
            && s.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

pub(super) fn positive(value: &Value) -> bool {
    value.as_u64().is_some_and(|n| n > 0)
}

pub(super) fn identity(value: &Value) -> Result<()> {
    if strict::exact(value, &["pid", "started", "executable"], "Worker 进程身份").is_err()
        || !value["pid"].as_u64().is_some_and(|pid| pid > 1)
        || !value["started"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
        || !value["executable"]
            .as_str()
            .is_some_and(|s| !s.contains('\0') && Path::new(s).is_absolute())
    {
        return reject("Worker 收据缺少有效的 PID 创建身份");
    }
    Ok(())
}

pub(super) fn common(value: &Value, kind: &str, directory: &Value, source: &Value) -> Result<()> {
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != kind
        || !hex(&value["operation_id"], 32)
        || !hex(source, 64)
        || value["runtime_directory"] != *directory
        || value["source_sha256"] != *source
    {
        return reject("Worker 控制收据与运行目录、操作或来源不匹配");
    }
    Ok(())
}

pub(super) fn hash(value: &Value) -> Result<String> {
    // 与现有收据的 json.dumps(sort_keys=True,separators=(',',':')) 保持一致。
    let text = serde_json::to_string(value)?;
    let mut ascii = String::new();
    for character in text.chars() {
        if character >= '\u{7f}' {
            let mut units = [0; 2];
            for unit in character.encode_utf16(&mut units) {
                use std::fmt::Write;
                write!(ascii, "\\u{unit:04x}")?;
            }
        } else {
            ascii.push(character);
        }
    }
    Ok(hex::encode(Sha256::digest(ascii.as_bytes())))
}

pub(super) fn bound(value: &Value, owner: &Value, request: &Value) -> Result<()> {
    if value["operation_id"] != owner["operation_id"]
        || value["owner_sha256"] != hash(owner)?
        || value["request_sha256"] != hash(request)?
    {
        return reject("Worker 控制收据没有绑定当前原子 owner 与请求");
    }
    Ok(())
}
