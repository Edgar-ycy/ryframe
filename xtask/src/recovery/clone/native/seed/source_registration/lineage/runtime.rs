//! 只读汇总单个 runtime 的创建身份历史；全局停止证明由父层负责。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        digest, input::digest_shape, json as documents, resources::common,
        target_verify::input::request_configuration::canonical_path,
    },
};

#[path = "runtime/contract.rs"]
mod contract;
#[path = "runtime/history.rs"]
mod history;
#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/source_registration/lineage/runtime.rs"]
mod tests;

pub(crate) fn collect(backend: &Path, runtime: &Path) -> Result<Value> {
    let backend = canonical(backend)?;
    let runtime = directory(runtime, &backend)?;
    let (mut receipt, runtime_binding) = read_document(&backend, &runtime.join("runtime.json"))?;
    contract::validate(&backend, &mut receipt)?;
    let (events, history_binding) =
        read_document(&backend, &runtime.join("producer-history.json"))?;
    let state = history::collect(&backend, &runtime, &receipt, &events)?;
    let roles = role_receipts(&backend, &runtime, &receipt, &state)?;
    for binding in [&runtime_binding, &history_binding] {
        common::bound_file(&backend, binding, "runtime 历史绑定")?;
    }
    Ok(
        json!({"runtime_directory": runtime, "runtime": runtime_binding,
        "history": history_binding, "scope_id": receipt["scope_id"],
        "role_receipts": roles, "generations": state.generations}),
    )
}

fn role_receipts(
    backend: &Path,
    runtime: &Path,
    contract: &Value,
    state: &history::History,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for role in ["api", "worker"] {
        let path = runtime.join(format!("{role}.json"));
        let Some(latest) = state.latest.get(role) else {
            match fs::symlink_metadata(&path) {
                Ok(_) => return Err("角色收据没有对应的 started 历史".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            continue;
        };
        let (receipt, binding) = read_document(backend, &path)?;
        documents::exact(
            &receipt,
            &["format_version", "role", "scope_id", "identity"],
            "runtime 角色收据",
        )?;
        let identity = identity(&receipt["identity"], &contract["artifacts"][role])?;
        if receipt["format_version"].as_u64() != Some(1)
            || receipt["role"] != role
            || receipt["scope_id"] != contract["scope_id"]
            || &identity != latest
        {
            return Err("角色收据不是该 runtime 的最新启动代次".into());
        }
        result.push(json!({"role": role, "receipt": binding, "identity": identity}));
    }
    Ok(result)
}

fn canonical(path: &Path) -> Result<PathBuf> {
    let text = digest::path_text(path, "runtime 路径")?;
    if !path.is_absolute()
        || text
            .split(['/', '\\'])
            .any(|part| matches!(part, "." | ".."))
        || digest::linked_path(path)?.iter().any(|linked| *linked)
    {
        return Err("runtime 路径必须规范、绝对且不能经过链接".into());
    }
    canonical_path(path)
}

fn directory(path: &Path, parent: &Path) -> Result<PathBuf> {
    let path = canonical(path)?;
    if path == parent || !path.starts_with(parent) || !fs::metadata(&path)?.is_dir() {
        return Err("runtime 或配置目录必须是明确范围内的无链接目录".into());
    }
    Ok(path)
}

fn regular(path: &Path, parent: &Path, allow_empty: bool) -> Result<PathBuf> {
    let path = canonical(path)?;
    let metadata = fs::metadata(&path)?;
    if !path.starts_with(parent)
        || !metadata.is_file()
        || metadata.len() > documents::MAX_JSON_BYTES
        || (!allow_empty && metadata.len() == 0)
    {
        return Err("runtime 文档或日志必须是明确范围内的普通文件且不超过16MiB".into());
    }
    Ok(path)
}

fn read_document(backend: &Path, path: &Path) -> Result<(Value, Value)> {
    let path = regular(path, backend, false)?;
    let binding = common::binding(&path)?;
    let value = common::read_bound(backend, &binding, "runtime 历史文档")?;
    if common::binding(&path)? != binding {
        return Err("runtime 历史文档在读取期间发生变化".into());
    }
    Ok((value, binding))
}

fn artifact(backend: &Path, value: &Value) -> Result<Value> {
    documents::exact(value, &["path", "sha256"], "runtime 产物")?;
    let path = canonical(Path::new(documents::text(&value["path"], "产物路径")?))?;
    let sha = digest_shape(&value["sha256"])?;
    // 产物按流式摘要核验，不将证据文档的16MiB上限施加到二进制。
    if !path.starts_with(backend) || digest::file(&path)?.sha256 != sha {
        return Err("runtime 产物路径或摘要已变化".into());
    }
    Ok(json!({"path": path, "sha256": sha}))
}

fn identity(value: &Value, artifact: &Value) -> Result<Value> {
    documents::exact(value, &["pid", "started", "executable"], "runtime 创建身份")?;
    let pid = positive_integer(&value["pid"])?;
    let started = documents::text(&value["started"], "创建时刻")?;
    if pid == "1"
        || started.is_empty()
        || started.len() > 32
        || started.starts_with('0')
        || !started.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("runtime 生产者缺少完整进程创建身份".into());
    }
    let path = canonical(Path::new(documents::text(
        &value["executable"],
        "身份产物",
    )?))?;
    let expected = canonical(Path::new(documents::text(&artifact["path"], "登记产物")?))?;
    if !fs::metadata(&path)?.is_file() || path != expected {
        return Err("runtime 创建身份可执行文件与登记产物不一致".into());
    }
    Ok(json!({"pid": value["pid"], "started": started, "executable": path}))
}

fn positive_integer(value: &Value) -> Result<String> {
    let text = value
        .as_number()
        .ok_or("runtime 序号、时间或PID必须是整数")?
        .to_string();
    if text == "0" || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("runtime 时间或PID必须是正整数".into());
    }
    Ok(text)
}
