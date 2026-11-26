//! 部门阶段必须验证已登记的运行后继，同时继续使用原身份计划及配置。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    super::{
        cache::handoff::registration,
        digest, json as document,
        lifecycle::{identity, state},
        resources::common,
        seed::history,
        target_verify::{authority::require_closed, storage::protocol},
    },
    api_successor, command, runtime,
};
use crate::{
    Result,
    recovery::full_stack::worker::{controller, role::Role},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::recovery::clone::native) enum Mode {
    Plan,
    Apply,
    Reconcile,
    Verify,
}

pub(in crate::recovery::clone::native) fn mode(row: &Value) -> Option<Mode> {
    if row["stage"] != "seed-runtime" {
        return None;
    }
    match row["mode"].as_str()? {
        "departments-plan" => Some(Mode::Plan),
        "departments-apply" => Some(Mode::Apply),
        "departments-reconcile" => Some(Mode::Reconcile),
        "departments-verify" => Some(Mode::Verify),
        _ => None,
    }
}

pub(in crate::recovery::clone::native) fn publication_for_stage(
    backend: &Path,
    run: &Path,
    attempt: u64,
) -> Result<Option<Value>> {
    controlled_publication(backend, run, attempt, ApiConsumer::Department)
}

pub(in crate::recovery::clone::native) fn publication_for_identity_verification(
    backend: &Path,
    run: &Path,
    attempt: u64,
) -> Result<Option<Value>> {
    controlled_publication(backend, run, attempt, ApiConsumer::IdentityVerification)
}

#[derive(Clone, Copy)]
enum ApiConsumer {
    Department,
    IdentityVerification,
}

impl ApiConsumer {
    fn matches(self, row: &Value) -> bool {
        match self {
            Self::Department => mode(row).is_some(),
            Self::IdentityVerification => {
                row["stage"] == "seed-runtime" && row["mode"] == "identities-verify"
            }
        }
    }
}

fn controlled_publication(
    backend: &Path,
    run: &Path,
    attempt: u64,
    consumer: ApiConsumer,
) -> Result<Option<Value>> {
    let saved = state::load(run, true)?;
    let row = state::running_attempt(&saved)?.ok_or("后继 API 核验缺少受管运行阶段")?;
    if row["number"].as_u64() != Some(attempt) || !consumer.matches(row) {
        return Err("后继 API 证明与授权的当前阶段不同".into());
    }
    let (_, owner) = state::controller_record(run, attempt, row)?;
    let path = run.join("run.lock").join("owner.json");
    if document::read(&path)? != owner {
        return Err("部门规划控制器与原持锁所有者不同".into());
    }
    identity::validate(&owner, run)?;
    let pid = u32::try_from(
        owner["identity"]["pid"]
            .as_u64()
            .ok_or("部门控制器缺少 PID")?,
    )?;
    if common::process_identity(pid)?.as_ref() != Some(&owner["identity"]) {
        return Err("部门规划原控制器已退出或创建身份变化".into());
    }
    let post = history::post(backend, run, None, None, false)?;
    let publication = api_successor::load(backend, run, &post.descriptor)?;
    if state::load(run, true)? != saved || document::read(&path)? != owner {
        return Err("部门规划控制权在读取 API 后继期间变化".into());
    }
    Ok(publication)
}

pub(crate) fn department_api_proof(
    backend: &Path,
    run: &Path,
    attempt: u64,
    environment: &Value,
) -> Result<Option<Value>> {
    controlled_api_proof(backend, run, attempt, environment, ApiConsumer::Department)
}

pub(crate) fn identity_verification_api_proof(
    backend: &Path,
    run: &Path,
    attempt: u64,
    environment: &Value,
) -> Result<Option<Value>> {
    controlled_api_proof(
        backend,
        run,
        attempt,
        environment,
        ApiConsumer::IdentityVerification,
    )
}

