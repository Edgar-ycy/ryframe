use std::{
    collections::BTreeSet,
    path::Path,
    process::{Command, Stdio},
};

use super::model::{
    CONTROL_GENERATED_PREFIX, CONTROL_PREFIX, TENANT_GENERATED_PREFIX, TENANT_PREFIX,
};

pub(super) fn validate_git_ref(value: &str) -> Result<&str, String> {
    let mut bytes = value.bytes();
    let safe = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
        && !value.contains("..")
        && !value.contains("@{");
    if safe {
        Ok(value)
    } else {
        Err(format!("受信 Git ref 不安全：{value:?}"))
    }
}

pub(super) fn head_paths(root: &Path, trusted_ref: &str) -> Result<BTreeSet<String>, String> {
    let trusted_ref = validate_git_ref(trusted_ref)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            trusted_ref,
            "--",
            CONTROL_PREFIX,
            TENANT_PREFIX,
            CONTROL_GENERATED_PREFIX,
            TENANT_GENERATED_PREFIX,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法读取受信 Git ref：{error}"))?;
    if !output.status.success() {
        if root.join(".git").exists() {
            return Err(format!(
                "无法从受信 Git ref `{trusted_ref}` 读取迁移，拒绝推断冻结边界"
            ));
        }
        return Ok(BTreeSet::new());
    }
    decode_null_paths(&output.stdout)
}

pub(super) fn head_file(
    root: &Path,
    relative: &str,
    trusted_ref: &str,
) -> Result<Option<Vec<u8>>, String> {
    if !root.join(".git").exists() {
        return Ok(None);
    }
    let trusted_ref = validate_git_ref(trusted_ref)?;
    let listed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            trusted_ref,
            "--",
            relative,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法读取受信 Git ref：{error}"))?;
    if !listed.status.success() {
        return Err(format!(
            "无法从受信 Git ref `{trusted_ref}` 读取迁移冻结基准，拒绝信任工作树 lock"
        ));
    }
    if !decode_null_paths(&listed.stdout)?.contains(relative) {
        return Ok(None);
    }
    let blob = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("{trusted_ref}:{relative}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法读取受信 Git ref：{error}"))?;
    if !blob.status.success() {
        return Err(format!(
            "无法读取受信 Git ref `{trusted_ref}` 中的文件：{relative}"
        ));
    }
    Ok(Some(blob.stdout))
}

fn decode_null_paths(content: &[u8]) -> Result<BTreeSet<String>, String> {
    content
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| {
            std::str::from_utf8(value)
                .map(|value| value.replace('\\', "/"))
                .map_err(|error| format!("受信 Git 路径不是 UTF-8：{error}"))
        })
        .collect()
}
