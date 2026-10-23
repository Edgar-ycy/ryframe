//! 部门计划与写入阶段的受管证据写入；外部请求由现有适配层执行。
use super::{common, digest, document, history, model, plan};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn dispatch(backend: &Path, run: &Path, method: &str, args: &Value) -> Result<Value> {
    match method {
        "prepare-prerequisite" => prepare(backend, run, args),
        "initial-action" => initial(args),
        "load-plan" => load_plan(backend, run, args),
        "plan-action" => plan_action(backend, run, args),
        "publish-plan" => publish(backend, run, args),
        "authenticate" => authenticate(backend, run, args),
        "reconciled-before" => {
            model::exact_args(args, &["plan", "action", "target", "attempt"])?;
            reconciled_before(
                backend,
                run,
                &args["action"],
                &args["target"],
                &path(args, "attempt")?,
            )
        }
        "begin-action" => begin(backend, run, args),
        "write-intent" => write_intent(backend, run, args),
        "write-response" => write_response(backend, run, args),
        "complete-action" => complete(backend, run, args),
        "reconcile-action" => reconcile(backend, run, args),
        "confirm-action" => confirm(backend, run, args),
        "verify-action" => verify(backend, run, args),
        "stage-result" => result(backend, run, args),
        method => super::adoption::dispatch(backend, run, method, args),
    }
}

fn path(value: &Value, key: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(document::text(&value[key], "部门证据路径")?))
}

fn stage_output(backend: &Path, run: &Path, value: &Value) -> Result<PathBuf> {
    let output = path(value, "output")?;
    let name = output
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("部门输出目录名无效")?;
    let number = name.strip_prefix("attempt-").ok_or("部门输出目录名无效")?;
    if output.parent() != Some(run.join("seed-runtime").as_path())
        || number.len() < 4
        || !number.bytes().all(|b| b.is_ascii_digit())
        || number.parse::<u64>().ok().filter(|n| *n > 0).is_none()
    {
        return Err("部门输出目录不属于当前运行阶段".into());
    }
    common::local_directory(backend, &output, "部门阶段输出")?;
    Ok(output)
}

fn prepare(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["request"])?;
    let current = plan::identity_projection(backend, run, &args["request"], true)?;
    if current["state"] == "verified" {
        return Err("身份已验证后不再补建部门计划".into());
    }
    let root = run.join("seed-runtime/departments");
    let file = root.join("identity-prerequisite.json");
    let value = json!({"format_version":1,"kind":"devex-seed-identity-prerequisite",
        "registration":common::binding(&run.join("seed-runtime.json"))?,"identity":current});
    if root.try_exists()? {
        common::local_directory(backend, &root, "部门证据目录")?;
        if document::read(&file)? != value {
            return Err("已有部门身份前提不同".into());
        }
    } else {
        common::ensure_new_directory(backend, &root, "部门证据目录")?;
        document::write_new(&file, &value)?;
    }
    plan::read_prerequisite(backend, run)
}

fn load_plan(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["request", "record"])?;
    let record = &args["record"];
    let result = common::read_bound(backend, &record["result"], "部门规划阶段结果")?;
    if record["mode"] != "departments-plan"
        || result["status"] != "seed_departments_planned"
        || result["plan"] != common::binding(&run.join("seed-runtime/department-plan.json"))?
    {
        return Err("部门计划缺少当前登记的外层成功证明".into());
    }
    let plan = plan::validate_plan(backend, run, &args["request"])?;
    common::bound_file(backend, &record["result"], "部门规划阶段结果")?;
    Ok(plan)
}

fn initial(args: &Value) -> Result<Value> {
    model::exact_args(args, &["target", "template"])?;
    let target = &args["target"];
    Ok(
        json!({"target":{"slot":target["slot"],"tenant_id":target["tenant_id"]},
        "body":target["body"],"expected":target["expected"],"before":null,"before_evidence":null,
        "before_template_paths":null,"principal":null,"template_principal":model::principal(&args["template"])?}),
    )
}

