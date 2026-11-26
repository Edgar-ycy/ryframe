//! 后继 API 的持久意图、事故标记来源和只新增的发布收据。
use super::*;

pub(super) fn directory(run: &Path) -> PathBuf {
    run.join("post-copy").join("api-successors")
}

pub(super) fn attempt_directory(run: &Path, number: u64) -> PathBuf {
    directory(run).join(format!("attempt-{number:04}"))
}

pub(super) fn current(request: &StageRequest<'_>) -> Result<(Value, Value)> {
    let run = request.run_dir;
    let saved = state::load(run, true)?;
    let number = request.attempt.ok_or("API 后继缺少受管阶段序号")?;
    let last = state::attempts(&saved)?
        .last()
        .ok_or("API 后继运行阶段缺失")?;
    let (_, owner) = state::controller_record(run, number, last)?;
    if last["number"] != number
        || last["stage"] != request.stage
        || last["mode"] != request.mode
        || last["status"] != "running"
        || owner != document::read(&run.join("run.lock/owner.json"))?
        || owner["identity"] != identity::current()?
    {
        return Err("API 后继必须属于当前受管阶段和运行锁所有者".into());
    }
    Ok((saved, owner))
}

pub(super) struct Markers {
    pub(super) result: Value,
    observation: Value,
    pub(super) redis: Value,
    pub(super) incident: Value,
}

impl Markers {
    pub(super) fn load(request: &StageRequest<'_>, saved: &Value) -> Result<Self> {
        let attempts = state::attempts(saved)?;
        let index = attempts
            .iter()
            .rposition(|row| row["stage"] == "target-cache-markers")
            .ok_or("API 后继缺少本次事故后的 Redis 标记确认")?;
        let marker = &attempts[index];
        if marker["status"] != "passed"
            || !matches!(marker["mode"].as_str(), Some("run" | "reconcile"))
            || attempts[index + 1..]
                .iter()
                .any(|row| !allows_later(request, row))
        {
            return Err("API 后继必须紧接标记确认且原业务尚未继续".into());
        }
        let backend = request.backend;
        let result = common::read_bound(backend, &marker["result"], "事故 Redis 标记阶段结果")?;
        let observation = &result["observation"];
        let path = common::bound_file(backend, observation, "事故 Redis 标记观察")?;
        if !path.starts_with(request.run_dir.join("target-cache-markers")) {
            return Err("事故 Redis 标记观察不在原运行目录".into());
        }
        let proof = common::read_bound(backend, observation, "事故 Redis 标记观察")?;
        let fixture = &proof["binding"]["fixture"];
        let incident = &proof["binding"]["completed_copy"]["after_api_incident"]["api_incident"];
        if result["status"] != "target_cache_markers_confirmed"
            || result["classification"] != "after"
            || result["restore_qualified"] != false
            || proof["kind"] != "fixture_redis_marker_observation"
            || proof["confirmed"] != true
            || proof["classification"] != "after"
            || !fixture["redis"].is_object()
            || !fixture["lineage"]["runtime_incident_redis_failure"].is_object()
            || incident["kind"] != "windows-api-incomplete-cleanup-incident"
            || !incident["normal_job_exit_proof"].is_null()
        {
            return Err("Redis 标记确认未绑定本次原 API 事故及新存储代次".into());
        }
        Ok(Self {
            result: marker["result"].clone(),
            observation: observation.clone(),
            redis: fixture["redis"].clone(),
            incident: incident.clone(),
        })
    }

    pub(super) fn paths(&self, backend: &Path) -> Result<Vec<PathBuf>> {
        [
            &self.result,
            &self.observation,
            &self.incident["api"],
            &self.incident["tree"],
            &self.incident["failure"],
            &self.incident["ready"],
        ]
        .into_iter()
        .map(|file| common::bound_file(backend, file, "原事故或标记证据"))
        .collect()
    }
}

fn allows_later(request: &StageRequest<'_>, row: &Value) -> bool {
    row["stage"] == "runtime-target"
        && match row["mode"].as_str() {
            Some("status") => row["status"] != "running",
            Some("restart-api" | "reconcile-api") => {
                row["status"] == "failed"
                    || (row["status"] == "running"
                        && row["number"].as_u64() == request.attempt
                        && row["mode"] == request.mode)
            }
            _ => false,
        }
}

pub(super) struct Intent {
    pub(super) value: Value,
    binding: Value,
    controller: Value,
    pub(super) directory: PathBuf,
    pub(super) token: String,
    pub(super) identity: Value,
}

