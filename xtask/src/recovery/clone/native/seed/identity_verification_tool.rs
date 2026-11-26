//! 身份核验独立登记执行器，并保留原和解、准备账本及后续 API 来源。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{continuation_tool, identity_successor};
use crate::{
    Result,
    data::performance_identities::seed_evidence,
    recovery::clone::native::{
        department, json as document,
        lifecycle::{identity, manifest, state},
        post_process::{api_successor, department_api},
        resources::common,
    },
};

const PURPOSE: &str = "identity-verification";
const MODE: &str = "identities-verify";

#[path = "identity_verification_tool/successor.rs"]
mod successor;
pub(crate) use successor::{
    path as successor_path, recovery_path, recovery_retry_path, resolve as resolve_successor,
};

pub(crate) fn path(run: &Path) -> PathBuf {
    run.join("seed-runtime")
        .join("identity-verification-tool.json")
}

pub(crate) fn select(
    backend: &Path,
    run: &Path,
    number: u64,
    request: &Value,
    original: &Value,
    supplied_build: Option<&Path>,
    departments: &Value,
) -> Result<(Value, Value)> {
    let Some(publication) =
        department_api::publication_for_identity_verification(backend, run, number)?
    else {
        if supplied_build.is_some() {
            return Err("身份核验工具只允许绑定已经发布的 API 后继".into());
        }
        return identity_successor::select(backend, run, number, request, original);
    };
    let saved = state::load(run, true)?;
    let attempts = state::attempts(&saved)?;
    let row = verify_attempt(attempts, number)?;
    let (_, owner) = verify_controller(run, row, true)?;
    if owner != identity::current()? {
        return Err("只有当前持锁身份核验控制器能登记工具".into());
    }
    let active = api_successor::select(backend, run, &request["post_copy"])?
        .ok_or("身份核验工具缺少当前已发布 API 后继")?;
    if active.publication != publication || active.passed_attempt >= number {
        return Err("身份核验工具的当前 API 发布或时序已变化".into());
    }
    let publication_path = common::bound_file(backend, &active.binding, "身份核验 API 发布")?;
    if publication_path != first_publication_path(run) {
        return successor::select(
            backend,
            run,
            &saved,
            supplied_build,
            &active,
            successor::Current {
                row,
                owner: &owner,
                original,
                request,
                departments,
            },
        );
    }
    let history = History::load(backend, run, attempts, number, original, &publication)?;
    if history.request != *request {
        return Err("身份核验工具与已确认的原 seed 证据不同".into());
    }
    history.check_departments(departments)?;
    let binding = register_legacy(backend, run, row, &owner, &history, supplied_build)?;
    let selected = resolve(backend, run, &binding, Some(number), false)?;
    if state::load(run, true)? != saved {
        return Err("身份核验工具登记期间正式阶段变化".into());
    }
    Ok((selected, binding))
}