fn plan_action(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target", "observed", "output"])?;
    let output = stage_output(backend, run, args)?;
    let mut action = args["action"].clone();
    let current = model::image(&action, &args["observed"], &args["target"])?;
    let slot = document::text(&args["target"]["slot"], "部门槽")?;
    if !regex::Regex::new(r"^tenant-(?:0[1-9]|10)$")?.is_match(slot) {
        return Err("部门目标槽无效".into());
    }
    let file = output.join(format!("{slot}-before.json"));
    document::write_new(&file, &args["observed"])?;
    if !current["department"].is_null()
        || document::array(&current["template"]["paths"], "模板路径")?
            .contains(&action["body"]["name"])
    {
        return Err("固定 seed 部门名称已经存在或模板状态矛盾".into());
    }
    action["before"] = current["directory"].clone();
    action["before_evidence"] = common::binding(&file)?;
    action["before_template_paths"] = current["template"]["paths"].clone();
    action["principal"] = current["principal"].clone();
    Ok(action)
}

fn publish(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["request", "actions"])?;
    let prerequisite =
        common::binding(&run.join("seed-runtime/departments/identity-prerequisite.json"))?;
    let mut value = json!({"format_version":1,"kind":"devex-seed-departments",
        "registration":common::binding(&run.join("seed-runtime.json"))?,
        "identity_plan":args["request"]["identity_plan"],"prerequisite":prerequisite,"actions":args["actions"]});
    value["plan_sha256"] = json!(digest::plan_hash(&value)?);
    let file = run.join("seed-runtime/department-plan.json");
    if fs::symlink_metadata(&file).is_ok() {
        if document::read(&file)? != value {
            return Err("已有部门计划不同，不能覆盖或重新规划".into());
        }
    } else {
        document::write_new(&file, &value)?;
    }
    plan::validate_plan(backend, run, &args["request"])?;
    Ok(
        json!({"status":"seed_departments_planned","plan":common::binding(&file)?,
        "prerequisite":prerequisite,"remote_writes":0,"worker_authorized":false,"restore_qualified":false}),
    )
}

fn authenticate(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["request", "auth", "output"])?;
    let source = plan::identity_plan(backend, &args["request"])?;
    let targets = plan::targets(&source)?;
    let templates = plan::template_identities(backend, run, &args["request"])?;
    let principals = document::array(&templates, "部门模板身份")?
        .iter()
        .map(model::principal)
        .collect::<Result<Vec<_>>>()?;
    let expected = json!({"scope_id":source["environment"]["scope_id"],
        "tenant_ids":targets.iter().map(|t| t["tenant_id"].clone()).collect::<Vec<_>>(),
        "department_name":targets[0]["body"]["name"],"template_principals":principals});
    if args["auth"] != expected {
        return Err("部门 bridge 初始化未绑定固定 scope、租户和名称".into());
    }
    document::write_new(
        &stage_output(backend, run, args)?.join("authentication.json"),
        &expected,
    )?;
    Ok(expected)
}

pub(super) fn directories(
    backend: &Path,
    run: &Path,
    action: &Value,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let slot = document::text(&action["target"]["slot"], "部门目标槽")?;
    if !regex::Regex::new(r"^tenant-(?:0[1-9]|10)$")?.is_match(slot) {
        return Err("部门目标槽无效".into());
    }
    let root = run.join("seed-runtime/departments").join(slot);
    crate::local_test_path::validate_local_test_path(
        &root,
        backend,
        crate::local_test_path::LocalTestPathKind::StateDirectory,
    )?;
    let mut paths = if root.try_exists()? {
        fs::read_dir(&root)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?
    } else {
        vec![]
    };
    paths.sort();
    for (index, child) in paths.iter().enumerate() {
        if child.file_name().and_then(|n| n.to_str()) != Some(format!("{:04}", index + 1).as_str())
        {
            return Err("部门动作目录不连续或含未知文件".into());
        }
        common::local_directory(backend, child, "部门动作目录")?;
    }
    Ok((root, paths))
}

pub(super) fn reconciled_before(
    backend: &Path,
    run: &Path,
    action: &Value,
    target: &Value,
    attempt: &Path,
) -> Result<Value> {
    history::attempt_path(backend, run, action, attempt)?;
    let intent = history::intent(backend, run, action, attempt)?;
    if attempt.join("api-response.json").try_exists()? {
        return Err("已有部门成功响应必须先核对为后像，禁止重放".into());
    }
    let mut files = fs::read_dir(attempt)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("reconcile-") && n.ends_with(".json"))
    });
    if files.is_empty() {
        return Err("存在未知部门写入，必须先显式核对，不能重放".into());
    }
    for file in files {
        if history::reconciliation(backend, run, action, target, attempt, &file)? != "before" {
            return Err("上次部门核对未证明完整空前像，禁止新写入".into());
        }
    }
    Ok(intent)
}