fn controlled_api_proof(
    backend: &Path,
    run: &Path,
    attempt: u64,
    environment: &Value,
    consumer: ApiConsumer,
) -> Result<Option<Value>> {
    let Some(publication) = controlled_publication(backend, run, attempt, consumer)? else {
        return Ok(None);
    };
    let post = history::post(
        backend,
        run,
        Some(&publication["registration"]),
        None,
        false,
    )?;
    let handoff = document::read(&post.runtime.join("handoff.json"))?;
    let frozen_path = post.runtime.join("runtime.json");
    let frozen = registration::cleanup_runtime(backend, run, &post)?;
    let ready = document::text(&handoff["api_url"], "原部门 API 端点")?;
    if !matches_original_environment(backend, &post.runtime, &frozen, environment, ready)
        || handoff["registration"] != post.descriptor
        || handoff["runtime"] != common::binding(&frozen_path)?
    {
        return Err("部门计划未绑定原 API 运行、范围及配置".into());
    }
    let directory = Path::new(document::text(
        &publication["runtime_directory"],
        "部门使用的 API 后继运行",
    )?);
    let proof = runtime::proof(&json!({
        "backend":backend,"directory":directory,"scope_id":frozen["scope_id"],"api_url":ready,
    }))?;
    let api = common::read_bound(backend, &publication["api"], "后继 API 固定收据")?;
    if proof["runtime"] != frozen || proof["api_process"] != api["identity"] {
        return Err("部门规划的实际 API 不属于原运行后继".into());
    }
    let product = runtime::product_root(backend, &frozen)?;
    check_live(backend, &product, directory, &publication, &proof)?;
    require_closed(document::text(
        &frozen["worker_ready_url"],
        "原 Worker 地址",
    )?)?;
    require_closed(document::text(&environment["frontend_url"], "原前端地址")?)?;
    if controlled_publication(backend, run, attempt, consumer)?.as_ref() != Some(&publication) {
        return Err("部门计划实际来源在运行核对期间变化".into());
    }
    Ok(Some(proof))
}

fn matches_original_environment(
    backend: &Path,
    original_runtime: &Path,
    frozen: &Value,
    environment: &Value,
    api_url: &str,
) -> bool {
    // 身份计划绑定协调源码，产品根独立来自已冻结运行合同。
    environment["runtime"]["directory"] == json!(original_runtime)
        && environment["backend_dir"] == json!(backend)
        && environment["scope_id"] == frozen["scope_id"]
        && environment["api_url"] == api_url
}

fn check_live(
    backend: &Path,
    product: &Path,
    directory: &Path,
    publication: &Value,
    proof: &Value,
) -> Result<()> {
    let live = controller::observe(Role::Api, product, directory)?;
    if live["state"] != "running" || live["identity"] != proof["api_process"] {
        return Err("部门计划的 API 监听或监督进程不属于发布的后继".into());
    }
    let actual = protocol::arguments(backend, directory, &publication["api"], &live["identity"])?;
    let argv = json!([
        proof["runtime"]["artifacts"]["api"]["path"],
        "--config-dir",
        proof["runtime"]["api_config_dir"]
    ]);
    if actual["actual_arguments_sha256"] != digest::plan_hash(&argv)? {
        return Err("部门计划的 API 启动参数不同于原冻结产物和私有配置".into());
    }
    Ok(())
}

pub(in crate::recovery::clone::native) fn require_passed_plan(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    number: u64,
    first: u64,
    registration: &Value,
    request: &Value,
) -> Result<()> {
    let latest = latest_plan(attempts, number, first)?;
    let result = common::read_bound(backend, &latest["result"], "最新部门规划通过结果")?;
    let path = run.join("seed-runtime").join("department-plan.json");
    let plan = read_at(backend, &result["plan"], &path)?;
    let mut fields = vec![
        "status",
        "plan",
        "prerequisite",
        "remote_writes",
        "worker_authorized",
        "restore_qualified",
    ];
    if result.get("adopted").is_some() {
        fields.push("adopted");
    }
    document::exact(&result, &fields, "最新部门规划通过结果")?;
    let mut payload = plan.clone();
    let hash = payload
        .as_object_mut()
        .and_then(|fields| fields.remove("plan_sha256"))
        .ok_or("部门计划缺少规范摘要")?;
    if result["status"] != "seed_departments_planned"
        || result["remote_writes"] != 0
        || result["worker_authorized"] != false
        || result["restore_qualified"] != false
        || plan["kind"] != "devex-seed-departments"
        || plan["format_version"] != 1
        || plan["registration"] != common::binding(&run.join("seed-runtime.json"))?
        || plan["identity_plan"] != request["identity_plan"]
        || plan["prerequisite"] != result["prerequisite"]
        || hash != digest::plan_hash(&payload)?
    {
        return Err("最新的部门规划结果未绑定原 seed 和固定身份前提".into());
    }
    read_at(
        backend,
        &plan["prerequisite"],
        &run.join("seed-runtime")
            .join("departments")
            .join("identity-prerequisite.json"),
    )?;
    let origin = if let Some(adopted) = result.get("adopted") {
        adopted_origin(backend, run, attempts, first, latest, &result, adopted)?
    } else {
        latest
    };
    let output = plan_output(run, number_of(origin)?);
    let mut before = Vec::new();
    for action in document::array(&plan["actions"], "部门规划动作")? {
        let evidence = &action["before_evidence"];
        let slot = document::text(&action["target"]["slot"], "部门规划原租户槽")?;
        let evidence_path = output.join(format!("{slot}-before.json"));
        if common::bound_file(backend, evidence, "部门规划原始前像")? != evidence_path {
            return Err("部门规划含其他生产者阶段的前像".into());
        }
        before.push(evidence);
    }
    if result
        .get("adopted")
        .is_some_and(|adopted| adopted["before"] != json!(before))
    {
        return Err("部门规划采用没有保留同一生产者的前像".into());
    }
    require_plan_producer(backend, run, origin, &output, registration, request)?;
    common::bound_file(backend, &latest["result"], "部门规划发布结果后像")?;
    common::bound_file(backend, &result["plan"], "部门规划原件后像")?;
    Ok(())
}

