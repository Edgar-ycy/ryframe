//! 拒绝重放已消费的身份核验失败，只从同一确定的历史 API 代次接续。
use super::*;
use crate::recovery::clone::native::post_process::api_successor;

pub(super) struct Second {
    pub(super) request: Value,
    pub(super) predecessor: Value,
    pub(super) original_restart: u64,
    pub(super) runtime_directory: PathBuf,
    pub(super) pending: Option<u64>,
    bindings: Vec<Value>,
    marker: Value,
    incident: Value,
}

impl Second {
    pub(super) fn bind_request(backend: &Path, path: &Path) -> Result<Value> {
        common::binding(&common::common_file(backend, path, "第二代 API 接续请求")?)
    }

    pub(super) fn load(
        stage: &StageRequest<'_>,
        saved: &Value,
        post: &history::Post,
        request: &Value,
    ) -> Result<Self> {
        let (backend, run) = (stage.backend, stage.run_dir);
        let contents = read_request(backend, request)?;
        let rows = state::attempts(saved)?;
        let (index, failure) = latest_verification_failure(rows)?;
        let number = number(failure)?;
        let pending = pending_restart(stage, &rows[index + 1..])?;
        let failed_controller =
            failure_controller(backend, run, failure, &contents["failed_verification"])?;
        let selected = api_successor::select_before(backend, run, &post.descriptor, number)?
            .ok_or("身份核验失败前缺少完整通过的 API 后继")?;
        let old = rows
            .iter()
            .find(|row| row["number"] == selected.passed_attempt && row["status"] == "passed")
            .ok_or("第二代 API 缺少原已通过的正式阶段")?;
        let predecessor = old["result"].clone();
        let result = common::read_bound(backend, &predecessor, "原已通过 API 阶段结果")?;
        if predecessor != contents["predecessor"] || result["publication"] != selected.binding {
            return Err("请求未绑定失败前最后一个已通过 API 阶段的原正式结果".into());
        }
        let runtime_directory = evidence::attempt_directory(run, selected.restart_attempt);
        let old_intent = common::read_bound(
            backend,
            &selected.publication["intent"],
            "原第一代 API 意图",
        )?;
        if selected.publication["runtime_directory"] != json!(runtime_directory)
            || old_intent["format_version"] != 1
            || old_intent["attempt"] != selected.restart_attempt
        {
            return Err("本接续入口必须绑定原第一代 API 及其规范运行目录".into());
        }
        let incident = incident_evidence(
            backend,
            &runtime_directory,
            &selected.publication,
            &old_intent,
        )?;
        let mut bindings = vec![
            request.clone(),
            predecessor.clone(),
            contents["failed_verification"].clone(),
            failed_controller,
            selected.binding,
            selected.publication["intent"].clone(),
            selected.publication["runtime"].clone(),
            selected.publication["binaries"].clone(),
        ];
        for field in ["api", "tree", "ready", "failure"] {
            bindings.push(incident[field].clone());
        }
        Ok(Self {
            request: request.clone(),
            predecessor,
            original_restart: selected.restart_attempt,
            runtime_directory,
            pending,
            bindings,
            marker: old_intent["marker"].clone(),
            incident,
        })
    }

    pub(super) fn paths(&self, backend: &Path) -> Result<Vec<PathBuf>> {
        self.bindings
            .iter()
            .map(|file| common::bound_file(backend, file, "原 API 后继或核验失败证据"))
            .collect()
    }

    pub(super) fn check_marker(&self, marker: &Markers) -> Result<()> {
        if marker.result != self.marker {
            return Err("第二代 API 接续没有保留第一代绑定的原 Redis 标记确认".into());
        }
        Ok(())
    }

    pub(super) fn check_incident(&self, observed: &Value) -> Result<()> {
        if *observed != self.incident {
            return Err("第二代 API 接续的现场事故不同于第一代原进程、失败收据或监听证明".into());
        }
        Ok(())
    }
}

fn read_request(backend: &Path, binding: &Value) -> Result<Value> {
    let value = common::read_bound(backend, binding, "第二代 API 接续请求")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "predecessor",
            "failed_verification",
        ],
        "第二代 API 接续请求",
    )?;
    if value["format_version"] != 1 || value["kind"] != "devex-clone-api-successor-request" {
        return Err("未知的第二代 API 接续请求格式".into());
    }
    Ok(value)
}

fn latest_verification_failure(rows: &[Value]) -> Result<(usize, &Value)> {
    let (index, row) = rows
        .iter()
        .enumerate()
        .rfind(|(_, row)| row["stage"] == "seed-runtime" && row["mode"] == "identities-verify")
        .ok_or("第二代 API 缺少原身份核验失败阶段")?;
    if row["status"] != "failed"
        || row["finished_at"].as_str().is_none_or(str::is_empty)
        || !row["result"].is_null()
        || row["error_type"] != "StageError"
    {
        return Err("最新身份核验不属于无正式结果的原失败阶段".into());
    }
    Ok((index, row))
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or_else(|| "第二代 API 历史缺少有效阶段序号".into())
}

fn failure_controller(
    backend: &Path,
    run: &Path,
    failed: &Value,
    requested: &Value,
) -> Result<Value> {
    let attempt = number(failed)?;
    let expected_path = run.join(format!("failure-{attempt:04}.json"));
    let path = common::bound_file(backend, requested, "原失败身份核验收据")?;
    if path != expected_path {
        return Err("第二代 API 只接受原身份核验的规范失败收据".into());
    }
    let (controller, owner) = state::controller_record(run, attempt, &evidence::running(failed))?;
    identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"] {
        return Err("身份核验失败控制器不属于原运行清单".into());
    }
    let receipt = common::read_bound(backend, requested, "原失败身份核验收据")?;
    if receipt
        != json!({"format_version":2,"kind":"devex-stage-failure","attempt":attempt,
        "stage":"seed-runtime","mode":"identities-verify","error_type":"StageError",
        "operation_id":"clone.seed-runtime.identities-verify","controller":controller})
    {
        return Err("身份核验失败文件与原阶段、操作或控制器不匹配".into());
    }
    Ok(controller)
}