fn begin(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target"])?;
    let (root, previous) = directories(backend, run, &args["action"])?;
    let mut key = uuid::Uuid::new_v4().to_string();
    if let Some(last) = previous.last() {
        if last.join("confirmed.json").try_exists()? {
            history::confirmation(backend, run, &args["action"], &args["target"], last)?;
            return Ok(json!({"confirmed":true,"attempt":last,"idempotency_key":null}));
        }
        if last.join("intent.json").try_exists()? {
            key = document::text(&reconciled_before(backend, run, &args["action"], &args["target"], last)?["idempotency_key"], "部门幂等键")?.to_owned();
        } else if fs::read_dir(last)?
            .any(|e| e.is_err() || e.is_ok_and(|e| e.file_name() != "before.json"))
        {
            return Err("未登记 POST 的部门目录含未知证据".into());
        }
    }
    common::ensure_directory(backend, &root, "部门动作根目录")?;
    let attempt = root.join(format!("{:04}", previous.len() + 1));
    common::ensure_new_directory(backend, &attempt, "部门动作尝试")?;
    Ok(json!({"confirmed":false,"attempt":attempt,"idempotency_key":key}))
}

fn write_intent(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(
        args,
        &["action", "target", "attempt", "observed", "idempotency_key"],
    )?;
    let attempt = path(args, "attempt")?;
    history::attempt_path(backend, run, &args["action"], &attempt)?;
    document::write_new(&attempt.join("before.json"), &args["observed"])?;
    if model::classify(&args["action"], &args["observed"], &args["target"])? != "before" {
        return Err("POST 前租户部门目录不再为空".into());
    }
    let intent = json!({"plan":common::binding(&run.join("seed-runtime/department-plan.json"))?,
        "target":args["action"]["target"],"before":common::binding(&attempt.join("before.json"))?,
        "body":args["action"]["body"],"operation":"post_system_depts", "idempotency_key":args["idempotency_key"],"automatic_retry":false});
    document::write_new(&attempt.join("intent.json"), &intent)?;
    history::intent(backend, run, &args["action"], &attempt)
}

fn write_response(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target", "attempt", "response"])?;
    let attempt = path(args, "attempt")?;
    history::attempt_path(backend, run, &args["action"], &attempt)?;
    history::intent(backend, run, &args["action"], &attempt)?;
    document::write_new(&attempt.join("api-response.json"), &args["response"])?;
    model::created_response(&args["action"], &args["target"], &args["response"])
}

fn save_observed(backend: &Path, run: &Path, args: &Value) -> Result<(PathBuf, Value)> {
    let attempt = path(args, "attempt")?;
    history::attempt_path(backend, run, &args["action"], &attempt)?;
    let output = stage_output(backend, run, args)?;
    let name = output
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("部门阶段名无效")?;
    let file = attempt.join(format!(
        "observed-{}.json",
        name.trim_start_matches("attempt-")
    ));
    document::write_new(&file, &args["observed"])?;
    Ok((attempt, common::binding(&file)?))
}

fn complete(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target", "attempt", "observed", "output"])?;
    let attempt = path(args, "attempt")?;
    history::attempt_path(backend, run, &args["action"], &attempt)?;
    let created = model::created_response(
        &args["action"],
        &args["target"],
        &document::read(&attempt.join("api-response.json"))?,
    )?;
    if model::image(&args["action"], &args["observed"], &args["target"])?["department"] != created
        || model::classify(&args["action"], &args["observed"], &args["target"])? != "after"
    {
        return Err("创建响应与随后完整目录及导入模板不同".into());
    }
    let (_, descriptor) = save_observed(backend, run, args)?;
    Ok(json!({"observed":descriptor,"response_proven":true,"reconcile":null,"state":"after"}))
}

fn reconcile(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target", "attempt", "observed", "output"])?;
    let (attempt, observed) = save_observed(backend, run, args)?;
    let state = model::classify(&args["action"], &args["observed"], &args["target"])?;
    if attempt.join("api-response.json").try_exists()? {
        return complete_saved(backend, run, args, observed);
    }
    let file = path(&observed, "path")?;
    let reconcile = file.with_file_name(
        file.file_name()
            .and_then(|n| n.to_str())
            .ok_or("部门观察名无效")?
            .replacen("observed-", "reconcile-", 1),
    );
    document::write_new(
        &reconcile,
        &json!({"intent":common::binding(&attempt.join("intent.json"))?,"observed":observed,"state":state}),
    )?;
    Ok(
        json!({"observed":observed,"response_proven":false,"reconcile":common::binding(&reconcile)?,"state":state}),
    )
}

