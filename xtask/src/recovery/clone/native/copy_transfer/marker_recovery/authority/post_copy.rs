//! 已完成复制后的受管 Redis 标记接续。
use super::*;
use crate::recovery::clone::native::seed;
mod after_api_incident;
mod failed_prepare;
mod prepared_before_api;

pub(super) struct Completed {
    backend: PathBuf,
    run: PathBuf,
    manifest: Value,
    plan: Value,
    files: Inputs,
    failures: failed_prepare::Evidence,
    prepared: Option<prepared_before_api::Evidence>,
    api_incident: Option<after_api_incident::Evidence>,
    pub(super) binding: Value,
}

struct PreparedRows<'a> {
    prepare: &'a Value,
    failure: &'a Value,
}

pub(super) fn completed_attempts(attempts: &[Value]) -> Result<(&Value, &Value)> {
    let prior = attempts
        .iter()
        .rfind(|r| r["stage"] == "copy")
        .filter(|r| r["status"] == "passed")
        .ok_or("完整复制缺少最新正式成功阶段")?;
    let verified = attempts.iter().find(|r| r["stage"] == "target-verify" && r["status"] == "passed"
        && matches!((r["number"].as_u64(), prior["number"].as_u64()), (Some(a), Some(b)) if a < b))
        .ok_or("完整复制缺少原目标复验")?;
    Ok((prior, verified))
}

pub(super) fn load(
    request: &StageRequest<'_>,
    manifest: &Value,
    saved: &Value,
) -> Result<Option<Completed>> {
    let path = request.run_dir.join("post-copy.json");
    if !path.try_exists()? {
        return Ok(None);
    }
    let attempts = state::attempts(saved)?;
    let current = request.attempt.ok_or("缺少标记阶段")?;
    let window =
        Checkpoint::ApiIncidentHistory.check(after_api_incident::window(attempts, current))?;
    let original_attempts = window.as_ref().map_or(attempts, |value| value.before_start);
    let prepared_rows = eligibility(original_attempts, current)?;
    let descriptor = common::binding(&protected_file(request.backend, &path)?)?;
    let post = seed::history::post(
        request.backend,
        request.run_dir,
        Some(&descriptor),
        request.attempt,
        false,
    )?;
    if post.request["format_version"] != 1 || post.request["kind"] != "devex-clone-post-copy" {
        return Err("完整复制标记接续只接受原 v1 post-copy 登记".into());
    }
    let (prepared, api_incident) = match (prepared_rows, window) {
        (Some(rows), Some(window)) => (
            None,
            Some(after_api_incident::Evidence::load(
                request, &post, window, rows,
            )?),
        ),
        (Some(rows), None) => (
            Some(prepared_before_api::Evidence::load(
                request, manifest, &post, rows,
            )?),
            None,
        ),
        (None, None) => {
            no_runtime(request.run_dir)?;
            (None, None)
        }
        (None, Some(_)) => return Err("原 API 事故缺少可核验的启动前准备".into()),
    };
    let failures = Checkpoint::PreparationHistory.check(failed_prepare::Evidence::load(
        request.backend,
        request.run_dir,
        manifest,
        saved,
        &descriptor,
    ))?;
    let paths = completed_files(request, manifest, attempts, path)?;
    let files = hold(request.backend, &paths)?;
    let plan = Checkpoint::CopyProof.check(seed::source_registration::copied::require(
        request.backend,
        request.run_dir,
        manifest,
    ))?;
    let mut binding = json!({"registration":descriptor,"failed_preparations":failures.binding,
        "files":paths.iter().map(|p| common::binding(p)).collect::<Result<Vec<_>>>()?});
    if let Some(prepared) = &prepared {
        binding["prepared_before_api"] = prepared.binding.clone();
    }
    if let Some(incident) = &api_incident {
        binding["prepared_before_api"] = incident.prepared.clone();
        binding["after_api_incident"] = incident.binding.clone();
    }
    let complete = Completed {
        backend: request.backend.to_owned(),
        run: request.run_dir.to_owned(),
        manifest: manifest.clone(),
        plan,
        files,
        failures,
        prepared,
        api_incident,
        binding,
    };
    complete.check()?;
    Ok(Some(complete))
}