fn register_legacy(
    backend: &Path,
    run: &Path,
    row: &Value,
    owner: &Value,
    history: &History,
    supplied_build: Option<&Path>,
) -> Result<Value> {
    let filename = path(run);
    match fs::symlink_metadata(&filename) {
        Ok(_) => {
            let bound = common::binding(&filename)?;
            if let Some(candidate) = supplied_build {
                let (build, _) = continuation_tool::bind(backend, candidate, PURPOSE, owner)?;
                let old = common::read_bound(backend, &bound, "原身份核验工具登记")?;
                if build != old["build"] {
                    return Err("已有身份核验工具登记只接受同一冻结构建".into());
                }
            }
            Ok(bound)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let build = supplied_build.ok_or("首次身份核验接续需要当前控制器的冻结构建收据")?;
            let (build, tool) = continuation_tool::bind(backend, build, PURPOSE, owner)?;
            history.check_projection(backend, true)?;
            let (controller, _) = verify_controller(run, row, true)?;
            let value = history.record(number_of(&row["number"])?, &controller, &build, &tool)?;
            document::publish_new(&filename, &value, "独立身份核验工具")?;
            common::binding(&filename)
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn resolve(
    backend: &Path,
    run: &Path,
    descriptor: &Value,
    number: Option<u64>,
    cleanup: bool,
) -> Result<Value> {
    let number = number.ok_or("独立身份核验工具缺少正式阶段编号")?;
    let saved = state::load(run, true)?;
    let attempts = state::attempts(&saved)?;
    let row = verify_attempt(attempts, number)?;
    let (_, owner) = verify_controller(run, row, !cleanup)?;
    let legacy = Legacy::load(backend, run, attempts, number, descriptor)?;
    if owner["executable"] != legacy.owner["executable"] {
        return Err("独立身份核验工具不同于原阶段控制器".into());
    }
    if !cleanup {
        legacy.history.check_projection(backend, false)?;
        legacy
            .history
            .check_departments(&department::authorize_identity_verification(
                backend,
                run,
                &legacy.history.request,
                number,
            )?)?;
    }
    if common::binding(&path(run))? != *descriptor || state::load(run, true)? != saved {
        return Err("独立身份核验工具或原阶段在解析期间变化".into());
    }
    let mut selected = legacy.history.request;
    selected["identity_tool"] = legacy.tool;
    Ok(selected)
}

struct Legacy {
    history: History,
    tool: Value,
    owner: Value,
}

impl Legacy {
    fn load(
        backend: &Path,
        run: &Path,
        attempts: &[Value],
        number: u64,
        descriptor: &Value,
    ) -> Result<Self> {
        let value = read_at(backend, descriptor, &path(run))?;
        let first = number_of(&value["first_attempt"])?;
        if first > number {
            return Err("独立身份核验工具不能改写原阶段授权".into());
        }
        let (controller, owner) = verify_controller(run, verify_attempt(attempts, first)?, false)?;
        let original = &value["original_registration"];
        let old = read_at(backend, original, &run.join("seed-runtime.json"))?;
        let api = api_successor::select_before(backend, run, &old["post_copy"], first)?
            .ok_or("原独立身份核验工具缺少首次执行前的 API 后继")?;
        let history = History::load(backend, run, attempts, number, original, &api.publication)?;
        let tool = continuation_tool::verify(backend, &value["build"], PURPOSE, &owner)?;
        if api.binding != history.api_publication
            || history.record(first, &controller, &value["build"], &tool)? != value
        {
            return Err("独立身份核验工具不同于原阶段、构建或不可变登记".into());
        }
        Ok(Self {
            history,
            tool,
            owner,
        })
    }
}

pub(crate) fn require_original(run: &Path, number: Option<u64>, cleanup: bool) -> Result<()> {
    let Some(number) = number else {
        return Ok(());
    };
    let saved = state::load(run, false)?;
    let attempts = state::attempts(&saved)?;
    if attempts
        .iter()
        .any(|row| row["number"] == number && is_verify(row))
        && before(attempts, number).any(is_api_successor)
    {
        if cleanup {
            match fs::symlink_metadata(output(run, number).join("session-launch.json")) {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
        }
        return Err("已发布 API 后继的身份核验需要独立核验工具登记".into());
    }
    Ok(())
}

struct History {
    manifest: Value,
    original: Value,
    request: Value,
    reconciliation: Value,
    preparation: Value,
    prepared: Value,
    previous_tool: Value,
    departments: Value,
    api_publication: Value,
}

impl History {
    fn load(
        backend: &Path,
        run: &Path,
        attempts: &[Value],
        number: u64,
        original: &Value,
        publication: &Value,
    ) -> Result<Self> {
        let manifest = state::manifest_binding(run)?;
        let request = read_at(backend, original, &run.join("seed-runtime.json"))?;
        if request["format_version"] != 1
            || request["kind"] != "devex-clone-seed-runtime"
            || request["run_manifest"] != manifest
            || publication["registration"] != request["post_copy"]
        {
            return Err("独立身份核验工具未绑定固定 seed 登记与 API 后继".into());
        }
        let api = before(attempts, number)
            .find(|row| is_api_successor(row))
            .ok_or("身份核验工具缺少已经通过的 API 后继")?;
        let api_number = number_of(&api["number"])?;
        let api_result = common::read_bound(backend, &api["result"], "原 API 后继结果")?;
        let api_path = first_publication_path(run);
        if api_result["status"] != "api_successor_ready"
            || api_result["restore_qualified"] != false
            || read_at(backend, &api_result["publication"], &api_path)? != *publication
        {
            return Err("身份核验阶段的 API 发布无效".into());
        }
        let prepared = before(attempts, number)
            .find(|row| row["stage"] == "seed-runtime" && row["mode"] == "identities-apply")
            .filter(|row| row["status"] == "passed")
            .ok_or("身份核验缺少最后通过的原身份准备")?;
        let prepared_number = number_of(&prepared["number"])?;
        let reconciled = before(attempts, prepared_number)
            .find(|row| row["stage"] == "seed-runtime" && row["mode"] == identity_successor::MODE)
            .filter(|row| row["status"] == "passed")
            .ok_or("身份核验缺少原身份和解")?;
        if prepared_number >= api_number {
            return Err("身份准备必须先于本次 API 接续".into());
        }
        let reconciliation = common::read_bound(backend, &reconciled["result"], "原身份和解")?;
        let old_tool = identity_successor::resolve(
            backend,
            run,
            &reconciliation["identity_successor"],
            Some(prepared_number),
        )?;
        let prepared_state = prepared_history(
            backend,
            run,
            prepared,
            original,
            &request,
            &reconciliation,
            &old_tool,
        )?;
        let departments = department_history(backend, run, attempts, number, api_number)?;
        Ok(Self {
            manifest,
            original: original.clone(),
            request,
            reconciliation: reconciled["result"].clone(),
            preparation: prepared["result"].clone(),
            prepared: prepared_state,
            previous_tool: old_tool["identity_tool"].clone(),
            departments,
            api_publication: api_result["publication"].clone(),
        })
    }

    fn record(
        &self,
        number: u64,
        controller: &Value,
        build: &Value,
        tool: &Value,
    ) -> Result<Value> {
        if tool["sha256"] == self.previous_tool["sha256"]
            || tool["sha256"] == self.request["identity_tool"]["sha256"]
        {
            return Err("独立身份核验工具必须不同于原应用或和解工具".into());
        }
        Ok(
            json!({"format_version":1,"kind":"devex-seed-identity-verification-tool",
            "run_manifest":self.manifest,"first_attempt":number,"controller":controller,
            "original_registration":self.original,"identity_plan":self.request["identity_plan"],
            "identity_state":self.request["identity_state"],"reconciliation":self.reconciliation,
            "preparation":self.preparation,"prepared_projection_sha256":self.prepared["projection_sha256"],
            "departments":self.departments,"api_publication":self.api_publication,"build":build,"tool":tool}),
        )
    }

    fn check_departments(&self, verified: &Value) -> Result<()> {
        if ["stage", "plan", "prerequisite"]
            .into_iter()
            .any(|field| self.departments[field] != verified[field])
        {
            return Err("独立身份核验前的部门后像已变化".into());
        }
        Ok(())
    }

    fn check_projection(&self, backend: &Path, first: bool) -> Result<Value> {
        let plan = common::read_bound(backend, &self.request["identity_plan"], "原身份计划")?;
        let root = Path::new(document::text(
            &self.request["identity_state"],
            "原身份账本目录",
        )?);
        let current = seed_evidence::projection(backend, &plan, root, false)?;
        if first || current["state"] == "prepared" {
            if current != self.prepared {
                return Err("原身份准备的完整账本已经变化".into());
            }
        } else if [
            "identity_state",
            "plan_sha256",
            "projection_sha256",
            "entries",
            "roles",
            "users",
        ]
        .iter()
        .any(|key| current[*key] != self.prepared[*key])
        {
            return Err("身份核验重试不能改写原 622 项已确认投影".into());
        }
        Ok(current)
    }
}

fn prepared_history(
    backend: &Path,
    run: &Path,
    stage: &Value,
    original: &Value,
    request: &Value,
    reconciliation: &Value,
    old_tool: &Value,
) -> Result<Value> {
    let result = common::read_bound(backend, &stage["result"], "原身份应用结果")?;
    let number = number_of(&stage["number"])?;
    let plan = common::read_bound(backend, &request["identity_plan"], "原身份计划")?;
    let prepared = &result["identity"];
    let ledger = &prepared["ledger"];
    manifest::validate_binding_shape(ledger, "原准备账本")?;
    let root = Path::new(document::text(
        &request["identity_state"],
        "原身份账本目录",
    )?);
    if result["status"] != "seed_identities_prepared"
        || result["registration"] != *original
        || result["restore_qualified"] != false
        || result["worker_authorized"] != false
        || prepared["identity_state"] != request["identity_state"]
        || prepared["state"] != "prepared"
        || prepared["entries"] != 622
        || prepared["roles"] != 11
        || prepared["users"] != 200
        || prepared["plan_sha256"] != plan["plan_sha256"]
        || !digest_shape(&prepared["projection_sha256"])
        || Path::new(document::text(&ledger["path"], "原准备账本路径")?) != root.join("ledger.json")
    {
        return Err("原身份应用结果没有绑定同一完整准备投影".into());
    }
    let output = output(run, number);
    let producer = read_at(
        backend,
        &result["producer"],
        &output.join("session-process.json"),
    )?;
    let launch = read_at(
        backend,
        &producer["launch"],
        &output.join("session-launch.json"),
    )?;
    let (controller, _) = verify_controller(run, stage, false)?;
    if producer["kind"] != "devex-post-producer"
        || launch["kind"] != "devex-post-producer-launch"
        || launch["attempt"] != number
        || launch["controller"] != controller
        || launch["producer_kind"] != "identity-apply"
        || launch["registration"] != reconciliation["identity_successor"]
        || launch["tool"] != old_tool["identity_tool"]
        || producer["identity"]["executable"] != launch["tool"]["path"]
    {
        return Err("原身份应用生产者没有绑定已通过的原和解工具".into());
    }
    common::validate_process_identity(&producer["identity"])?;
    Ok(prepared.clone())
}

fn department_history(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    number: u64,
    api: u64,
) -> Result<Value> {
    let latest = before(attempts, number)
        .find(|row| department_api::mode(row).is_some())
        .filter(|row| row["mode"] == "departments-verify" && row["status"] == "passed")
        .ok_or("独立核验工具缺少最新通过的部门只读验证")?;
    let n = number_of(&latest["number"])?;
    if n <= api {
        return Err("部门只读验证必须属于同一 API 后继阶段".into());
    }
    let result = common::read_bound(backend, &latest["result"], "原部门只读验证")?;
    let dir = run.join("seed-runtime");
    let plan = read_at(backend, &result["plan"], &dir.join("department-plan.json"))?;
    let observed = document::array(&result["observed"], "部门最终观察")?;
    if result["status"] != "seed_departments_verified"
        || result["remote_writes"] != 0
        || result["restore_qualified"] != false
        || result["worker_authorized"] != false
        || observed.len() != 10
        || document::array(&result["confirmed"], "部门已确认前后像")?.len() != 10
    {
        return Err("独立核验工具需要十个已通过的部门最终后像".into());
    }
    for observed in observed {
        if common::bound_file(backend, observed, "部门最终后像")?.parent()
            != Some(output(run, n).as_path())
        {
            return Err("部门后像不属于最新成功阶段".into());
        }
    }
    read_at(
        backend,
        &plan["prerequisite"],
        &dir.join("departments").join("identity-prerequisite.json"),
    )?;
    Ok(json!({"stage":latest["result"],"plan":result["plan"],"prerequisite":plan["prerequisite"]}))
}

fn verify_controller(run: &Path, row: &Value, live: bool) -> Result<(Value, Value)> {
    let number = number_of(&row["number"])?;
    let mut running = row.clone();
    running["status"] = json!("running");
    for field in ["finished_at", "error_type", "result"] {
        running[field] = Value::Null;
    }
    let (binding, owner) = state::controller_record(run, number, &running)?;
    identity::validate(&owner, run)?;
    let selected = common::validate_process_identity(&owner["identity"])?;
    if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"] {
        return Err("身份核验控制器属于其他运行".into());
    }
    if live {
        let pid = u32::try_from(selected["pid"].as_u64().ok_or("身份核验控制器缺少 PID")?)?;
        if row["status"] != "running"
            || document::read(&run.join("run.lock").join("owner.json"))? != owner
            || common::process_identity(pid)?.as_ref() != Some(&selected)
        {
            return Err("原控制器未持有身份核验当前运行锁".into());
        }
    }
    Ok((binding, selected))
}

fn read_at(backend: &Path, binding: &Value, path: &Path) -> Result<Value> {
    if common::bound_file(backend, binding, "独立身份核验固定证据")? != path {
        return Err("独立身份核验文件不在固定位置".into());
    }
    common::read_bound(backend, binding, "独立身份核验固定证据")
}

fn before(attempts: &[Value], number: u64) -> impl Iterator<Item = &Value> {
    attempts
        .iter()
        .rev()
        .filter(move |row| row["number"].as_u64().is_some_and(|n| n < number))
}

fn is_api_successor(row: &Value) -> bool {
    row["stage"] == "runtime-target"
        && matches!(row["mode"].as_str(), Some("restart-api" | "reconcile-api"))
        && row["status"] == "passed"
}

fn is_verify(row: &Value) -> bool {
    row["stage"] == "seed-runtime" && row["mode"] == MODE
}

fn verify_attempt(attempts: &[Value], number: u64) -> Result<&Value> {
    attempts
        .iter()
        .find(|row| row["number"] == number && is_verify(row))
        .ok_or_else(|| "独立身份工具只授权正式 identities-verify".into())
}

fn number_of(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "独立身份核验阶段序号无效".into())
}

fn digest_shape(value: &Value) -> bool {
    value.as_str().is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

fn output(run: &Path, number: u64) -> PathBuf {
    run.join("seed-runtime")
        .join(format!("attempt-{number:04}"))
}

fn first_publication_path(run: &Path) -> PathBuf {
    run.join("post-copy/api-successors/published.json")
}

#[cfg(test)]
#[test]
fn verification_successor_cannot_replace_other_modes_or_a_historical_launch() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path();
    document::write_new(&run.join("manifest.json"), &json!({})).unwrap();
    state::initialize(run).unwrap();
    let (api, _) = state::begin(run, "runtime-target", "restart-api", &json!({})).unwrap();
    state::finish(
        run,
        api,
        Some(&json!({"status":"api_successor_ready"})),
        None,
    )
    .unwrap();
    let (apply, _) = state::begin(run, "seed-runtime", "identities-apply", &json!({})).unwrap();
    assert!(require_original(run, Some(apply), false).is_ok());
    state::finish(
        run,
        apply,
        Some(&json!({"status":"seed_identities_prepared"})),
        None,
    )
    .unwrap();
    let (verify, _) = state::begin(run, "seed-runtime", MODE, &json!({})).unwrap();
    let saved = state::load(run, false).unwrap();
    let rows = state::attempts(&saved).unwrap();
    assert!(verify_attempt(rows, apply).is_err());
    assert!(verify_attempt(rows, verify).is_ok());
    assert!(require_original(run, Some(verify), false).is_err());
    assert!(require_original(run, Some(verify), true).is_ok());
    let output = output(run, verify);
    fs::create_dir_all(&output).unwrap();
    document::write_new(&output.join("session-launch.json"), &json!({})).unwrap();
    assert!(require_original(run, Some(verify), true).is_err());
}
