//! 部门阶段使用同一已发布 API 后继的执行器，保留原 seed 和历史生产者登记。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{
        json as document,
        lifecycle::{identity, manifest, state},
        post_process::{
            api_successor,
            department_api::{self, Mode},
        },
        resources::common,
    },
};

#[path = "department_successor/correction.rs"]
mod correction;

pub(crate) use correction::{path as correction_path, runtime_path as runtime_correction_path};

pub(crate) fn path(run: &Path) -> PathBuf {
    run.join("seed-runtime")
        .join("department-plan-successor.json")
}

pub(crate) fn launch_path(backend: &Path, run: &Path, number: u64) -> Result<Option<PathBuf>> {
    let saved = state::load(run, false)?;
    let attempts = state::attempts(&saved)?;
    let attempt = stage_attempt(attempts, number)?;
    if department_api::mode(attempt).is_none() || predecessor(attempts, number)?.is_none() {
        return Ok(None);
    }
    for filename in [
        runtime_correction_path(run),
        correction_path(run),
        path(run),
    ] {
        if let Some((_, receipt)) = existing(backend, &filename)?
            && number_field(&receipt, "first_attempt")? <= number
        {
            return Ok(Some(filename));
        }
    }
    Ok(None)
}

pub(crate) fn select(
    backend: &Path,
    run: &Path,
    number: u64,
    original_descriptor: &Value,
    producer_build: Option<&Path>,
) -> Result<Option<(Value, Value)>> {
    let Some(current) = department_api::publication_for_stage(backend, run, number)? else {
        if producer_build.is_some() {
            return Err("部门修正构建需要已登记的在用 API 后继".into());
        }
        require_original(run, Some(number), false)?;
        return Ok(None);
    };
    let saved = state::load(run, false)?;
    let attempts = state::attempts(&saved)?;
    let mode = stage_mode(attempts, number)?;
    let original = original(backend, run, original_descriptor)?;
    let api = predecessor(attempts, number)?.ok_or("部门规划缺少通过的 API 后继")?;
    let registered = RegisteredPlan {
        descriptor: original_descriptor,
        request: &original,
    };
    let correction = correction::present(&correction_path(run))?;
    let scope = correction::Scope::new(backend, run, attempts, api, &registered, &current);
    let (receipt, binding) = if correction::present(&runtime_correction_path(run))?
        || (correction && producer_build.is_some())
    {
        scope.select(number, producer_build, correction::Generation::Runtime)?
    } else if producer_build.is_some() || correction {
        scope.select(number, producer_build, correction::Generation::Bridge)?
    } else {
        select_initial(backend, run, attempts, number, api, &registered, &current)?
    };
    let first = number_field(&receipt, "first_attempt")?;
    if first > number {
        return Err("不能将后继的部门规划工具用于原历史阶段".into());
    }
    let projected = project(original, &receipt);
    if mode != Mode::Plan {
        department_api::require_passed_plan(
            backend, run, attempts, number, first, &binding, &projected,
        )?;
    }
    if state::load(run, false)? != saved {
        return Err("部门工具选择期间正式阶段变化".into());
    }
    Ok(Some((projected, binding)))
}

