//! 已完整规划但外层失败的部门计划采用；不重读远程状态、不重放写入。
use super::{adoption_receipts as receipts, artifacts, common, document, model, plan};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn dispatch(backend: &Path, run: &Path, method: &str, args: &Value) -> Result<Value> {
    let state = common::read_bound(
        backend,
        &common::binding(&run.join("state.json"))?,
        "部门采用阶段历史",
    )?;
    if state != args["state"] {
        return Err("部门采用期间阶段历史发生变化".into());
    }
    match method {
        "adoption-inputs" => {
            model::exact_args(args, &["state"])?;
            let selected = attempts(&args["state"])?;
            let mut pids = vec![];
            for attempt in &selected {
                let number = attempt["number"].as_u64().ok_or("部门采用序号无效")?;
                let file = run.join(format!("controller-{number:04}.json"));
                let controller =
                    common::read_bound(backend, &common::binding(&file)?, "部门采用控制器")?;
                let pid = controller["owner"]["identity"]["pid"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("部门控制器 PID 无效")?;
                pids.push(pid);
            }
            Ok(json!({"attempts":selected,"pids":pids}))
        }
        "adoption-evidence" => {
            model::exact_args(
                args,
                &["request", "state", "processes", "producers", "target"],
            )?;
            evidence(backend, run, args)
        }
        _ => Err("未知部门采用操作".into()),
    }
}

fn attempts(state: &Value) -> Result<Vec<Value>> {
    let all = document::array(&state["attempts"], "部门阶段历史")?;
    let Some(current) = all.last() else {
        return Ok(vec![]);
    };
    let mut selected = vec![current.clone()];
    for attempt in all.iter().rev().skip(1) {
        if attempt["stage"] != "seed-runtime"
            || attempt["mode"] != "departments-plan"
            || attempt["status"] != "failed"
        {
            break;
        }
        selected.push(attempt.clone());
    }
    selected.reverse();
    Ok(selected)
}

fn output(run: &Path, attempt: &Value) -> Result<PathBuf> {
    let n = attempt["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("部门采用序号无效")?;
    Ok(run.join(format!("seed-runtime/attempt-{n:04}")))
}

fn guard(backend: &Path, run: &Path, attempt: &Value, target: &Value) -> Result<Value> {
    let n = attempt["number"].as_u64().ok_or("部门采用序号无效")?;
    super::inspect(backend, &output(run, attempt)?, target, n)
}

fn skipped(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    processes: &Value,
    target: &Value,
) -> Result<bool> {
    if !attempt["result"].is_null() {
        return Ok(false);
    }
    let output = output(run, attempt)?;
    let exists = match fs::symlink_metadata(&output) {
        Ok(metadata) => {
            if super::digest::linked(&metadata) {
                return Err("部门计划预检失败目录是链接".into());
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let (controller, value) = receipts::controller(backend, run, attempt, false, processes)?;
    let (_, failure) = receipts::failure(backend, run, attempt, &controller, &value)?;
    if !exists {
        if failure["format_version"] == 2 {
            return Ok(false);
        }
        if !receipts::prefix(
            &failure,
            &[
                ("tools/python/devex_clone_run.py", "execute"),
                ("tools/python/devex_clone_seed_runtime.py", "execute_seed"),
                ("tools/python/devex_clone_post_process.py", "require_quiet"),
            ],
        ) {
            return Err("无结果部门计划失败不是 Bridge 前 quiet gate 预检".into());
        }
        return Ok(true);
    }
    if !receipts::prefix(
        &failure,
        &[
            ("tools/python/devex_clone_run.py", "execute"),
            ("tools/python/devex_clone_seed_runtime.py", "execute_seed"),
            (
                "tools/python/devex_clone_department.py",
                "execute_departments",
            ),
            (
                "tools/python/devex_clone_department.py",
                "adopt_failed_plan",
            ),
            (
                "tools/python/devex_clone_department.py",
                "_adoption_evidence",
            ),
        ],
    ) {
        return Ok(false);
    }
    if !guard(backend, run, attempt, target)?.is_null() {
        return Err("无结果部门计划失败不是采用前完整只读守卫失败".into());
    }
    if document::array(
        &artifacts::inspect(backend, &output, target)?,
        "部门守卫文件",
    )?
    .is_empty()
    {
        return Err("部门计划只读守卫失败缺少外部核验收据".into());
    }
    Ok(true)
}

fn adoption_result(backend: &Path, run: &Path, attempt: &Value, target: &Value) -> Result<Value> {
    let value = common::read_bound(backend, &attempt["result"], "部门采用结果")?;
    document::exact(
        &value,
        &[
            "status",
            "plan",
            "prerequisite",
            "adopted",
            "remote_writes",
            "worker_authorized",
            "restore_qualified",
        ],
        "部门采用结果",
    )?;
    document::exact(
        &value["adopted"],
        &[
            "attempt",
            "failure",
            "controller",
            "current_controller",
            "producer",
            "authentication",
            "plan",
            "prerequisite",
            "before",
            "original_fingerprints",
            "current_fingerprints",
            "cleanup",
            "guard",
        ],
        "部门采用证据",
    )?;
    if value["status"] != "seed_departments_planned"
        || value["remote_writes"].as_u64() != Some(0)
        || value["worker_authorized"] != false
        || value["restore_qualified"] != false
        || guard(backend, run, attempt, target)? != value["adopted"]["guard"]
    {
        return Err("部门计划采用链节点未发布精确只读结果".into());
    }
    common::bound_file(backend, &attempt["result"], "部门采用结果")?;
    Ok(value)
}

fn evidence(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    let selected = attempts(&args["state"])?;
    if selected.len() < 2 {
        return Ok(Value::Null);
    }
    let current = selected.last().ok_or("部门当前阶段缺失")?;
    if current["stage"] != "seed-runtime"
        || current["mode"] != "departments-plan"
        || current["status"] != "running"
    {
        return Err("部门计划采用必须属于紧邻的当前重试 attempt".into());
    }
    let mut failed = selected[..selected.len() - 1].to_vec();
    let mut skipped_nodes = vec![];
    while let Some(tip) = failed.last() {
        if !tip["result"].is_null()
            || !skipped(backend, run, tip, &args["processes"], &args["target"])?
        {
            break;
        }
        skipped_nodes.push(failed.pop().ok_or("部门采用节点缺失")?);
    }
    let tip = failed
        .last()
        .ok_or("部门计划采用链只有空预检失败，没有可采用原始计划")?;
    let origin = if tip["result"].is_null() {
        tip["number"].clone()
    } else {
        adoption_result(backend, run, tip, &args["target"])?["adopted"]["attempt"].clone()
    };
    let origin = origin
        .as_u64()
        .ok_or("部门计划采用链未绑定唯一原始 attempt")?;
    let lineage = failed
        .iter()
        .filter(|a| a["number"].as_u64().is_some_and(|n| n >= origin))
        .collect::<Vec<_>>();
    let candidate = lineage.first().ok_or("部门计划采用原始节点缺失")?;
    if candidate["number"].as_u64() != Some(origin)
        || !candidate["result"].is_null()
        || lineage
            .iter()
            .enumerate()
            .any(|(i, a)| a["number"].as_u64() != Some(origin + i as u64))
    {
        return Err("部门计划采用链不能连续追溯唯一原始 attempt".into());
    }
    let (current_controller, _) =
        receipts::controller(backend, run, current, true, &args["processes"])?;
    let fingerprints = receipts::fingerprints(candidate)?;
    for attempt in &skipped_nodes {
        if receipts::fingerprints(attempt)?["product"] != fingerprints["product"] {
            return Err("部门计划预检失败链包含不同产品构建来源".into());
        }
    }
    let base = original(backend, run, args, candidate, &fingerprints)?;
    for adopted in lineage.iter().skip(1) {
        validate_node(backend, run, args, adopted, &base, &fingerprints)?;
    }
    let current_fingerprints = receipts::fingerprints(current)?;
    if current_fingerprints["product"] != fingerprints["product"] {
        return Err("原部门计划与当前产品构建来源不同，禁止采用".into());
    }
    if !guard(backend, run, current, &args["target"])?.is_null() {
        return Err("当前部门计划采用提前出现守卫清单".into());
    }
    let mut value = base;
    value["current_controller"] = current_controller;
    value["current_fingerprints"] = current_fingerprints;
    Ok(json!({"evidence":value,"prerequisite":plan::read_prerequisite(backend, run)?}))
}

fn original(
    backend: &Path,
    run: &Path,
    args: &Value,
    candidate: &Value,
    fingerprints: &Value,
) -> Result<Value> {
    let number = candidate["number"].as_u64().ok_or("部门原始序号缺失")?;
    let (controller, controller_value) =
        receipts::controller(backend, run, candidate, false, &args["processes"])?;
    let observed = args["producers"]
        .get(number.to_string())
        .ok_or("部门计划缺少生产者观察")?;
    if observed.is_null()
        || observed["alive"] != false
        || observed["producer_kind"] != "department-plan"
        || observed["request"] != args["request"]
        || observed["controller"] != controller_value
    {
        return Err("原部门计划生产者未确认退出或来源登记不同".into());
    }
    let (failure, _) = receipts::failure(backend, run, candidate, &controller, &controller_value)?;
    let prior = plan::read_prerequisite(backend, run)?;
    plan::dispatch(
        backend,
        run,
        &json!("require-prerequisite"),
        &json!({"request":args["request"],"value":prior}),
    )?;
    let plan_descriptor = common::binding(&run.join("seed-runtime/department-plan.json"))?;
    let prerequisite =
        common::binding(&run.join("seed-runtime/departments/identity-prerequisite.json"))?;
    let plan = plan::validate_plan(backend, run, &args["request"])?;
    let output = output(run, candidate)?;
    let mut before = vec![];
    for action in document::array(&plan["actions"], "部门计划动作")? {
        let path = common::bound_file(backend, &action["before_evidence"], "部门原始前像")?;
        let slot = document::text(&action["target"]["slot"], "部门前像槽")?;
        if path != output.join(format!("{slot}-before.json")) {
            return Err("部门计划前像跨 attempt 或租户槽位".into());
        }
        before.push(action["before_evidence"].clone());
    }
    let root = run.join("seed-runtime/departments");
    common::local_directory(backend, &root, "部门原始目录")?;
    let names = fs::read_dir(&root)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    if names != [std::ffi::OsString::from("identity-prerequisite.json")] {
        return Err("原部门计划后出现部门动作目录或未知写入证据".into());
    }
    let source = plan::identity_plan(backend, &args["request"])?;
    let targets = plan::targets(&source)?;
    let templates = plan::template_identities(backend, run, &args["request"])?;
    let authentication = common::binding(&output.join("authentication.json"))?;
    let expected = json!({"scope_id":source["environment"]["scope_id"],
        "tenant_ids":targets.iter().map(|t| t["tenant_id"].clone()).collect::<Vec<_>>(),
        "department_name":targets[0]["body"]["name"],"template_principals":document::array(&templates,"部门模板主体")?.iter().map(model::principal).collect::<Result<Vec<_>>>()?});
    if common::read_bound(backend, &authentication, "部门原始初始化")? != expected {
        return Err("原部门计划初始化未绑定当前十租户和模板主体".into());
    }
    let cleanup = output.join("department-logout-error.json");
    let cleanup = if fs::symlink_metadata(&cleanup).is_ok() {
        let descriptor = common::binding(&cleanup)?;
        if common::read_bound(backend, &descriptor, "部门关闭诊断")?
            != json!({"error_type":"ValueError"})
        {
            return Err("原部门计划关闭诊断发生变化".into());
        }
        descriptor
    } else {
        Value::Null
    };
    common::bound_file(backend, &plan_descriptor, "部门原始计划")?;
    common::bound_file(backend, &prerequisite, "部门原始前提")?;
    common::bound_file(backend, &observed["receipt"], "部门生产者观察")?;
    Ok(
        json!({"attempt":number,"failure":failure,"controller":controller,"producer":observed["receipt"],
        "authentication":authentication,"plan":plan_descriptor,"prerequisite":prerequisite,"before":before,
        "original_fingerprints":fingerprints,"cleanup":cleanup}),
    )
}

fn validate_node(
    backend: &Path,
    run: &Path,
    args: &Value,
    adopted: &Value,
    base: &Value,
    fingerprints: &Value,
) -> Result<()> {
    let current = receipts::fingerprints(adopted)?;
    if current["product"] != fingerprints["product"] {
        return Err("部门计划采用链包含不同产品构建来源".into());
    }
    if adopted["result"].is_null() {
        if !skipped(backend, run, adopted, &args["processes"], &args["target"])? {
            return Err("部门计划采用链包含无结果的非预检失败".into());
        }
        return Ok(());
    }
    let (controller, value) =
        receipts::controller(backend, run, adopted, false, &args["processes"])?;
    receipts::failure(backend, run, adopted, &controller, &value)?;
    let guard = guard(backend, run, adopted, &args["target"])?;
    if guard.is_null() {
        return Err("部门计划采用链成功节点缺少只读守卫清单".into());
    }
    let mut expected = base.clone();
    expected["current_controller"] = controller;
    expected["current_fingerprints"] = current;
    expected["guard"] = guard;
    if adoption_result(backend, run, adopted, &args["target"])?
        != json!({"status":"seed_departments_planned",
        "plan":base["plan"],"prerequisite":base["prerequisite"],"adopted":expected,"remote_writes":0,"worker_authorized":false,"restore_qualified":false})
    {
        return Err("部门计划采用链结果与原始证据或当前节点不同".into());
    }
    Ok(())
}