fn latest_plan(attempts: &[Value], current: u64, first: u64) -> Result<&Value> {
    attempts
        .iter()
        .rev()
        .find(|row| {
            mode(row) == Some(Mode::Plan) && row["number"].as_u64().is_some_and(|n| n < current)
        })
        .filter(|row| {
            row["status"] == "passed" && row["number"].as_u64().is_some_and(|n| n >= first)
        })
        .ok_or_else(|| "最新部门规划尚未由后继工具正式通过".into())
}

fn adopted_origin<'a>(
    backend: &Path,
    run: &Path,
    attempts: &'a [Value],
    first: u64,
    latest: &Value,
    result: &Value,
    adopted: &Value,
) -> Result<&'a Value> {
    document::exact(
        adopted,
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
        "通过的部门规划采用",
    )?;
    let origin = attempts
        .iter()
        .find(|row| row["number"] == adopted["attempt"])
        .ok_or("部门规划采用缺少原生产者阶段")?;
    let number = number_of(origin)?;
    let latest_number = number_of(latest)?;
    let controller = plan_controller(run, origin)?;
    let output = plan_output(run, number);
    if mode(origin) != Some(Mode::Plan)
        || origin["status"] != "failed"
        || number < first
        || number >= latest_number
        || adopted["plan"] != result["plan"]
        || adopted["prerequisite"] != result["prerequisite"]
        || adopted["controller"] != controller
        || adopted["current_controller"] != plan_controller(run, latest)?
        || adopted["producer"] != common::binding(&output.join("session-process.json"))?
    {
        return Err("部门规划采用未绑定同一后继执行器的原失败阶段".into());
    }
    let failure = read_at(
        backend,
        &adopted["failure"],
        &run.join(format!("failure-{number:04}.json")),
    )?;
    if failure
        != json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
        "stage":"seed-runtime","mode":"departments-plan","error_type":origin["error_type"],
        "operation_id":"clone.seed-runtime.departments-plan","controller":controller})
    {
        return Err("部门规划采用的原失败收据已变化".into());
    }
    let guard = plan_output(run, latest_number).join("adoption-guard.json");
    let guard = read_at(backend, &adopted["guard"], &guard)?;
    document::exact(
        &guard,
        &["format_version", "kind", "attempt", "artifacts"],
        "部门计划采用守卫",
    )?;
    if guard["format_version"] != 1
        || guard["kind"] != "devex-department-adoption-guard"
        || guard["attempt"] != latest_number
        || !guard["artifacts"].is_array()
    {
        return Err("部门规划采用的只读守卫不属于通过的计划阶段".into());
    }
    read_at(
        backend,
        &adopted["authentication"],
        &output.join("authentication.json"),
    )?;
    Ok(origin)
}