fn completed_files(
    request: &StageRequest<'_>,
    manifest: &Value,
    attempts: &[Value],
    registration: PathBuf,
) -> Result<Vec<PathBuf>> {
    let copy = Path::new(document::text(&manifest["copy_directory"], "原复制目录")?);
    let mut paths = vec![registration];
    paths.extend(["result.json", "session.json", "input.json", "plan.json"].map(|n| copy.join(n)));
    paths.extend(["head.json", "events.ndjson"].map(|n| copy.join("ledger").join(n)));
    for row in attempts.iter().filter(|row| {
        row["stage"] == "post-copy" || row["stage"] == "copy" && row["status"] == "passed"
    }) {
        let number = row["number"].as_u64().ok_or("原完成阶段序号无效")?;
        paths.push(state::controller_path(request.run_dir, number));
        if !row["result"].is_null() {
            paths.push(common::bound_file(
                request.backend,
                &row["result"],
                "原完成阶段结果",
            )?);
        }
    }
    Ok(paths)
}

impl Completed {
    pub(super) fn prepared_before_api(&self) -> bool {
        self.prepared.is_some() || self.api_incident.is_some()
    }

    pub(super) fn after_api_incident(&self) -> bool {
        self.api_incident.is_some()
    }

    pub(super) fn check(&self) -> Result<()> {
        self.files.check()?;
        self.failures.check()?;
        if let Some(incident) = &self.api_incident {
            incident.check()?;
        } else if let Some(prepared) = &self.prepared {
            prepared.check()?;
        } else {
            no_runtime(&self.run)?;
        }
        if Checkpoint::CopyProof.check(seed::source_registration::copied::require(
            &self.backend,
            &self.run,
            &self.manifest,
        ))? != self.plan
        {
            return Err("完整复制标记接续期间原完成证明变化".into());
        }
        self.files.check()
    }
}

fn no_runtime(run: &Path) -> Result<()> {
    for name in [
        "post-copy/runtime",
        "post-copy/runtime-configuration",
        "post-copy/handoff.json",
        "seed-runtime.json",
    ] {
        absent(&run.join(name))?;
    }
    Ok(())
}

fn eligibility(attempts: &[Value], current: u64) -> Result<Option<PreparedRows<'_>>> {
    let registrations: Vec<_> = attempts
        .iter()
        .filter(|r| r["stage"] == "post-copy" && r["mode"] == "register")
        .collect();
    let [registration] = registrations.as_slice() else {
        return Err("标记接续要求唯一原 post-copy 登记".into());
    };
    let number = registration["number"].as_u64().ok_or("原登记序号无效")?;
    if registration["status"] != "passed" || number >= current {
        return Err("原 post-copy 登记未正式通过".into());
    }
    let mut failed_prepares = Vec::new();
    let mut prepared = None;
    let mut rejected = None;
    for row in attempts
        .iter()
        .filter(|r| r["number"].as_u64().is_some_and(|n| n > number))
    {
        if row["stage"] == "post-copy"
            && row["mode"] == "prepare"
            && prepared.is_none()
            && row["status"] == "failed"
            && matches!(
                row["error_type"].as_str(),
                Some("ControllerInterrupted" | "PostCopyPreparationRejected")
            )
            && row["result"].is_null()
            && row["finished_at"].is_string()
        {
            failed_prepares.push(row);
        } else if row["stage"] == "post-copy"
            && row["mode"] == "prepare"
            && prepared.is_none()
            && row["status"] == "passed"
            && row["error_type"].is_null()
            && row["finished_at"].is_string()
            && row["result"].is_object()
        {
            prepared = Some(row);
        } else if prepared.is_some()
            && rejected.is_none()
            && prepared_before_api::rejected_start(row)
        {
            rejected = Some(row);
        } else if row["stage"] != "target-cache-markers"
            || !matches!(row["mode"].as_str(), Some("run" | "reconcile"))
            || prepared.is_some() && rejected.is_none()
            || !(row["number"] == current && row["status"] == "running"
                || row["number"].as_u64().is_some_and(|n| n < current)
                    && matches!(row["status"].as_str(), Some("failed" | "passed")))
        {
            return Err("登记后已有产品、seed、成功准备或未知阶段，禁止标记接续".into());
        }
    }
    if let Some(prepare) = prepared {
        let failure = rejected.ok_or("已成功准备须由明确的 API 启动前拒绝收口")?;
        let mut explicit_rejections = failed_prepares
            .iter()
            .filter(|row| row["error_type"] == "PostCopyPreparationRejected");
        if explicit_rejections.next().is_none() || explicit_rejections.next().is_some() {
            return Err("已准备窗口缺少唯一的原明确准备拒绝".into());
        }
        return Ok(Some(PreparedRows { prepare, failure }));
    }
    verify_failed_prepares(&failed_prepares)?;
    Ok(None)
}

