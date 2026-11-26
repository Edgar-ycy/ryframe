//! 消费统一阶段选择器已经确认的最新记录，不另建阶段状态机。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{common, document, history, model, plan};
use crate::Result;

pub(super) fn dispatch(backend: &Path, run: &Path, method: &Value, args: &Value) -> Result<Value> {
    let fields: &[&str] = if method == "authorize-identity" {
        &["request", "record", "mode"]
    } else {
        &["request", "record"]
    };
    model::exact_args(args, fields)?;
    let verify_resume = method == "authorize-identity" && args["mode"] == "verify";
    let proof = department_evidence_for(
        backend,
        run,
        &args["request"],
        &args["record"],
        verify_resume,
    )?;
    if method == "department-evidence" {
        return Ok(proof);
    }
    let saved = plan::read_prerequisite(backend, run)?["identity"].clone();
    match args["mode"].as_str() {
        Some("apply") => {
            if saved["state"] != "absent"
                || plan::identity_projection(backend, run, &args["request"], true)? != saved
            {
                return Err("身份 apply 只允许在部门验证后从明确空账本开始，不能重放".into());
            }
        }
        Some("verify") => {
            let actual = plan::identity_projection(backend, run, &args["request"], false)?;
            if !matches!(
                actual["state"].as_str(),
                Some("prepared" | "verifying" | "verified")
            ) || saved["state"] == "prepared"
                && actual["projection_sha256"] != saved["projection_sha256"]
            {
                return Err("身份 verify 缺少全部 622 项确认或补部门前后投影不同".into());
            }
        }
        _ => return Err("未知身份模式".into()),
    }
    Ok(proof)
}

pub(super) fn department_evidence(
    backend: &Path,
    run: &Path,
    request: &Value,
    record: &Value,
) -> Result<Value> {
    department_evidence_for(backend, run, request, record, false)
}

fn department_evidence_for(
    backend: &Path,
    run: &Path,
    request: &Value,
    record: &Value,
    verify_resume: bool,
) -> Result<Value> {
    let plan = if verify_resume {
        plan::validate_plan_for_identity_verification(backend, run, request)?
    } else {
        plan::validate_plan(backend, run, request)?
    };
    let result = common::read_bound(backend, &record["result"], "部门最终阶段结果")?;
    document::exact(
        &result,
        &[
            "status",
            "plan",
            "confirmed",
            "observed",
            "remote_writes",
            "worker_authorized",
            "restore_qualified",
        ],
        "部门最终结果",
    )?;
    let targets = plan::targets(&plan::identity_plan(backend, request)?)?;
    let actions = document::array(&plan["actions"], "部门动作")?;
    let mut confirmed = Vec::new();
    for (action, target) in actions.iter().zip(&targets) {
        let root = run
            .join("seed-runtime/departments")
            .join(document::text(&action["target"]["slot"], "部门目标槽")?);
        common::local_directory(backend, &root, "部门动作根目录")?;
        let mut attempts = fs::read_dir(root)?
            .map(|item| item.map(|item| item.path()))
            .collect::<std::io::Result<Vec<PathBuf>>>()?;
        attempts.sort();
        confirmed.push(history::action_history(
            backend, run, action, target, &attempts,
        )?);
    }
    let bound_plan = common::binding(&run.join("seed-runtime/department-plan.json"))?;
    if record["mode"] != "departments-verify"
        || result["status"] != "seed_departments_verified"
        || result["plan"] != bound_plan
        || result["confirmed"] != json!(confirmed)
        || document::array(&result["observed"], "部门最终观察")?.len() != 10
        || result["remote_writes"].as_u64() != Some(0)
        || result["worker_authorized"] != false
        || result["restore_qualified"] != false
    {
        return Err("部门必须在最新外层只读验证中全部通过".into());
    }
    let number = record["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("部门阶段序号无效")?;
    for ((descriptor, action), target) in document::array(&result["observed"], "部门最终观察")?
        .iter()
        .zip(actions)
        .zip(&targets)
    {
        let path = common::bound_file(backend, descriptor, "部门最终观察")?;
        if path.parent()
            != Some(
                run.join(format!("seed-runtime/attempt-{number:04}"))
                    .as_path(),
            )
            || model::classify(
                action,
                &common::read_bound(backend, descriptor, "部门最终观察")?,
                target,
            )? != "after"
        {
            return Err("部门最终观察不属于当前只读验证或完整后像".into());
        }
    }
    common::bound_file(backend, &record["result"], "部门最终阶段结果")?;
    Ok(
        json!({"stage":record["result"],"plan":bound_plan,"confirmed":confirmed,"prerequisite":plan["prerequisite"]}),
    )
}
