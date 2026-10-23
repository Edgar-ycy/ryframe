//! 固定迁移序列与唯一连续成功前缀；不执行迁移，不推断未知写入。

use std::{collections::BTreeMap, fs, path::Path};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::{digest, json as json_io};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Operation {
    id: String,
    stage: String,
    command: Vec<String>,
    write: bool,
    stdout: String,
}

pub(crate) fn operations(executable: &str) -> Result<Value> {
    if executable.is_empty() || executable.chars().any(char::is_control) {
        return Err("迁移程序路径必须是非空且无控制字符的文本".into());
    }
    Ok(json!({"operations": sequence(executable)}))
}

fn sequence(executable: &str) -> Vec<Operation> {
    let mut scopes = vec![("control", None)];
    let mut keys = super::input::request::KEYS.map(|(key, _, _)| key);
    keys.sort();
    scopes.extend(keys.into_iter().map(|key| ("tenant-data", Some(key))));
    scopes
        .into_iter()
        .flat_map(|(scope, target)| {
            ["up", "verify"].into_iter().map(move |action| {
                let mut command = vec![executable.to_owned(), scope.into(), action.into()];
                let (id, stage, stdout) = match target {
                    Some(target) => {
                        command.extend(["--target".into(), target.into()]);
                        (
                            format!("{scope}-{target}-{action}"),
                            format!("migrate-{scope}---target-{target}-{action}"),
                            format!("scope={scope} target={target} "),
                        )
                    }
                    None => (
                        format!("{scope}-{action}"),
                        format!("migrate-{scope}-{action}"),
                        format!("scope={scope} "),
                    ),
                };
                let write = action == "up";
                let suffix = if write {
                    "migration=completed schema=verified\n"
                } else {
                    "migration_ledger=current schema=verified\n"
                };
                Operation {
                    id,
                    stage,
                    command,
                    write,
                    stdout: stdout + suffix,
                }
            })
        })
        .collect()
}

fn parse_operations(value: &Value) -> Result<Vec<Operation>> {
    let operations: Vec<Operation> =
        serde_json::from_value(value.clone()).map_err(|_| "迁移操作必须符合固定序列结构")?;
    let executable = operations
        .first()
        .and_then(|item| item.command.first())
        .ok_or("迁移固定序列不能为空")?;
    if executable.is_empty() || operations != sequence(executable) {
        return Err("迁移操作不同于固定完整序列".into());
    }
    Ok(operations)
}

fn candidates(target: &Path, prefix: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(target)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("迁移收据名称必须为 UTF-8")?;
        if name.starts_with(prefix) && name.ends_with(".command.json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn pattern(operation: &Operation) -> Result<Regex> {
    Ok(Regex::new(&format!(
        r"^{}-([a-f0-9]{{32}})\.command\.json$",
        regex::escape(&operation.stage)
    ))?)
}

fn validate_receipt(backend: &Path, path: &Path, operation: &Operation) -> Result<Value> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    let value = json_io::read(path)?;
    json_io::exact(
        &value,
        &["command", "returncode", "error_type", "stdout", "stderr"],
        "迁移成功命令收据",
    )?;
    if value["command"] != json!(operation.command)
        || value["returncode"].as_i64() != Some(0)
        || !value.get("error_type").is_some_and(Value::is_null)
        || value["stderr"] != ""
        || value["stdout"]
            .as_str()
            .map(|text| text.replace("\r\n", "\n"))
            .as_deref()
            != Some(operation.stdout.as_str())
    {
        return Err("迁移前缀命令、退出状态或成功输出无效".into());
    }
    let actual = digest::file(path)?;
    Ok(json!({"path":path,"bytes":actual.bytes,"sha256":actual.sha256}))
}

pub(crate) fn prefix(
    backend: &Path,
    target: &Path,
    value: &Value,
    complete: bool,
) -> Result<Value> {
    let operations = parse_operations(value)?;
    let patterns = operations.iter().map(pattern).collect::<Result<Vec<_>>>()?;
    let mut receipts = BTreeMap::new();
    for path in candidates(target, "migrate-")? {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("迁移收据名称无效")?;
        let matched = patterns
            .iter()
            .enumerate()
            .filter(|(_, pattern)| pattern.is_match(name))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matched.len() != 1 {
            return Err("迁移前缀包含未知命令收据".into());
        }
        let operation = &operations[matched[0]];
        let receipt = validate_receipt(backend, &path, operation)?;
        if receipts.insert(operation.id.clone(), receipt).is_some() {
            return Err("迁移前缀包含重复命令收据".into());
        }
    }
    let mut completed = Vec::new();
    for operation in &operations {
        let Some(receipt) = receipts.get(&operation.id) else {
            break;
        };
        completed.push(json!({"id":operation.id,"receipt":receipt}));
    }
    let next = completed.len();
    if receipts.len() != next {
        return Err("迁移收据越过首个缺失操作，不能证明连续前缀".into());
    }
    if complete {
        if next != operations.len() {
            return Err("迁移续作没有完成固定序列".into());
        }
    } else if next == 0 || next == operations.len() || operations[next].write {
        return Err("迁移前缀的首个缺失操作必须是只读 verify".into());
    }
    Ok(json!({"completed":completed,"next_index":next}))
}

pub(crate) fn operation_record(backend: &Path, target: &Path, value: &Value) -> Result<Value> {
    let operation: Operation =
        serde_json::from_value(value.clone()).map_err(|_| "迁移操作结构无效")?;
    let executable = operation.command.first().ok_or("迁移操作命令为空")?;
    if !sequence(executable).contains(&operation) {
        return Err("迁移操作不同于固定完整序列".into());
    }
    let paths = candidates(target, &format!("{}-", operation.stage))?;
    let pattern = pattern(&operation)?;
    if paths.len() != 1
        || !pattern.is_match(
            paths[0]
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
        )
    {
        return Err("续作操作没有产生唯一成功命令收据".into());
    }
    validate_receipt(backend, &paths[0], &operation)
}
