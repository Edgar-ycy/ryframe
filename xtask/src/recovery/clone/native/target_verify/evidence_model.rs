//! 目标证据共享的只读路径、摘要与时间规则。
use super::super::{digest, json as json_io, lifecycle::manifest};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    utc_timestamp::rfc3339_order_key,
};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub(super) const BUCKETS: [&str; 5] =
    ["uploads", "avatar", "exports", "imports", "config-packages"];

pub(super) fn check(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub(super) fn read(backend: &Path, path: &Path) -> Result<Value> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    json_io::read(path)
}

pub(super) fn binding(backend: &Path, path: &Path) -> Result<Value> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    let actual = digest::file(path)?;
    Ok(json!({"path":path,"bytes":actual.bytes,"sha256":actual.sha256}))
}

pub(super) fn bound(backend: &Path, descriptor: &Value) -> Result<PathBuf> {
    let path = manifest::bound_file(backend, descriptor, "目标历史证据", true)?;
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
    Ok(path)
}

pub(super) fn names(target: &Path, pattern: &str) -> Result<BTreeSet<String>> {
    let pattern = Regex::new(pattern)?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(target)? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| "目标证据文件名无效")?;
        if pattern.is_match(&name) {
            names.insert(name);
        }
    }
    Ok(names)
}

pub(super) fn timestamp(value: &Value) -> Result<(i64, u32)> {
    let value = json_io::text(value, "目标证据时间")?;
    check(
        !value.is_empty() && value.trim() == value,
        "目标证据时间无效",
    )?;
    rfc3339_order_key(value).map_err(|_| "目标证据时间无效".into())
}

pub(super) fn ordered(values: &[&Value]) -> bool {
    values
        .iter()
        .map(|value| timestamp(value))
        .collect::<Result<Vec<_>>>()
        .is_ok_and(|values| values.windows(2).all(|pair| pair[0] <= pair[1]))
}

pub(super) fn executable<'a>(maintenance: &'a Value, role: &str) -> Result<&'a str> {
    let path = json_io::text(
        &maintenance["artifacts"][role]["executable"],
        "目标维护工具路径",
    )?;
    crate::local_test_path::validate_external_existing_file(Path::new(path))?;
    let actual = digest::file(Path::new(path))?;
    let artifact = &maintenance["artifacts"][role];
    check(
        artifact["bytes"].as_u64() == Some(actual.bytes)
            && artifact["sha256"].as_str() == Some(actual.sha256.as_str()),
        "维护 CLI 在执行前发生变化",
    )?;
    Ok(path)
}

pub(super) fn stage_record(
    backend: &Path,
    target: &Path,
    stage: &str,
    kind: &str,
    expected: &Value,
) -> Result<Value> {
    let path = target.join(format!("{stage}.{kind}.json"));
    let value = read(backend, &path)?;
    let field = if kind == "intent" {
        "resource"
    } else {
        "observed"
    };
    json_io::exact(&value, &["at", "stage", field], "fresh 创建阶段")?;
    timestamp(&value["at"])?;
    check(
        value["stage"] == stage && value[field] == *expected,
        "fresh 创建阶段 intent 或 confirmed 不属于固定目标",
    )?;
    binding(backend, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_binding_checks_both_bytes_and_sha_before_use() {
        let parent = crate::workspace::root_dir().join(".local-tests/target-evidence-model");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let path = directory.path().join("fixture-reset.exe");
        fs::write(&path, b"fixture-maintenance").unwrap();
        let actual = digest::file(&path).unwrap();
        let maintenance = json!({"artifacts":{"reset":{"executable":path,"bytes":actual.bytes,"sha256":actual.sha256}}});
        assert_eq!(
            executable(&maintenance, "reset").unwrap(),
            path.to_str().unwrap()
        );
        let mut changed = maintenance.clone();
        changed["artifacts"]["reset"]["bytes"] = json!(actual.bytes + 1);
        assert!(executable(&changed, "reset").is_err());
        fs::write(&path, b"changed-maintenance").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), actual.bytes);
        assert!(executable(&maintenance, "reset").is_err());
    }
}
