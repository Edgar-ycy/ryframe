//! 部门采用链的只读证据闭集；不运行外部命令，不改变既有阶段生命周期。
use std::path::Path;

use serde_json::{Value, json};

use super::{digest, json as document, resources::common};
use crate::Result;

#[path = "department/artifacts.rs"]
mod artifacts;
#[path = "department/receipts.rs"]
mod receipts;

pub(crate) fn dispatch(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    document::exact(args, &["backend_root", "parameters"], "部门证据工作区绑定")?;
    let selected = Path::new(document::text(&args["backend_root"], "部门证据后端")?);
    if selected != backend {
        common::local_directory(backend, selected, "隔离部门证据后端")?;
    }
    common::local_directory(selected, run, "部门证据运行目录")?;
    execute(selected, run, &args["parameters"])
}

pub(crate) fn execute(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    if args["operation"] == "identity-projection" {
        return projection(backend, run, args);
    }
    document::exact(
        args,
        &["operation", "output", "target", "attempt"],
        "部门守卫参数",
    )?;
    let output = Path::new(document::text(&args["output"], "部门守卫目录")?);
    if output.parent() != Some(run.join("seed-runtime").as_path()) {
        return Err("部门守卫目录必须属于当前 run 的 seed 阶段".into());
    }
    match args["operation"].as_str() {
        Some("artifacts") if args["attempt"].is_null() => {
            artifacts::inspect(backend, output, &args["target"])
        }
        Some("inspect" | "publish") => {
            let attempt = args["attempt"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or("部门守卫序号无效")?;
            if output != run.join(format!("seed-runtime/attempt-{attempt:04}")) {
                return Err("部门守卫目录与 attempt 不同".into());
            }
            if args["operation"] == "publish" {
                if !inspect(backend, output, &args["target"], attempt)?.is_null() {
                    return Err("部门采用守卫清单已经存在，不能覆盖".into());
                }
                let files = artifacts::inspect(backend, output, &args["target"])?;
                document::write_new(
                    &output.join("adoption-guard.json"),
                    &json!({
                        "format_version":1,"kind":"devex-department-adoption-guard",
                        "attempt":attempt,"artifacts":files
                    }),
                )?;
            }
            inspect(backend, output, &args["target"], attempt)
        }
        _ => Err("未知部门守卫操作或参数".into()),
    }
}

fn projection(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["operation", "request", "allow_absent"],
        "部门身份投影参数",
    )?;
    let request = &args["request"];
    document::exact(
        request,
        &["identity_plan", "identity_state"],
        "部门身份输入",
    )?;
    let mut plan = common::read_bound(backend, &request["identity_plan"], "部门身份计划")?;
    document::exact(
        &plan,
        &[
            "format_version",
            "kind",
            "configuration",
            "environment",
            "groups",
            "plan_sha256",
        ],
        "部门身份计划",
    )?;
    let hash = plan
        .as_object_mut()
        .ok_or("部门身份计划不是对象")?
        .remove("plan_sha256")
        .ok_or("部门身份计划摘要缺失")?;
    if plan["format_version"].as_u64() != Some(1)
        || plan["kind"] != "devex-identities"
        || hash.as_str() != Some(digest::plan_hash(&plan)?.as_str())
    {
        return Err("部门身份计划摘要无效".into());
    }
    plan["plan_sha256"] = hash;
    let root = Path::new(document::text(&request["identity_state"], "部门身份账本")?);
    if root != run.join("seed-runtime/identities") {
        return Err("部门身份账本不属于当前 seed run".into());
    }
    let allow_absent = document::boolean(&args["allow_absent"], "部门身份允许缺失")?;
    let value = crate::data::performance_identities::seed_evidence::projection(
        backend,
        &plan,
        root,
        allow_absent,
    )?;
    common::bound_file(backend, &request["identity_plan"], "部门身份计划")?;
    Ok(value)
}

fn inspect(backend: &Path, output: &Path, target: &Value, attempt: u64) -> Result<Value> {
    let files = artifacts::inspect(backend, output, target)?;
    let path = output.join("adoption-guard.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let descriptor = common::binding(&path)?;
    let manifest = common::read_bound(backend, &descriptor, "部门采用守卫清单")?;
    if manifest
        != json!({"format_version":1,"kind":"devex-department-adoption-guard",
        "attempt":attempt,"artifacts":files})
        || common::binding(&path)? != descriptor
        || artifacts::inspect(backend, output, target)? != files
    {
        return Err("部门守卫清单、文件闭集或原始证据变化；保留现场，禁止自动重放".into());
    }
    Ok(descriptor)
}

#[cfg(test)]
#[path = "department/projection_tests.rs"]
mod projection_tests;
#[cfg(test)]
#[path = "department/tests.rs"]
mod tests;
