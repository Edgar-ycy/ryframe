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

pub(crate) fn operations(executable: &str, request: &Value) -> Result<Value> {
    if executable.is_empty() || executable.chars().any(char::is_control) {
        return Err("迁移程序路径必须是非空且无控制字符的文本".into());
    }
    let configuration = config_directory(request)?;
    Ok(json!({"operations": sequence(executable, configuration)}))
}

pub(crate) fn config_directory(request: &Value) -> Result<&str> {
    let directory = json_io::text(&request["config_dir"], "显式维护配置目录")?;
    if !Path::new(directory).is_absolute() || directory.chars().any(char::is_control) {
        return Err("维护命令必须绑定绝对配置目录；请核对固定请求config_dir".into());
    }
    Ok(directory)
}
pub(crate) fn configured_command(
    executable: &str,
    request: &Value,
    args: Vec<String>,
) -> Result<Vec<String>> {
    let mut command = command_prefix(executable, config_directory(request)?);
    command.extend(args);
    Ok(command)
}

fn command_prefix(executable: &str, configuration: &str) -> Vec<String> {
    vec![
        executable.to_owned(),
        "--config-dir".into(),
        configuration.into(),
    ]
}
fn sequence(executable: &str, configuration: &str) -> Vec<Operation> {
    let mut scopes = vec![("control", None)];
    let mut keys = super::input::request::KEYS.map(|(key, _, _)| key);
    keys.sort();
    scopes.extend(keys.into_iter().map(|key| ("tenant-data", Some(key))));
    scopes
        .into_iter()
        .flat_map(|(scope, target)| {
            ["up", "verify"].into_iter().map(move |action| {
                let mut command = command_prefix(executable, configuration);
                command.extend([scope.into(), action.into()]);
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

fn parse_operations(value: &Value, request: &Value) -> Result<Vec<Operation>> {
    let operations: Vec<Operation> =
        serde_json::from_value(value.clone()).map_err(|_| "迁移操作必须符合固定序列结构")?;
    let executable = operations
        .first()
        .and_then(|item| item.command.first())
        .ok_or("迁移固定序列不能为空")?;
    let configuration = config_directory(request)?;
    if executable.is_empty() || operations != sequence(executable, configuration) {
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
    let request = json_io::read(&target.join("request.json"))?;
    let operations = parse_operations(value, &request)?;
    let receipts = scan(backend, target, &operations)?;
    let mut completed = Vec::new();
    for operation in &operations {
        let Some(receipt) = receipts.success.get(&operation.id) else {
            break;
        };
        completed.push(json!({"id":operation.id,"receipt":receipt}));
    }
    let next = completed.len();
    if receipts.success.len() != next {
        return Err("迁移收据越过首个缺失操作，不能证明连续前缀".into());
    }
    if receipts.failed.as_ref().is_some_and(|(index, _)| {
        if complete {
            *index >= next
        } else {
            *index != next
        }
    }) {
        return Err("失败只读收据不属于原始首个缺失verify；保留现场，禁止续作".into());
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
    let request = json_io::read(&target.join("request.json"))?;
    if !sequence(executable, config_directory(&request)?).contains(&operation) {
        return Err("迁移操作不同于固定完整序列".into());
    }
    scan(
        backend,
        target,
        &sequence(executable, config_directory(&request)?),
    )?
    .success
    .remove(&operation.id)
    .ok_or_else(|| "续作操作没有产生唯一成功命令收据".into())
}

struct Receipts {
    success: BTreeMap<String, Value>,
    failed: Option<(usize, String)>,
}

fn scan(backend: &Path, target: &Path, operations: &[Operation]) -> Result<Receipts> {
    let patterns = operations.iter().map(pattern).collect::<Result<Vec<_>>>()?;
    let mut receipts = Receipts {
        success: BTreeMap::new(),
        failed: None,
    };
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
        let index = matched[0];
        let operation = &operations[index];
        validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
        let value = json_io::read(&path)?;
        if value["error_type"] == "FileNotFoundError" {
            failed_read(backend, target, &path, &value, operation)?;
            if receipts.failed.replace((index, name.to_owned())).is_some() {
                return Err("迁移前缀包含重复失败只读收据".into());
            }
        } else {
            let receipt = validate_receipt(backend, &path, operation)?;
            if receipts
                .success
                .insert(operation.id.clone(), receipt)
                .is_some()
            {
                return Err("迁移前缀包含重复命令收据".into());
            }
        }
    }
    Ok(receipts)
}

fn failed_read(
    backend: &Path,
    target: &Path,
    path: &Path,
    value: &Value,
    operation: &Operation,
) -> Result<()> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    json_io::exact(
        value,
        &["command", "returncode", "error_type", "stdout", "stderr"],
        "迁移只读中断收据",
    )?;
    validate_local_test_path(
        &target.join("failure.json"),
        backend,
        LocalTestPathKind::ExistingFile,
    )?;
    if operation.write
        || value["command"] != json!(operation.command)
        || !value["returncode"].is_null()
        || value["stdout"] != ""
        || value["stderr"] != ""
        || !super::resume_model::failure(&json_io::read(&target.join("failure.json"))?)?
    {
        return Err("迁移失败收据不是已登记的FileNotFound只读verify中断；禁止重放".into());
    }
    Ok(())
}

pub(super) fn failed_readonly(backend: &Path, target: &Path) -> Result<Option<(usize, String)>> {
    let prepared = json_io::read(&target.join("prepare.json"))?;
    let executable =
        super::evidence_model::executable(&prepared["generation"]["maintenance"], "migrate")?;
    let request = json_io::read(&target.join("request.json"))?;
    Ok(scan(
        backend,
        target,
        &sequence(executable, config_directory(&request)?),
    )?
    .failed)
}

/// 仅核验已在intent完整前像中绑定的原始失败只读收据，不枚举本次未完成写入。
pub(super) fn original_failed_read(backend: &Path, target: &Path, path: &Path) -> Result<()> {
    if path.parent() != Some(target) {
        return Err("原始失败收据不属于固定目标目录".into());
    }
    let prepared = json_io::read(&target.join("prepare.json"))?;
    let request = json_io::read(&target.join("request.json"))?;
    let executable =
        super::evidence_model::executable(&prepared["generation"]["maintenance"], "migrate")?;
    let operations = sequence(executable, config_directory(&request)?);
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("原始收据名称无效")?;
    let mut matched = operations
        .iter()
        .filter_map(|operation| match pattern(operation) {
            Ok(pattern) if pattern.is_match(name) => Some(Ok(operation)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        });
    let operation = matched.next().ok_or("原始失败收据阶段未知")??;
    if matched.next().is_some() {
        return Err("原始失败收据阶段不唯一".into());
    }
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    let value = json_io::read(path)?;
    if value["error_type"] != "FileNotFoundError" {
        return Err("历史同阶段文件不是已核验原始失败只读收据".into());
    }
    failed_read(backend, target, path, &value, operation)
}

/// 原失败只读收据必须既属于完整intent前像，也仍保持原字节；其他历史同阶段文件不排除。
pub(super) fn prior_failed_stage(
    backend: &Path,
    target: &Path,
    files: &Value,
    stage: &str,
) -> Result<std::collections::BTreeSet<String>> {
    let old = super::resume_observations::file_map(files)?;
    let current =
        super::resume_observations::file_map(&super::tree::snapshot(backend, target, true)?)?;
    if old
        .iter()
        .any(|(name, value)| current.get(name) != Some(value))
    {
        return Err("intent绑定的完整历史前像发生变化".into());
    }
    let names = old.keys().cloned().collect();
    let names = super::resume_observations::stage(&names, stage)?;
    if names.len() > 1 {
        return Err("intent前像包含重复同阶段命令收据".into());
    }
    for name in &names {
        original_failed_read(backend, target, &target.join(name))?;
    }
    Ok(names.into_iter().collect())
}