fn complete_saved(backend: &Path, run: &Path, args: &Value, observed: Value) -> Result<Value> {
    let attempt = path(args, "attempt")?;
    history::intent(backend, run, &args["action"], &attempt)?;
    let created = model::created_response(
        &args["action"],
        &args["target"],
        &document::read(&attempt.join("api-response.json"))?,
    )?;
    if model::classify(&args["action"], &args["observed"], &args["target"])? != "after"
        || model::image(&args["action"], &args["observed"], &args["target"])?["department"]
            != created
    {
        return Err("已有部门成功响应与当前完整后像矛盾".into());
    }
    Ok(json!({"observed":observed,"response_proven":true,"reconcile":null,"state":"after"}))
}

fn confirm(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target", "attempt", "observed", "proof"])?;
    let attempt = path(args, "attempt")?;
    history::attempt_path(backend, run, &args["action"], &attempt)?;
    let proof = &args["proof"];
    document::exact(
        proof,
        &["observed", "response_proven", "reconcile", "state"],
        "部门写入后像证明",
    )?;
    if proof["state"] != "after"
        || model::classify(&args["action"], &args["observed"], &args["target"])? != "after"
        || common::read_bound(backend, &proof["observed"], "部门观察")? != args["observed"]
    {
        return Err("部门完整后像不符".into());
    }
    let mut evidence = json!({"intent":common::binding(&attempt.join("intent.json"))?,
        "before":common::binding(&attempt.join("before.json"))?,"observed":proof["observed"]});
    if document::boolean(&proof["response_proven"], "部门响应证明")? {
        evidence["api-response"] = common::binding(&attempt.join("api-response.json"))?;
    } else {
        evidence["reconcile"] = proof["reconcile"].clone();
    }
    document::write_new(
        &attempt.join("confirmed.json"),
        &json!({"plan":common::binding(&run.join("seed-runtime/department-plan.json"))?,
        "target":args["action"]["target"],"observed":args["observed"],"post_attempted":true,
        "api_response_proven":proof["response_proven"],"evidence":evidence}),
    )?;
    history::confirmation(backend, run, &args["action"], &args["target"], &attempt)
}

fn verify(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["action", "target", "observed", "output"])?;
    let (_, attempts) = directories(backend, run, &args["action"])?;
    let attempt = attempts.last().ok_or("部门尚未覆盖全部普通租户")?;
    history::confirmation(backend, run, &args["action"], &args["target"], attempt)?;
    if model::classify(&args["action"], &args["observed"], &args["target"])? != "after" {
        return Err("部门最终只读验证发现完整后像变化".into());
    }
    let slot = document::text(&args["action"]["target"]["slot"], "部门槽")?;
    let file = stage_output(backend, run, args)?.join(format!("{slot}-observed.json"));
    document::write_new(&file, &args["observed"])?;
    Ok(
        json!({"confirmed":common::binding(&attempt.join("confirmed.json"))?,"observed":common::binding(&file)?}),
    )
}

fn result(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    model::exact_args(args, &["plan", "targets", "mode", "writes", "unresolved"])?;
    let mut confirmed = vec![];
    for (action, target) in document::array(&args["plan"]["actions"], "部门动作")?
        .iter()
        .zip(document::array(&args["targets"], "部门目标")?)
    {
        let (_, attempts) = directories(backend, run, action)?;
        if let Some(last) = attempts
            .last()
            .filter(|p| p.join("confirmed.json").is_file())
        {
            history::confirmation(backend, run, action, target, last)?;
            confirmed.push(common::binding(&last.join("confirmed.json"))?);
        }
    }
    let status = match args["mode"].as_str() {
        Some("departments-apply") => "seed_departments_applied",
        Some("departments-reconcile") => "seed_departments_reconciled",
        _ => return Err("未知部门执行阶段".into()),
    };
    Ok(
        json!({"status":status,"plan":common::binding(&run.join("seed-runtime/department-plan.json"))?,
        "confirmed":confirmed,"unresolved":args["unresolved"],"remote_writes":args["writes"],"worker_authorized":false,"restore_qualified":false}),
    )
}
