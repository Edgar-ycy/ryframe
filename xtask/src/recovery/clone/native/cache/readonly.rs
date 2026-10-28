//! Redis 创建收据的原生只读观察；不启动冻结 Python helper、不发送停止信号。
use super::{
    super::{digest, json as document, resources::common},
    evidence,
    linux_observer::{FileProof, Identity, Observer},
    model,
};
use crate::{Result, recovery::fixture_control::services::lifecycle::readonly::process};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn status(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    effective: Option<&Value>,
) -> Result<Value> {
    let current = effective.unwrap_or(request);
    model::validate(backend, current, true)?;
    let tools = effective
        .map(|value| successor_tools(request, value))
        .transpose()?;
    let output = Path::new(document::text(&runtime["output"], "缓存运行目录")?);
    let observed = evidence::inspect(request, output)?;
    if observed.is_null()
        || observed.as_object().is_none_or(|fields| {
            fields
                .iter()
                .any(|(key, value)| runtime.get(key) != Some(value))
        })
    {
        return Err("缓存运行证据与原创建身份不同".into());
    }
    let files = frozen_files(output)?;
    common::directory_identity(backend, &current["directory"])?;
    let launcher_alive = launcher(request, runtime)?;
    let partial = runtime.get("partial_receipt").is_some();
    if launcher_alive && (partial || effective.is_some()) {
        return Err("partial 或前代 launcher 仍存活，拒绝关闭证明".into());
    }
    let observer = Observer::new(backend, current)?;
    let identity = if partial {
        None
    } else {
        model::identity(&runtime["linux_identity"], true)?;
        Some(serde_json::from_value::<Identity>(
            runtime["linux_identity"].clone(),
        )?)
    };
    let observed_linux =
        observer.observe(linux_files(backend, request, output, &files)?, identity)?;
    let mut result = if runtime.get("partial_receipt").is_some() {
        if launcher_alive {
            return Err("Redis 部分启动 launcher PID 当前仍被占用".into());
        }
        if !observed_linux.bound_and_closed {
            return Err("partial 缺少原生 bind 关闭证明".into());
        }
        json!({"state":"stopped","partial":runtime["state"],"launcher_alive":false})
    } else {
        let linux = json!({"identity":observed_linux.identity,"alive":observed_linux.alive,
            "boot_id":observed_linux.boot_id,"terminated":false});
        if linux["alive"] == true {
            if effective.is_some() || linux["boot_id"] != runtime["boot_id"] {
                return Err("缓存前代未停止或存活状态与登记 boot 不符".into());
            }
            json!({"state":"running","launcher_alive":launcher_alive,"linux":linux})
        } else {
            if launcher_alive {
                return Err("Redis 已退出但 WSL launcher 仍在运行".into());
            }
            if !observed_linux.bound_and_closed {
                return Err("缓存缺少原生 bind 关闭证明".into());
            }
            json!({"state":"stopped","launcher_alive":false,"linux":linux})
        }
    };
    if result["state"] == "stopped" {
        super::super::target_verify::authority::require_closed(&format!(
            "http://127.0.0.1:{}",
            request["port"].as_u64().ok_or("Redis 端口无效")?
        ))?;
    }
    observer.finish()?;
    if evidence::inspect(request, output)? != observed
        || frozen_files(output)? != files
        || launcher(request, runtime)? != launcher_alive
    {
        return Err("缓存观察期间原始证据或 launcher 身份变化".into());
    }
    if let Some(tools) = tools {
        result["tools_sha256"] = json!(tools);
    }
    Ok(result)
}

fn launcher(request: &Value, runtime: &Value) -> Result<bool> {
    let output = Path::new(document::text(&runtime["output"], "缓存运行目录")?);
    if runtime.get("partial_receipt").is_some() {
        let proof = document::read(&output.join("partial.json"))?;
        let pid = proof["launcher_pid"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or("部分启动 PID 无效")?;
        return Ok(common::process_identity(pid)?.is_some());
    }
    let expected = common::validate_process_identity(&runtime["launcher"])?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("launcher PID 无效")?)?;
    let Some(actual) = common::process_identity(pid)? else {
        return Ok(false);
    };
    if actual != expected
        || process::arguments(&expected)? != evidence::command(request, output, "serve")?
    {
        return Err("WSL launcher 创建身份或实际参数不符".into());
    }
    Ok(true)
}

