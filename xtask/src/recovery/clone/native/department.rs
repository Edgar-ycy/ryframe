//! 部门阶段、受管证据与采用链；外部命令和会话仍由统一控制器调度。
use std::path::Path;

use serde_json::{Value, json};

use super::{digest, json as document, resources::common};
use crate::Result;

#[path = "department/adopt_plan.rs"]
pub(super) mod adopt_plan;
#[path = "department/adoption.rs"]
mod adoption;
#[path = "department/adoption_receipts.rs"]
mod adoption_receipts;
#[path = "department/artifacts.rs"]
mod artifacts;
#[path = "department/evidence.rs"]
mod evidence;
#[path = "department/history.rs"]
mod history;
#[path = "department/model.rs"]
mod model;
#[path = "department/orchestration.rs"]
pub(super) mod orchestration;
#[path = "department/plan.rs"]
mod plan;
#[path = "department/receipts.rs"]
mod receipts;
#[path = "department/workbook.rs"]
mod workbook;
#[path = "department/workflow.rs"]
mod workflow;

pub(super) fn read_prerequisite(backend: &Path, run: &Path) -> Result<Value> {
    plan::read_prerequisite(backend, run)
}

pub(super) fn require_prerequisite(
    backend: &Path,
    run: &Path,
    request: &Value,
    prior: &Value,
) -> Result<()> {
    model::execute(backend, run, &json!("require-prerequisite"), &json!({
        "request":{"identity_plan":request["identity_plan"],"identity_state":request["identity_state"]},
        "value":prior
    })).map(|_| ())
}

/// 复用最新阶段选择器和部门证据算法，不创建中间受管输入文件。
pub(crate) fn verified_evidence(
    backend: &Path,
    run: &Path,
    request: &Value,
    current: Option<u64>,
) -> Result<Value> {
    verified_evidence_for(backend, run, request, current, false)
}

pub(super) fn authorize_identity_verification(
    backend: &Path,
    run: &Path,
    request: &Value,
    current: u64,
) -> Result<Value> {
    verified_evidence_for(backend, run, request, Some(current), true)
}

fn verified_evidence_for(
    backend: &Path,
    run: &Path,
    request: &Value,
    current: Option<u64>,
    authorize_identity: bool,
) -> Result<Value> {
    let request_path = run.join("seed-runtime.json");
    let request_binding = common::binding(&request_path)?;
    if common::read_bound(backend, &request_binding, "部门seed登记请求")? != *request {
        return Err("部门请求不同于当前seed登记".into());
    }
    let before = super::lifecycle::state::load(run, true)?;
    let record = super::seed::latest(
        &before,
        "seed-runtime",
        &[
            "departments-plan",
            "departments-apply",
            "departments-reconcile",
            "departments-verify",
        ],
        current,
        false,
    )?;
    let result = if authorize_identity {
        evidence::dispatch(
            backend,
            run,
            &json!("authorize-identity"),
            &json!({"request":{"identity_plan":request["identity_plan"],
                "identity_state":request["identity_state"]},"record":record,"mode":"verify"}),
        )?
    } else {
        evidence::department_evidence(backend, run, request, record)?
    };
    common::bound_file(backend, &record["result"], "部门最终阶段结果")?;
    common::bound_file(backend, &request["identity_plan"], "部门身份计划")?;
    common::bound_file(backend, &request_binding, "部门seed登记请求")?;
    if super::lifecycle::state::load(run, true)? != before {
        return Err("部门核验期间完整阶段状态变化；保留现场，禁止重放".into());
    }
    Ok(result)
}

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
    if args["operation"] == "model" {
        return model::dispatch(backend, run, args);
    }
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
#[path = "../../../../tests/unit/recovery/clone/native/department/history_tests.rs"]
mod history_tests;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/model_tests.rs"]
mod model_tests;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/orchestration_tests.rs"]
mod orchestration_tests;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/plan_tests.rs"]
mod plan_tests;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/projection_tests.rs"]
mod projection_tests;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/department/adoption_tests.rs"]
mod adoption_tests;
