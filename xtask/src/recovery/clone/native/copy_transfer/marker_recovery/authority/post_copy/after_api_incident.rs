//! 保全已经开始业务的原 API 与 seed 历史，限定事故后 Redis 标记的写入窗口。
use super::*;
use crate::recovery::{
    clone::native::cache::handoff::registration,
    full_stack::worker::controller::observe_windows_api_incident,
};

const RUNTIME_GUARD: &str = "runtime-control.guard";
const API_GUARD: &str = "api-control.guard";

const TAIL: &[(&str, &str, Option<&str>)] = &[
    ("post-copy", "schedules", Some("PostCopyCleanupRejected")),
    ("post-copy", "recover-session", None),
    ("post-copy", "schedules", Some("PostCopyCleanupRejected")),
    ("post-copy", "recover-session", None),
    ("post-copy", "schedules", None),
    ("post-copy", "verify", None),
    ("seed-runtime", "register", None),
    (
        "seed-runtime",
        "quotas-plan",
        Some("PostCopyCleanupRejected"),
    ),
    ("seed-runtime", "quotas-plan", None),
    ("seed-runtime", "quotas-apply", None),
    ("seed-runtime", "identities-apply", Some("StageError")),
    ("seed-runtime", "identities-reconcile", Some("StageError")),
    ("seed-runtime", "identities-reconcile", None),
    ("seed-runtime", "identities-apply", None),
    (
        "seed-runtime",
        "departments-plan",
        Some("PostCopyCleanupRejected"),
    ),
    ("seed-runtime", "recover-session", None),
];

pub(super) struct Window<'a> {
    pub(super) before_start: &'a [Value],
    pub(super) rows: &'a [Value],
    marker: &'a Value,
}

pub(super) fn window(attempts: &[Value], current: u64) -> Result<Option<Window<'_>>> {
    let Some(start) = attempts.iter().position(|row| {
        row["stage"] == "runtime-target" && row["mode"] == "start" && row["status"] == "passed"
    }) else {
        return Ok(None);
    };
    let (before_start, after_start) = attempts.split_at(start);
    let marker = before_start
        .last()
        .filter(|row| {
            row["stage"] == "target-cache-markers"
                && row["mode"] == "run"
                && row["status"] == "passed"
        })
        .ok_or("API 原启动缺少紧邻的标记确认")?;
    let count = TAIL.len() + 1;
    let rows = after_start
        .get(..count)
        .ok_or("API 事故窗口缺少原业务闭合历史")?;
    check_window(rows, &after_start[count..], current)?;
    Ok(Some(Window {
        before_start,
        rows,
        marker,
    }))
}

fn check_window(rows: &[Value], later: &[Value], current: u64) -> Result<()> {
    let expected = std::iter::once(("runtime-target", "start", None)).chain(TAIL.iter().copied());
    for (row, (stage, mode, failure)) in rows.iter().zip(expected) {
        let closed = row["finished_at"].is_string() && number(row)? < current;
        let witnessed = match failure {
            Some(error) => {
                row["status"] == "failed" && row["error_type"] == error && row["result"].is_null()
            }
            None => {
                row["status"] == "passed"
                    && row["error_type"].is_null()
                    && row["result"].is_object()
            }
        };
        if !closed || !witnessed || row["stage"] != stage || row["mode"] != mode {
            return Err("API 启动后的原业务历史与事故窗口不匹配".into());
        }
    }
    let Some((running, failed)) = later.split_last() else {
        return Err("API 事故后缺少当前标记阶段".into());
    };
    if !marker_attempt(running)
        || running["number"] != current
        || running["status"] != "running"
        || failed.iter().any(|row| {
            !marker_attempt(row)
                || row["status"] != "failed"
                || !row["result"].is_null()
                || !row["finished_at"].is_string()
                || row["number"].as_u64().is_none_or(|n| n >= current)
        })
    {
        return Err("API 事故后的原业务已推进或标记阶段没有合法收口".into());
    }
    Ok(())
}

fn marker_attempt(row: &Value) -> bool {
    row["stage"] == "target-cache-markers"
        && matches!(row["mode"].as_str(), Some("run" | "reconcile"))
}

pub(super) struct Evidence {
    runtime: PathBuf,
    contract: Value,
    api_url: String,
    worker_url: String,
    runtime_guard: ProcessGuard,
    api_guard: ProcessGuard,
    files: Inputs,
    artifacts: Inputs,
    observation: Value,
    pub(super) prepared: Value,
    pub(super) binding: Value,
}