fn existing(backend: &Path, filename: &Path) -> Result<Option<(Value, Value)>> {
    match fs::symlink_metadata(filename) {
        Ok(_) => {
            let binding = common::binding(filename)?;
            let receipt = common::read_bound(backend, &binding, "部门规划工具登记")?;
            Ok(Some((binding, receipt)))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn select_initial(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    number: u64,
    api: &Value,
    original: &RegisteredPlan<'_>,
    current: &Value,
) -> Result<(Value, Value)> {
    let filename = path(run);
    let (first, exists) = match existing(backend, &filename)? {
        Some((_, receipt)) => (number_field(&receipt, "first_attempt")?, true),
        None if is_plan(stage_attempt(attempts, number)?) => (number, false),
        None => {
            return Err("部门阶段尚未登记并通过后继部门规划".into());
        }
    };
    if first > number {
        return Err("不能将后继的部门规划工具用于原历史阶段".into());
    }
    let receipt = build(backend, run, attempts, first, api, original, current)?;
    if !exists {
        document::publish_new(&filename, &receipt, "部门规划后继工具")?;
    }
    let binding = common::binding(&filename)?;
    if common::read_bound(backend, &binding, "部门规划后继工具")? != receipt {
        return Err("部门规划后继工具已变化，保留原登记".into());
    }
    Ok((receipt, binding))
}

pub(crate) fn resolve(
    backend: &Path,
    run: &Path,
    descriptor: &Value,
    number: Option<u64>,
    cleanup: bool,
) -> Result<Value> {
    let number = number.ok_or("部门后继登记缺少正式阶段编号")?;
    let filename = common::bound_file(backend, descriptor, "部门规划后继登记")?;
    if launch_path(backend, run, number)?.as_deref() != Some(filename.as_path()) {
        return Err("部门规划后继登记不属于本次实际阶段".into());
    }
    let saved = state::load(run, false)?;
    let attempts = state::attempts(&saved)?;
    let attempt = stage_attempt(attempts, number)?;
    let mode = department_api::mode(attempt).ok_or("部门工具后继只用于正式部门阶段")?;
    if !cleanup && attempt["status"] != "running" {
        return Err("部门工具只用于正在运行的正式部门阶段".into());
    }
    let receipt = common::read_bound(backend, descriptor, "部门规划后继登记")?;
    let first = number_field(&receipt, "first_attempt")?;
    let api = predecessor(attempts, number)?.ok_or("历史规划未使用已通过的 API 后继")?;
    if first > number {
        return Err("不能将后继的部门规划工具用于原历史阶段".into());
    }
    let original_descriptor = &receipt["original_registration"];
    let original = original(backend, run, original_descriptor)?;
    let publication = api_successor::load(backend, run, &original["post_copy"])?
        .ok_or("缺少原部门规划实际使用的 API 后继发布")?;
    let registered = RegisteredPlan {
        descriptor: original_descriptor,
        request: &original,
    };
    let scope = correction::Scope::new(backend, run, attempts, api, &registered, &publication);
    if filename == runtime_correction_path(run) {
        scope.validate(&receipt, correction::Generation::Runtime)?;
    } else if filename == correction_path(run) {
        if !cleanup {
            scope.require_launch_generation(correction::Generation::Bridge, first, number)?;
        }
        scope.validate(&receipt, correction::Generation::Bridge)?;
    } else if build(
        backend,
        run,
        attempts,
        first,
        api,
        &registered,
        &publication,
    )? != receipt
    {
        return Err("部门规划后继与正式发布或绑定历史不匹配".into());
    }
    let projected = project(original, &receipt);
    if mode != Mode::Plan {
        department_api::require_passed_plan(
            backend, run, attempts, number, first, descriptor, &projected,
        )?;
    }
    if common::binding(&filename)? != *descriptor || state::load(run, false)? != saved {
        return Err("部门工具授权期间原登记或阶段变化".into());
    }
    Ok(projected)
}

pub(crate) fn require_original(run: &Path, number: Option<u64>, cleanup: bool) -> Result<()> {
    let Some(number) = number else {
        return Ok(());
    };
    let saved = state::load(run, false)?;
    let attempts = state::attempts(&saved)?;
    if attempts
        .iter()
        .any(|row| row["number"] == number && department_api::mode(row).is_some())
        && predecessor(attempts, number)?.is_some()
    {
        if cleanup {
            let launch = output(run, number).join("session-launch.json");
            match fs::symlink_metadata(&launch) {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
        }
        return Err("已接续 API 的部门阶段必须使用其正式后继工具登记".into());
    }
    Ok(())
}

fn is_plan(row: &Value) -> bool {
    department_api::mode(row) == Some(Mode::Plan)
}

fn stage_attempt(attempts: &[Value], number: u64) -> Result<&Value> {
    attempts
        .iter()
        .find(|row| row["number"] == number)
        .ok_or_else(|| "部门工具后继缺少正式阶段".into())
}

fn stage_mode(attempts: &[Value], number: u64) -> Result<Mode> {
    department_api::mode(stage_attempt(attempts, number)?)
        .ok_or_else(|| "部门工具后继只用于正式部门阶段".into())
}

fn output(run: &Path, number: u64) -> PathBuf {
    run.join("seed-runtime")
        .join(format!("attempt-{number:04}"))
}

fn predecessor(attempts: &[Value], number: u64) -> Result<Option<&Value>> {
    let mut rows = attempts.iter().filter(|row| {
        row["stage"] == "runtime-target"
            && matches!(row["mode"].as_str(), Some("restart-api" | "reconcile-api"))
            && row["status"] == "passed"
            && row["number"].as_u64().is_some_and(|prior| prior < number)
    });
    let row = rows.next();
    if rows.next().is_some() {
        return Err("部门规划缺少唯一已通过的 API 后继".into());
    }
    Ok(row)
}

fn original(backend: &Path, run: &Path, descriptor: &Value) -> Result<Value> {
    let original = run.join("seed-runtime.json");
    if common::bound_file(backend, descriptor, "部门规划原 seed 登记")? != original {
        return Err("部门规划没有绑定本运行的原 seed 登记".into());
    }
    let request = common::read_bound(backend, descriptor, "部门规划原 seed 登记")?;
    if request["kind"] != "devex-clone-seed-runtime"
        || request["format_version"] != 1
        || request["run_manifest"] != state::manifest_binding(run)?
    {
        return Err("部门规划原 seed 登记不同于本运行".into());
    }
    common::bound_file(backend, &request["identity_plan"], "部门规划原身份计划")?;
    Ok(request)
}

fn project(mut original: Value, receipt: &Value) -> Value {
    original["identity_tool"] = receipt["tool"].clone();
    original
}

struct RegisteredPlan<'a> {
    descriptor: &'a Value,
    request: &'a Value,
}

fn build(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    first: u64,
    api: &Value,
    original: &RegisteredPlan<'_>,
    publication: &Value,
) -> Result<Value> {
    let api_number = number_field(api, "number")?;
    if first <= api_number
        || !attempts
            .iter()
            .any(|row| row["number"] == first && is_plan(row))
        || publication["registration"] != original.request["post_copy"]
    {
        return Err("部门规划后继未绑定原 seed、身份计划和 API 发布顺序".into());
    }
    let outer = common::read_bound(backend, &api["result"], "部门规划之前的 API 后继结果")?;
    let binding = &outer["publication"];
    if outer["status"] != "api_successor_ready"
        || outer["restore_qualified"] != false
        || common::bound_file(backend, binding, "原 API 后继发布")?
            != run
                .join("post-copy")
                .join("api-successors")
                .join("published.json")
        || common::read_bound(backend, binding, "原 API 后继发布")? != *publication
    {
        return Err("部门规划工具没有绑定同一 API 后继正式发布结果".into());
    }
    let proof = historical(backend, run, attempts, api_number, original)?;
    let tool = executor(backend, publication)?;
    if tool["sha256"] == original.request["identity_tool"]["sha256"] {
        return Err("API 后继执行器仍是原失败的部门规划工具".into());
    }
    Ok(
        json!({"format_version":1,"kind":"devex-seed-department-plan-successor",
        "run_manifest":state::manifest_binding(run)?,"first_attempt":first,
        "original_registration":original.descriptor,"identity_plan":original.request["identity_plan"],
        "api_publication":binding,"historical":proof,"tool":tool}),
    )
}

fn executor(backend: &Path, publication: &Value) -> Result<Value> {
    let intent = common::read_bound(backend, &publication["intent"], "部门规划原 API 启动意图")?;
    let binding = &intent["executor"];
    manifest::validate_binding_shape(binding, "部门规划后继执行器")?;
    let path = Path::new(document::text(&binding["path"], "部门规划后继执行器路径")?);
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    if common::binding(path)? != *binding {
        return Err("部门规划后继执行器不同于原启动意图和归档".into());
    }
    Ok(json!({"path":binding["path"],"sha256":binding["sha256"]}))
}

fn historical(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    api: u64,
    original: &RegisteredPlan<'_>,
) -> Result<Value> {
    let mut seed = attempts.iter().rev().filter(|row| {
        row["stage"] == "seed-runtime" && row["number"].as_u64().is_some_and(|n| n < api)
    });
    let recovery = seed.next().ok_or("部门规划缺少原会话的零写入回收")?;
    let failed = seed.next().ok_or("部门规划缺少已失败的原生产者")?;
    let number = number_field(failed, "number")?;
    if !is_plan(failed)
        || failed["status"] != "failed"
        || failed["error_type"] != "PostCopyCleanupRejected"
        || !failed["result"].is_null()
        || !failed["finished_at"].is_string()
        || recovery["mode"] != "recover-session"
        || recovery["status"] != "passed"
        || number.checked_add(1) != recovery["number"].as_u64()
    {
        return Err("原部门失败及会话回收未形成连续的正式历史".into());
    }
    let controller = controller(backend, run, failed)?;
    let failure = common::binding(&run.join(format!("failure-{number:04}.json")))?;
    let error = common::read_bound(backend, &failure, "原部门规划失败")?;
    if error
        != json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
        "stage":"seed-runtime","mode":"departments-plan","error_type":"PostCopyCleanupRejected",
        "operation_id":"clone.seed-runtime.departments-plan","controller":controller})
    {
        return Err("原部门规划失败没有绑定原控制器和操作".into());
    }
    let output = run
        .join("seed-runtime")
        .join(format!("attempt-{number:04}"));
    let launch = common::binding(&output.join("session-launch.json"))?;
    let process = common::binding(&output.join("session-process.json"))?;
    let bridge = common::binding(&output.join("http-failure-1.json"))?;
    old_producer(
        backend,
        run,
        number,
        &controller,
        &launch,
        &process,
        original,
    )?;
    if common::read_bound(backend, &bridge, "原部门桥接失败")?
        != json!({"id":1,"ok":false,"status":null,"error_type":"NativeBridgeFailure"})
    {
        return Err("原部门规划没有绑定发送业务请求前的 NativeBridge 失败".into());
    }
    let recovered = common::read_bound(backend, &recovery["result"], "原部门零写入会话回收")?;
    if recovered
        != json!({"status":"seed_session_recovered","producer":process,
        "already_stopped":true,"copy_requires_reconciliation":true,"remote_writes":0,"restore_qualified":false})
    {
        return Err("原会话回收没有证明该部门生产者已停和零远端写入".into());
    }
    Ok(
        json!({"failed_attempt":number,"controller":controller,"failure":failure,
        "launch":launch,"producer":process,"bridge_failure":bridge,"recovery":recovery["result"]}),
    )
}

fn controller(backend: &Path, run: &Path, row: &Value) -> Result<Value> {
    let (binding, _) = controller_owner(backend, run, row)?;
    Ok(binding)
}

fn controller_owner(backend: &Path, run: &Path, row: &Value) -> Result<(Value, Value)> {
    let mut running = row.clone();
    running["status"] = json!("running");
    for field in ["finished_at", "result", "error_type"] {
        running[field] = Value::Null;
    }
    let (binding, owner) = state::controller_record(run, number_field(row, "number")?, &running)?;
    identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"] {
        return Err("原部门失败控制器属于其他运行".into());
    }
    common::bound_file(backend, &binding, "原部门规划控制器")?;
    Ok((binding, owner))
}

