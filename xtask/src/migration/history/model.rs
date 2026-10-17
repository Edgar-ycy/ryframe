use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

pub(crate) const CONTROL_BASELINE: &str = "m20260820_000000_control_baseline";
pub(crate) const TENANT_BASELINE: &str = "m20260820_000000_tenant_baseline";
pub(crate) const CONTROL_PREFIX: &str = "crates/ryframe-db/src/migration/";
pub(crate) const TENANT_PREFIX: &str = "crates/ryframe-tenant-db/src/migration/";
pub(crate) const CONTROL_GENERATED_PREFIX: &str = "crates/ryframe-db/src/generated/";
pub(crate) const TENANT_GENERATED_PREFIX: &str = "crates/ryframe-tenant-db/src/generated/";
pub(crate) const LOCK_RELATIVE_PATH: &str = "catalog/migrations.lock.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Migration {
    pub(crate) name: String,
    pub(crate) source: PathBuf,
    pub(crate) module_registry: PathBuf,
    pub(crate) migrator_registry: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockedFile {
    pub(crate) relative: String,
    pub(crate) path: PathBuf,
    pub(crate) expected_sha256: String,
    pub(crate) storage: String,
    pub(crate) target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockDocument {
    pub(super) frozen_at: Option<String>,
}

pub(super) fn migration_name(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('m') else {
        return false;
    };
    if rest.len() < 17 || rest.as_bytes().get(15) != Some(&b'_') {
        return false;
    }
    let (stamp, label) = rest.split_at(15);
    let label = &label[1..];
    valid_timestamp(stamp)
        && label.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_timestamp(value: &str) -> bool {
    value.len() == 15
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 8 && byte == b'_' || index != 8 && byte.is_ascii_digit())
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn sha256(path: &Path) -> Result<String, String> {
    let content = std::fs::read(path)
        .map_err(|error| format!("无法读取冻结迁移源码 {}：{error}", path.display()))?;
    Ok(sha256_bytes(&content))
}

pub(crate) fn sha256_bytes(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut value = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut value, "{byte:02x}").expect("写入内存摘要不会失败");
    }
    value
}

pub(crate) fn safe_locked_path(root: &Path, value: &str) -> Result<(String, PathBuf), String> {
    let relative = normalize_relative(value)?;
    if ![
        CONTROL_PREFIX,
        TENANT_PREFIX,
        CONTROL_GENERATED_PREFIX,
        TENANT_GENERATED_PREFIX,
    ]
    .iter()
    .any(|prefix| relative.starts_with(prefix))
    {
        return Err(format!("冻结文件不属于迁移目录：{value}"));
    }
    if !relative.ends_with(".rs") {
        return Err(format!("冻结文件必须是 Rust 迁移源码：{value}"));
    }
    lock_identity(&relative)?;
    let path = relative
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component));
    if !path.starts_with(root) {
        return Err(format!("冻结文件路径不安全：{value:?}"));
    }
    Ok((relative, path))
}

fn normalize_relative(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
        || value.as_bytes().get(1) == Some(&b':')
    {
        return Err(format!("冻结文件路径不安全：{value:?}"));
    }
    let mut components = Vec::new();
    for component in value.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(format!("冻结文件路径不安全：{value:?}")),
            _ if component.contains(':') => {
                return Err(format!("冻结文件路径不安全：{value:?}"));
            }
            _ => components.push(component),
        }
    }
    if components.is_empty() {
        return Err(format!("冻结文件路径不安全：{value:?}"));
    }
    Ok(components.join("/"))
}

pub(crate) fn lock_identity(relative: &str) -> Result<(&'static str, String), String> {
    let (storage, remainder, generated) =
        if let Some(remainder) = relative.strip_prefix(CONTROL_PREFIX) {
            ("control", remainder, false)
        } else if let Some(remainder) = relative.strip_prefix(TENANT_PREFIX) {
            ("tenant-data", remainder, false)
        } else if let Some(remainder) = relative.strip_prefix(CONTROL_GENERATED_PREFIX) {
            ("control", remainder, true)
        } else if let Some(remainder) = relative.strip_prefix(TENANT_GENERATED_PREFIX) {
            ("tenant-data", remainder, true)
        } else {
            return Err(format!("冻结文件不属于迁移目录：{relative}"));
        };
    let parts = remainder.split('/').collect::<Vec<_>>();
    if generated {
        if parts.len() != 2 || parts[1] != "migration.rs" || parts[0].is_empty() {
            return Err(format!(
                "生成迁移路径必须是 <resource>/migration.rs：{relative}"
            ));
        }
        return Ok((storage, format!("resource:{}:initial", parts[0])));
    }
    if parts.is_empty() || parts[0].is_empty() {
        return Err(format!("冻结文件没有迁移目标：{relative}"));
    }
    let target = if parts.len() > 1 {
        parts[0].to_owned()
    } else {
        parts[0]
            .strip_suffix(".rs")
            .ok_or_else(|| format!("冻结文件没有迁移目标：{relative}"))?
            .to_owned()
    };
    if !migration_name(&target) {
        return Err(format!("冻结文件无法推导合法迁移目标：{relative}"));
    }
    Ok((storage, target))
}

pub(super) fn relative_posix(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("迁移源码越过工作区边界：{}", path.display()))?;
    let mut components = Vec::new();
    for component in relative.components() {
        let value = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| format!("迁移源码路径不是 UTF-8：{}", path.display()))?;
        components.push(value);
    }
    Ok(components.join("/"))
}