impl Evidence {
    pub(super) fn load(
        request: &StageRequest<'_>,
        post: &seed::history::Post,
        window: Window<'_>,
        preparation: PreparedRows<'_>,
    ) -> Result<Self> {
        let backend = request.backend;
        let run = request.run_dir;
        let runtime = run.join("post-copy/runtime");
        if runtime != post.runtime {
            return Err("事故标记窗口必须保留原 API 运行目录".into());
        }
        let runtime_guard = Checkpoint::ApiIncidentFiles.check(
            ProcessGuard::acquire_existing_named(&runtime, RUNTIME_GUARD),
        )?;
        let api_guard = Checkpoint::ApiIncidentFiles
            .check(ProcessGuard::acquire_existing_named(&runtime, API_GUARD))?;
        let mut files = Vec::new();
        let (prepared, marker) = Checkpoint::ApiIncidentHistory.check(marker_parent(
            backend,
            window.marker,
            &preparation,
            &mut files,
        ))?;
        let contract = Checkpoint::ApiIncidentRuntime
            .check(registration::cleanup_runtime(backend, run, post))?;
        let handoff = read(backend, &prepared["handoff"], &mut files)?;
        let api_url = document::text(&handoff["api_url"], "原 API 端点")?.to_owned();
        let worker_url =
            document::text(&contract["worker_ready_url"], "原 Worker 端点")?.to_owned();
        let pinned_config = common::read_bound(backend, &prepared["runtime"], "原 API 准备合同")?;
        if pinned_config != contract || handoff["registration"] != post.descriptor {
            return Err("事故窗口的原 API 准备合同不匹配".into());
        }
        let artifacts = Checkpoint::ApiIncidentFiles
            .check(prepared_before_api::hold_artifacts(backend, &contract))?;
        let observation = Checkpoint::ApiIncidentObservation.check(
            observe_windows_api_incident(&runtime, &contract, &api_url, &worker_url),
        )?;
        for key in ["api", "tree", "ready", "failure"] {
            files.push(common::bound_file(
                backend,
                &observation[key],
                "原 API 事故证据",
            )?);
        }
        let history = Checkpoint::ApiIncidentHistory.check(bind_closed_history(
            backend,
            run,
            window.rows,
            &observation,
            &mut files,
        ))?;
        let evidence = Self {
            runtime,
            contract,
            api_url,
            worker_url,
            runtime_guard,
            api_guard,
            files: Checkpoint::ApiIncidentFiles.check(hold(backend, &files))?,
            artifacts,
            observation: observation.clone(),
            prepared,
            binding: json!({"history":history,"previous_markers":marker,"api_incident":observation}),
        };
        evidence.check()?;
        Ok(evidence)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.runtime_guard
            .check_named(&self.runtime, RUNTIME_GUARD)?;
        self.api_guard.check_named(&self.runtime, API_GUARD)?;
        self.files.check()?;
        #[cfg(windows)]
        self.artifacts.check_identities()?;
        #[cfg(not(windows))]
        self.artifacts.check()?;
        if Checkpoint::ApiIncidentObservation.check(observe_windows_api_incident(
            &self.runtime,
            &self.contract,
            &self.api_url,
            &self.worker_url,
        ))? != self.observation
        {
            return Err("原 API 事故证据或运行端口在标记窗口中变化".into());
        }
        self.files.check()
    }
}

fn marker_parent(
    backend: &Path,
    row: &Value,
    preparation: &PreparedRows<'_>,
    files: &mut Vec<PathBuf>,
) -> Result<(Value, Value)> {
    let result = read(backend, &row["result"], files)?;
    let observation = read(backend, &result["observation"], files)?;
    let intent = read(backend, &observation["intent"], files)?;
    let prepared = &intent["binding"]["completed_copy"]["prepared_before_api"];
    if result["status"] != "target_cache_markers_confirmed"
        || observation["kind"] != "fixture_redis_marker_observation"
        || observation["confirmed"] != true
        || observation["classification"] != "after"
        || intent["kind"] != "fixture_redis_marker_intent"
        || observation["attempt"] != row["number"]
        || intent["attempt"] != row["number"]
        || observation["binding"] != intent["binding"]
        || observation["observed"] != intent["after"]
        || prepared["prepare"] != preparation.prepare["result"]
    {
        return Err("API 事故恢复不属于原准备及已确认 Redis 标记".into());
    }
    for field in [
        "prepare",
        "prepare_controller",
        "handoff",
        "runtime",
        "binaries",
        "failure",
        "start_controller",
    ] {
        files.push(common::bound_file(
            backend,
            &prepared[field],
            "原 API 准备证据",
        )?);
    }
    let configs = document::array(&prepared["configuration"], "原 API 与 Worker 配置")?;
    if configs.len() != 4 {
        return Err("原 API 准备配置数量变化".into());
    }
    for descriptor in configs {
        files.push(common::bound_file(backend, descriptor, "原冻结运行配置")?);
    }
    Ok((
        prepared.clone(),
        json!({"result":row["result"],"observation":result["observation"],"intent":observation["intent"]}),
    ))
}

