//! 两端共用完整相对来源清单；跨 OS 摘要不包含根路径或 inode。
#[cfg(target_os = "linux")]
use super::source_guard;
use super::sources;
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, source},
};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn capture(root: &Path) -> Result<Value> {
    let paths = sources::discover(root)?;
    let entries = super::scan::ordered(&paths, |relative| {
        let path = root.join(relative);
        if digest::linked_path(&path)?.contains(&true) {
            return Err("观察器来源不能经过链接".into());
        }
        Ok(json!({"path":relative,"sha256":digest::file(&path)?.sha256}))
    })?;
    Ok(json!(entries))
}

pub(crate) fn sha256(entries: &Value) -> Result<String> {
    let entries = entries.as_array().ok_or("来源必须是完整数组")?;
    let mut previous = "";
    let mut policy = false;
    for entry in entries {
        document::exact(entry, &["path", "sha256"], "完整来源项")?;
        let path = document::text(&entry["path"], "来源路径")?;
        let sha = document::text(&entry["sha256"], "来源摘要")?;
        if path <= previous
            || path.contains(['\\', ':', '\0', '\r', '\n'])
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || !hex_digest(sha, 64)
        {
            return Err("来源清单必须完整、有序、唯一且含有效相对路径和摘要".into());
        }
        previous = path;
        policy |= path == sources::POLICY;
    }
    if !policy {
        return Err("完整来源缺少 inventory 规则".into());
    }
    digest::plan_hash(&json!(entries))
}

pub(crate) fn hex_digest(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(target_os = "linux")]
pub(crate) fn verify_compiled(root: &Path, entries: &Value) -> Result<()> {
    let input = serde_json::to_vec(&json!({"format_version":1,"root":root,"sources":entries}))?;
    source_guard::execute(&mut input.as_slice())
        .map(|_| ())
        .map_err(|_| "Linux 观察器编译根或完整编译来源不匹配".into())
}

pub(crate) fn git_source(root: &Path, commit: &str) -> Result<source::Inventory> {
    if !root.is_absolute()
        || !hex_digest(commit, 40)
        || digest::linked_path(&root.join(".git"))?.contains(&true)
        || !std::fs::symlink_metadata(root.join(".git"))?.is_dir()
        || root.join(".git/objects/info/alternates").try_exists()?
        || root.join(".git/commondir").try_exists()?
    {
        return Err("Linux 观察器来源必须是无 alternates 的独立普通 Git clone".into());
    }
    let observed = source::capture(root)?;
    if !observed.source.snapshot.clean || observed.source.snapshot.head != commit {
        return Err("Linux 观察器来源 HEAD 或 clean 状态不匹配".into());
    }
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_manifest_rejects_missing_policy_duplicates_and_invalid_paths() {
        let entries = json!([{"path":sources::POLICY,"sha256":"a".repeat(64)}]);
        assert!(sha256(&entries).is_ok());
        for invalid in [
            json!([]),
            json!([entries[0], entries[0]]),
            json!([{"path":"../escape","sha256":"a".repeat(64)}]),
            json!([{"path":sources::POLICY,"sha256":"A".repeat(64)}]),
        ] {
            assert!(sha256(&invalid).is_err());
        }
    }
}