fn linux_files(
    backend: &Path,
    request: &Value,
    output: &Path,
    files: &[Value],
) -> Result<Vec<FileProof>> {
    let payload = document::read(&output.join("launch-request.json"))?;
    let linux = model::linux_path(output.to_str().ok_or("缓存目录不是 UTF-8")?)?;
    if payload["linux_output"] != linux {
        return Err("缓存冻结目录跨 OS 不同".into());
    }
    let mut proofs = Vec::new();
    for file in files
        .iter()
        .chain(std::iter::once(&request["configuration"]))
    {
        let path = common::bound_file(backend, file, "缓存冻结文件")?;
        let proof = FileProof {
            path: model::linux_path(path.to_str().ok_or("冻结路径不是 UTF-8")?)?,
            bytes: file["bytes"].as_u64().ok_or("冻结文件无大小")?,
            sha256: document::text(&file["sha256"], "冻结摘要")?.into(),
        };
        proofs.push(proof);
    }
    if payload["format_version"] == 1 {
        for file in document::object(&payload["helpers"], "冻结辅助文件")?.values() {
            proofs.push(serde_json::from_value(file.clone())?);
        }
    }
    proofs.sort_by(|left, right| left.path.cmp(&right.path));
    proofs.dedup();
    Ok(proofs)
}

pub(super) fn port(backend: &Path, operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "status" => {
            document::exact(args, &["request", "runtime"], "缓存只读状态")?;
            status(backend, &args["request"], &args["runtime"], None)
        }
        "predecessor" => {
            document::exact(
                args,
                &["request", "runtime", "effective"],
                "缓存前代只读状态",
            )?;
            status(
                backend,
                &args["request"],
                &args["runtime"],
                Some(&args["effective"]),
            )
        }
        _ => Err("未知缓存只读端口".into()),
    }
}

fn successor_tools(original: &Value, effective: &Value) -> Result<String> {
    document::exact(original, &model::fields(original), "原缓存进程")?;
    document::exact(effective, &model::fields(original), "缓存工具后继")?;
    for key in model::fields(original)
        .iter()
        .copied()
        .filter(|key| !["wsl", "python", "native_tool"].contains(key))
    {
        if original[key] != effective[key] {
            return Err("缓存工具换代不得替换原资源或端点".into());
        }
    }
    if original["format_version"] == 2 {
        super::native_tool::successor(
            original,
            &json!({"wsl":effective["wsl"],"native_tool":effective["native_tool"],
            "redis_server":{"distribution":original["distribution"],"resolved_path":original["executable"],"sha256":original["sha256"]}}),
        )?;
        return super::native_tool::tools_hash(effective);
    }
    if original["wsl"]["path"]
        .as_str()
        .map(str::to_ascii_lowercase)
        != effective["wsl"]["path"]
            .as_str()
            .map(str::to_ascii_lowercase)
        || original["python"]["path"] != effective["python"]["path"]
        || original["python"]["executable"] != effective["python"]["executable"]
    {
        return Err("缓存工具换代不得替换原工具路径".into());
    }
    digest::plan_hash(&json!({"wsl":effective["wsl"],"python":effective["python"]}))
}

fn frozen_files(output: &Path) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(output)? {
        let path = entry?.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_file() && path.extension().is_some_and(|value| value == "json") {
            model::plain(&path)?;
            result.push(common::binding(&path)?);
        }
    }
    result.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_tool_successor_keeps_all_resource_and_path_facts() {
        let mut original = Value::Object(
            model::FIELDS
                .iter()
                .map(|field| ((*field).to_owned(), Value::Null))
                .collect(),
        );
        original["wsl"] = json!({"path":"D:/tools/wsl.exe","sha256":"a".repeat(64)});
        original["python"] = json!({"path":"/usr/bin/python3","executable":"/usr/bin/python3.12","sha256":"b".repeat(64)});
        original["port"] = json!(16390);
        let mut next = original.clone();
        next["wsl"]["sha256"] = json!("c".repeat(64));
        next["python"]["sha256"] = json!("d".repeat(64));
        let before = original.clone();
        assert!(successor_tools(&original, &next).is_ok());
        for field in model::FIELDS
            .iter()
            .copied()
            .filter(|field| !["wsl", "python"].contains(field))
        {
            let mut changed = next.clone();
            changed[field] = json!("different-resource");
            assert!(successor_tools(&original, &changed).is_err());
        }
        for (group, field) in [
            ("wsl", "path"),
            ("python", "path"),
            ("python", "executable"),
        ] {
            let mut changed = next.clone();
            changed[group][field] = json!("different-tool");
            assert!(successor_tools(&original, &changed).is_err());
        }
        next["unknown"] = json!(true);
        assert!(successor_tools(&original, &next).is_err());
        assert_eq!(original, before);
    }
}