fn bind_closed_history(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    incident: &Value,
    files: &mut Vec<PathBuf>,
) -> Result<Value> {
    let [start, rest @ .., department, recovered] = rows else {
        return Err("API 事故缺少启动及 seed 会话闭合".into());
    };
    let mut evidence = Vec::with_capacity(rest.len() + 3);
    for row in rows {
        let attempt = number(row)?;
        let controller = prepared_before_api::closed_controller(run, row)?;
        files.push(common::bound_file(
            backend,
            &controller,
            "原业务闭合阶段控制器",
        )?);
        let outcome = if row["status"] == "passed" {
            read(backend, &row["result"], files)?;
            row["result"].clone()
        } else {
            let descriptor = common::binding(&run.join(format!("failure-{attempt:04}.json")))?;
            let failure = read(backend, &descriptor, files)?;
            if failure["kind"] != "devex-stage-failure"
                || failure["attempt"] != attempt
                || failure["stage"] != row["stage"]
                || failure["mode"] != row["mode"]
                || failure["controller"] != controller
                || failure["error_type"] != row["error_type"]
            {
                return Err("原业务失败没有被原控制器精确收口".into());
            }
            descriptor
        };
        evidence.push(json!({"controller":controller,"outcome":outcome}));
    }
    let started = common::read_bound(backend, &start["result"], "原 API 启动结果")?;
    let recovered = common::read_bound(backend, &recovered["result"], "原 seed 会话回收结果")?;
    let producer_path = run.join(format!(
        "seed-runtime/attempt-{:04}/session-process.json",
        number(department)?
    ));
    let producer = common::bound_file(backend, &recovered["producer"], "原部门失败会话进程")?;
    if started["kind"] != "devex-clone-runtime-control"
        || started["operation"] != "start"
        || started["processes"]["api"]["identity"] != incident["identities"]["process"]
        || started["scope_id"] != incident["scope_id"]
        || recovered["status"] != "seed_session_recovered"
        || recovered["remote_writes"] != 0
        || recovered["already_stopped"] != true
        || producer != producer_path
    {
        return Err("原 API 启动身份或部门失败会话回收结果不匹配".into());
    }
    files.push(producer);
    Ok(json!({"stages":evidence,"closed_seed_producer":recovered["producer"]}))
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .ok_or_else(|| "事故窗口阶段编号无效".into())
}

fn read(backend: &Path, descriptor: &Value, files: &mut Vec<PathBuf>) -> Result<Value> {
    files.push(common::bound_file(backend, descriptor, "事故窗口原始收据")?);
    common::read_bound(backend, descriptor, "事故窗口原始收据")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_marker_requires_closed_department_incident_and_no_later_business() {
        let mut rows = vec![
            json!({"number":35,"stage":"target-cache-markers","mode":"run","status":"passed"}),
        ];
        let historical =
            std::iter::once(("runtime-target", "start", None)).chain(TAIL.iter().copied());
        for (index, (stage, mode, failure)) in historical.enumerate() {
            let status = if failure.is_some() {
                "failed"
            } else {
                "passed"
            };
            let result = if failure.is_some() {
                Value::Null
            } else {
                json!({"path":"result"})
            };
            rows.push(
                json!({"number":36+index,"stage":stage,"mode":mode,"status":status,
                "error_type":failure,"finished_at":"closed","result":result}),
            );
        }
        let reconcile = json!({"number":55,"stage":"target-cache-markers","mode":"reconcile","status":"running"});
        rows.push(reconcile);
        assert!(window(&rows, 55).unwrap().is_some());
        assert!(window(&rows[..1], 55).unwrap().is_none());

        let mut unfinished = rows.clone();
        let recovered = unfinished.len() - 2;
        unfinished[recovered]["finished_at"] = Value::Null;
        assert!(window(&unfinished, 55).is_err());
        let mut advanced = rows.clone();
        let later = advanced.len() - 1;
        advanced.insert(later, json!({"number":53,"stage":"seed-runtime","mode":"departments-apply","status":"passed"}));
        assert!(window(&advanced, 55).is_err());
        let earlier_marker = json!({"number":53,"stage":"target-cache-markers","mode":"run",
            "status":"failed","result":null,"finished_at":"closed"});
        rows.insert(later, earlier_marker);
        assert!(window(&rows, 55).unwrap().is_some());
        rows[later]["status"] = json!("passed");
        assert!(window(&rows, 55).is_err());
    }
}