fn old_producer(
    backend: &Path,
    run: &Path,
    number: u64,
    controller: &Value,
    launch: &Value,
    process: &Value,
    original: &RegisteredPlan<'_>,
) -> Result<()> {
    let launched = common::read_bound(backend, launch, "原部门规划启动")?;
    document::exact(
        &launched,
        &[
            "format_version",
            "kind",
            "run_manifest",
            "registration",
            "controller",
            "attempt",
            "producer_kind",
            "command",
            "tool",
            "source",
        ],
        "原部门规划启动",
    )?;
    let recorded = common::read_bound(backend, process, "原部门规划生产者")?;
    document::exact(
        &recorded,
        &[
            "format_version",
            "kind",
            "launch",
            "identity",
            "producer_owner",
        ],
        "原部门规划生产者",
    )?;
    let identity = common::validate_process_identity(&recorded["identity"])?;
    if launched["format_version"] != 1
        || launched["kind"] != "devex-post-producer-launch"
        || launched["run_manifest"] != state::manifest_binding(run)?
        || launched["registration"] != *original.descriptor
        || launched["controller"] != *controller
        || launched["attempt"] != number
        || launched["producer_kind"] != "department-plan"
        || launched["tool"] != original.request["identity_tool"]
        || recorded["kind"] != "devex-post-producer"
        || recorded["format_version"] != 1
        || recorded["launch"] != *launch
        || identity["executable"] != launched["tool"]["path"]
    {
        return Err("原部门会话不属于原 seed 工具和正式规划启动".into());
    }
    Ok(())
}

fn number_field(value: &Value, name: &str) -> Result<u64> {
    value[name]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "部门规划缺少有效阶段编号".into())
}

#[cfg(test)]
#[test]
fn only_later_department_plan_selects_the_published_api_tool() -> Result<()> {
    let attempts = [
        json!({"number":51,"stage":"seed-runtime","mode":"departments-plan","status":"failed"}),
        json!({"number":53,"stage":"runtime-target","mode":"restart-api","status":"failed"}),
        json!({"number":54,"stage":"runtime-target","mode":"reconcile-api","status":"passed"}),
        json!({"number":55,"stage":"seed-runtime","mode":"departments-apply","status":"running"}),
        json!({"number":56,"stage":"seed-runtime","mode":"departments-plan","status":"running"}),
    ];
    assert!(predecessor(&attempts, 51)?.is_none());
    assert!(predecessor(&attempts, 54)?.is_none());
    assert_eq!(predecessor(&attempts, 56)?, Some(&attempts[2]));
    assert!(!is_plan(&attempts[3]));
    assert!(is_plan(&attempts[4]));
    let mut ambiguous = attempts.to_vec();
    ambiguous[1]["status"] = json!("passed");
    assert!(predecessor(&ambiguous, 56).is_err());
    Ok(())
}