impl Intent {
    pub(super) fn load(request: &StageRequest<'_>, saved: &Value) -> Result<Self> {
        let (backend, run) = (request.backend, request.run_dir);
        let path = common::common_file(
            backend,
            &directory(run).join("intent.json"),
            "API 后继原意图",
        )?;
        let binding = common::binding(&path)?;
        let value = common::read_bound(backend, &binding, "API 后继原意图")?;
        document::exact(
            &value,
            &[
                "format_version",
                "kind",
                "attempt",
                "registration",
                "marker",
                "runtime_directory",
                "token",
                "executor",
                "incident",
            ],
            "API 后继意图",
        )?;
        let number = value["attempt"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("API 后继意图缺少启动序号")?;
        let token = document::text(&value["token"], "API 后继原预留 ID")?;
        let directory = attempt_directory(run, number);
        if value["format_version"] != 1
            || value["kind"] != "devex-clone-api-successor-intent"
            || value["runtime_directory"] != json!(directory)
            || token.len() != 32
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("API 后继意图的原目录、版本或预留 ID 无效".into());
        }
        let row = state::attempts(saved)?
            .iter()
            .find(|row| row["number"] == number)
            .ok_or("API 后继原启动未登记")?;
        let original = original_attempt(
            row,
            request.attempt == Some(number) && request.mode == "restart-api",
        )?;
        let (controller, owner) = state::controller_record(run, number, &original)?;
        identity::validate(&owner, run)?;
        if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
            || executor(&owner["identity"])? != value["executor"]
        {
            return Err("API 后继意图不同于原启动控制器或运行清单".into());
        }
        let directory = common::local_directory(backend, &directory, "API 后继原运行目录")?;
        Ok(Self {
            token: token.to_owned(),
            value,
            binding,
            controller,
            directory,
            identity: owner["identity"].clone(),
        })
    }

    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        if common::read_bound(backend, &self.binding, "API 后继原意图")? != self.value
            || executor(&self.identity)? != self.value["executor"]
        {
            return Err("API 后继启动意图或执行器在核对期间变化".into());
        }
        common::bound_file(backend, &self.controller, "API 后继原控制器记录")?;
        Ok(())
    }
}

fn original_attempt(row: &Value, current: bool) -> Result<Value> {
    if row["stage"] != "runtime-target"
        || row["mode"] != "restart-api"
        || if current {
            row["status"] != "running"
        } else {
            row["status"] != "failed"
        }
    {
        return Err("API 后继只能核对当前启动意图或原失败启动".into());
    }
    let mut original = row.clone();
    original["status"] = json!("running");
    for key in ["finished_at", "result", "error_type"] {
        original[key] = Value::Null;
    }
    Ok(original)
}

pub(super) fn executor(identity: &Value) -> Result<Value> {
    let path = Path::new(document::text(&identity["executable"], "API 后继原执行器")?);
    crate::local_test_path::validate_absolute_path(
        path,
        crate::local_test_path::LocalTestPathKind::ExistingFile,
    )?;
    common::binding(path)
}

pub(super) fn publish(
    request: &StageRequest<'_>,
    intent: &Intent,
    registration: &Value,
    identity: &Value,
    check: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    let (backend, run) = (request.backend, request.run_dir);
    let runtime = common::binding(&intent.directory.join("runtime.json"))?;
    let binaries = common::binding(&intent.directory.join("binaries.json"))?;
    let api = common::binding(&intent.directory.join("api.json"))?;
    let tree = common::binding(&intent.directory.join("api-tree.json"))?;
    let child = common::read_bound(backend, &api, "后继 API 角色收据")?;
    let observed_tree = common::read_bound(backend, &tree, "后继 API 完整树")?;
    if child["identity"] != *identity
        || observed_tree["process"] != *identity
        || observed_tree["operation_id"] != intent.token
        || observed_tree["runtime_directory"] != json!(intent.directory)
    {
        return Err("API 后继原预留、实际角色和完整树不一致".into());
    }
    let value = json!({"format_version":1,"kind":"devex-clone-api-successor","registration":registration,
        "intent":intent.binding,"runtime_directory":intent.directory,"runtime":runtime,"binaries":binaries,"api":api,"api_tree":tree});
    let path = directory(run).join("published.json");
    check()?;
    intent.check(backend)?;
    if path.try_exists()? {
        let path = common::common_file(backend, &path, "已有后继 API 发布收据")?;
        if request.mode != "reconcile-api" || document::read(&path)? != value {
            return Err("API 后继发布收据已存在，拒绝覆盖或重复启动".into());
        }
    } else {
        document::publish_new(&path, &value, "API 后继发布")?;
    }
    let publication = common::binding(&path)?;
    if common::read_bound(backend, &publication, "已发布 API 后继")? != value {
        return Err("API 后继发布读回不同".into());
    }
    check()?;
    Ok(json!({"status":"api_successor_ready","publication":publication,"restore_qualified":false}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_only_recovers_the_controller_of_the_original_failed_restart() {
        let failed = json!({"stage":"runtime-target","mode":"restart-api","status":"failed",
            "finished_at":"closed","error_type":"ControllerInterrupted","result":null});
        let restored = original_attempt(&failed, false).unwrap();
        assert_eq!(restored["status"], "running");
        assert!(restored["finished_at"].is_null());
        assert!(restored["error_type"].is_null());
        assert!(original_attempt(&failed, true).is_err());
        assert!(original_attempt(&restored, false).is_err());
        let mut other = failed;
        other["mode"] = json!("reconcile-api");
        assert!(original_attempt(&other, false).is_err());
        other["mode"] = json!("restart-api");
        other["status"] = json!("passed");
        assert!(original_attempt(&other, false).is_err());
    }
}