fn require_plan_producer(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    output: &Path,
    registration: &Value,
    request: &Value,
) -> Result<()> {
    let number = number_of(attempt)?;
    let launch = common::binding(&output.join("session-launch.json"))?;
    let launch_receipt = common::read_bound(backend, &launch, "部门规划原启动")?;
    let expected = command::build(&json!({"backend":backend,"run_dir":run,"attempt":number,
        "kind":"department-plan","output":output,"tool":request["identity_tool"],
        "registration":request,"configuration":null}))?;
    let process = common::binding(&output.join("session-process.json"))?;
    let producer = common::read_bound(backend, &process, "部门规划原生产者")?;
    document::exact(
        &producer,
        &[
            "format_version",
            "kind",
            "launch",
            "identity",
            "producer_owner",
        ],
        "部门规划原生产者",
    )?;
    let known_identity = common::validate_process_identity(&producer["identity"])?;
    if launch_receipt
        != json!({"format_version":1,"kind":"devex-post-producer-launch",
        "run_manifest":state::manifest_binding(run)?,"registration":registration,
        "controller":plan_controller(run, attempt)?,"attempt":number,"producer_kind":"department-plan",
        "command":expected["argv"],"tool":expected["tool"],"source":expected["source"]})
        || producer["format_version"] != 1
        || producer["kind"] != "devex-post-producer"
        || producer["launch"] != launch
        || known_identity["executable"] != request["identity_tool"]["path"]
        || !producer["producer_owner"]
            .as_str()
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .is_some_and(|value| value.get_version_num() == 4)
    {
        return Err("已通过的部门规划由其他登记、工具或生产者生成".into());
    }
    Ok(())
}

fn plan_controller(run: &Path, attempt: &Value) -> Result<Value> {
    let mut running = attempt.clone();
    running["status"] = json!("running");
    for field in ["finished_at", "error_type", "result"] {
        running[field] = Value::Null;
    }
    let (binding, owner) = state::controller_record(run, number_of(attempt)?, &running)?;
    identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"] {
        return Err("原部门规划控制器属于其他运行".into());
    }
    Ok(binding)
}

fn plan_output(run: &Path, number: u64) -> PathBuf {
    run.join("seed-runtime")
        .join(format!("attempt-{number:04}"))
}

fn read_at(backend: &Path, binding: &Value, path: &Path) -> Result<Value> {
    if common::bound_file(backend, binding, "部门规划阶段固定证据")? != path {
        return Err("部门规划阶段证据不同于固定路径".into());
    }
    common::read_bound(backend, binding, "部门规划阶段固定证据")
}

fn number_of(attempt: &Value) -> Result<u64> {
    attempt["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "部门规划阶段缺少有效序号".into())
}

#[cfg(test)]
#[test]
fn original_identity_environment_keeps_distinct_coordinator_and_product_roots() {
    let coordinator = Path::new("/recovery/coordinator");
    let original = Path::new("/recovery/original-runtime");
    let product = Path::new("/recovery/product");
    let url = "http://127.0.0.1:24320";
    let frozen = json!({"backend_root":product,"scope_id":"same-scope"});
    let mut environment = json!({"backend_dir":coordinator,"scope_id":"same-scope","api_url":url,
        "runtime":{"directory":original}});
    assert!(matches_original_environment(
        coordinator,
        original,
        &frozen,
        &environment,
        url
    ));
    environment["backend_dir"] = json!(product);
    assert!(!matches_original_environment(
        coordinator,
        original,
        &frozen,
        &environment,
        url
    ));
}

#[cfg(test)]
#[test]
fn latest_department_plan_must_pass_and_belong_to_the_registered_tool_window() -> Result<()> {
    let rows = [
        json!({"number":51,"stage":"seed-runtime","mode":"departments-plan","status":"passed"}),
        json!({"number":57,"stage":"seed-runtime","mode":"departments-plan","status":"passed"}),
        json!({"number":59,"stage":"seed-runtime","mode":"departments-plan","status":"failed"}),
        json!({"number":60,"stage":"seed-runtime","mode":"quotas-plan","status":"passed"}),
    ];
    assert!(latest_plan(&rows, 57, 57).is_err());
    assert_eq!(latest_plan(&rows, 58, 57)?, &rows[1]);
    assert!(latest_plan(&rows, 61, 57).is_err());
    assert_eq!(mode(&rows[3]), None);
    assert_eq!(
        mode(&json!({"stage":"runtime-target","mode":"departments-verify"})),
        None
    );
    Ok(())
}