fn verify_failed_prepares(failed_prepares: &[&Value]) -> Result<()> {
    let Some((last, preceding)) = failed_prepares.split_last() else {
        return Err("原登记后缺少未启动产品的失败准备".into());
    };
    if last["error_type"] != "PostCopyPreparationRejected"
        || preceding
            .iter()
            .any(|row| row["error_type"] != "ControllerInterrupted")
    {
        return Err("原准备必须先由原生回收收口中断，再以明确准备拒绝结束".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_copy_marker_window_rejects_started_or_unknown_followups() {
        let register = json!({"number":22,"stage":"post-copy","mode":"register","status":"passed"});
        let failed = json!({"number":29,"stage":"post-copy","mode":"prepare","status":"failed",
            "error_type":"PostCopyPreparationRejected","finished_at":"closed","result":null});
        let current =
            json!({"number":30,"stage":"target-cache-markers","mode":"run","status":"running"});
        let mut original = vec![register];
        for number in 23..29 {
            original.push(json!({"number":number,"stage":"post-copy","mode":"prepare",
            "status":"failed","error_type":"ControllerInterrupted","finished_at":"closed","result":null}));
        }
        original.extend([failed, current]);
        assert!(matches!(eligibility(&original, 30), Ok(None)));
        assert!(eligibility(&[original[0].clone(), original[8].clone()], 30).is_err());
        let mut unclosed = original.clone();
        unclosed[7]["error_type"] = json!("ControllerInterrupted");
        assert!(eligibility(&unclosed, 30).is_err());
        let mut unknown = original.clone();
        unknown[1]["error_type"] = json!("StageError");
        assert!(eligibility(&unknown, 30).is_err());
        for (stage, mode, status) in [
            ("post-copy", "prepare", "passed"),
            ("post-copy", "start", "failed"),
            ("post-copy", "api", "failed"),
            ("post-copy", "amend", "passed"),
            ("seed-runtime", "register", "failed"),
            ("cache-target", "run", "failed"),
            ("unknown", "run", "failed"),
        ] {
            let mut rows = original.clone();
            rows[1]["stage"] = json!(stage);
            rows[1]["mode"] = json!(mode);
            rows[1]["status"] = json!(status);
            assert!(eligibility(&rows, 30).is_err(), "{stage}/{mode}/{status}");
        }
    }

    #[test]
    fn completed_copy_marker_window_preserves_preparation_and_requires_a_closed_before_start_failure()
     {
        let mut rows = vec![
            json!({"number":22,"stage":"post-copy","mode":"register","status":"passed"}),
            json!({"number":29,"stage":"post-copy","mode":"prepare","status":"failed",
                "error_type":"PostCopyPreparationRejected","finished_at":"closed","result":null}),
            json!({"number":31,"stage":"target-cache-markers","mode":"run","status":"passed"}),
            json!({"number":32,"stage":"post-copy","mode":"prepare","status":"failed",
                "error_type":"ControllerInterrupted","finished_at":"closed","result":null}),
            json!({"number":33,"stage":"post-copy","mode":"prepare","status":"passed",
                "error_type":null,"finished_at":"closed","result":{"path":"prepare-result"}}),
            json!({"number":34,"stage":"runtime-target","mode":"start","status":"failed",
                "error_type":"ApiHandoffPreStartRejected","finished_at":"closed","result":null}),
            json!({"number":35,"stage":"target-cache-markers","mode":"run","status":"running"}),
        ];
        let prepared = eligibility(&rows, 35).unwrap().unwrap();
        assert_eq!(prepared.prepare, &rows[4]);
        assert_eq!(prepared.failure, &rows[5]);

        rows[6]["mode"] = json!("reconcile");
        assert!(matches!(eligibility(&rows, 35), Ok(Some(_))));
        for (field, value) in [
            ("error_type", json!("ApiHandoffStartRejected")),
            ("status", json!("running")),
            ("finished_at", Value::Null),
            ("result", json!({"path":"unknown-result"})),
        ] {
            let mut rejected = rows.clone();
            rejected[5][field] = value;
            assert!(eligibility(&rejected, 35).is_err(), "{field}");
        }
        let no_shutdown = [&rows[..5], &rows[6..]].concat();
        assert!(eligibility(&no_shutdown, 35).is_err());
        let mut unknown_failure = rows.clone();
        unknown_failure[3]["error_type"] = json!("StageError");
        assert!(eligibility(&unknown_failure, 35).is_err());
        let mut second_start = rows.clone();
        let mut second = rows[5].clone();
        second["number"] = json!(35);
        second_start.insert(6, second);
        second_start[7]["number"] = json!(36);
        assert!(eligibility(&second_start, 36).is_err());
    }
}