fn pending_restart(stage: &StageRequest<'_>, later: &[Value]) -> Result<Option<u64>> {
    let mut original = None;
    for row in later {
        let attempt = number(row)?;
        if row["stage"] == stage.stage
            && stage.attempt == Some(attempt)
            && row["mode"] == stage.mode
            && row["status"] == "running"
        {
            continue;
        }
        if row["stage"] != "runtime-target" {
            return Err("身份核验失败后已有其他正式阶段，禁止回退到旧 API 接续请求".into());
        }
        let status = row["status"].as_str();
        match row["mode"].as_str() {
            Some("status") if matches!(status, Some("passed" | "failed")) => {}
            Some("reconcile-api") if status == Some("failed") => {}
            Some("restart-api") if status == Some("failed") => {
                let directory = evidence::attempt_directory(stage.run_dir, attempt);
                match fs::symlink_metadata(&directory) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                    Ok(_) => {
                        if original.replace(attempt).is_some() {
                            return Err(
                                "身份核验失败后有多个未完成 API 启动目录，保留原现场".into()
                            );
                        }
                    }
                }
            }
            _ => return Err("原身份核验失败已被消费或后继阶段尚未关闭".into()),
        }
    }
    match (stage.mode, original) {
        ("restart-api", None) | ("reconcile-api", Some(_)) => Ok(original),
        ("restart-api", Some(_)) => Err("已有原失败 API 启动，只能用同一请求核对".into()),
        _ => Err("没有可由同一请求核对的唯一原失败 API 启动".into()),
    }
}

fn incident_evidence(
    backend: &Path,
    directory: &Path,
    publication: &Value,
    intent: &Value,
) -> Result<Value> {
    let token = evidence::token(&intent["token"])?;
    let tree = common::read_bound(backend, &publication["api_tree"], "第一代 API 原进程树")?;
    let api = common::read_bound(backend, &publication["api"], "第一代 API 原角色收据")?;
    let ready = common::binding(&directory.join(format!("api-members-{token}-ready.json")))?;
    let failure = common::binding(&directory.join(format!("api-members-{token}-stopped.json")))?;
    let failed = common::read_bound(backend, &failure, "第一代 API 原监督失败")?;
    let ready_proof = common::read_bound(backend, &ready, "第一代 API 原监督就绪")?;
    let expected_ready = json!({"format_version":1,"operation_id":token,"directory":directory,
        "role":"api","scope_id":tree["scope_id"],"supervisor":tree["supervisor"],
        "monitor":tree["monitor"]});
    let mut expected_failure = expected_ready.clone();
    expected_failure["status"] = json!("failed");
    expected_failure["error_type"] = json!("MemberCleanupFailed");
    expected_failure["members"] = failed["members"].clone();
    if tree["operation_id"] != token
        || tree["runtime_directory"] != json!(directory)
        || api["identity"] != tree["process"]
        || ready_proof != expected_ready
        || failed != expected_failure
        || !failed["members"].as_array().is_some_and(|members| {
            members.contains(&tree["supervisor"]) && members.contains(&tree["process"])
        })
    {
        return Err("第一代 API 的故障四收据不属于同一原监督与产品进程".into());
    }
    Ok(json!({"kind":"windows-api-incomplete-cleanup-incident",
        "scope_id":tree["scope_id"],"operation_id":token,
        "api":publication["api"],"tree":publication["api_tree"],"ready":ready,"failure":failure,
        "identities":{"monitor":tree["monitor"],"supervisor":tree["supervisor"],
            "process":tree["process"],"members":failed["members"]},
        "normal_job_exit_proof":null
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_a_failed_verification_rejects_replay_and_requires_one_original_restart() {
        let temp = tempfile::tempdir().unwrap();
        let run = temp.path();
        let mut stage = StageRequest {
            backend: run,
            run_dir: run,
            stage: "runtime-target",
            mode: "restart-api",
            attempt: None,
        };
        let original =
            json!({"number":66,"stage":"runtime-target","mode":"restart-api","status":"failed"});
        let status =
            json!({"number":67,"stage":"runtime-target","mode":"status","status":"passed"});
        let later = [original.clone(), status];
        assert_eq!(pending_restart(&stage, &later).unwrap(), None);
        fs::create_dir_all(evidence::attempt_directory(run, 66)).unwrap();
        assert!(pending_restart(&stage, &later).is_err());

        stage.mode = "reconcile-api";
        assert_eq!(pending_restart(&stage, &later).unwrap(), Some(66));
        let passed =
            json!({"number":68,"stage":"runtime-target","mode":"reconcile-api","status":"passed"});
        assert!(pending_restart(&stage, &[original.clone(), passed]).is_err());
        let intervening = json!({"number":68,"stage":"seed-runtime","mode":"identities-verify","status":"passed"});
        assert!(pending_restart(&stage, &[original.clone(), intervening]).is_err());
        let another =
            json!({"number":68,"stage":"runtime-target","mode":"restart-api","status":"failed"});
        fs::create_dir(evidence::attempt_directory(run, 68)).unwrap();
        assert!(pending_restart(&stage, &[original, another]).is_err());
    }
}
