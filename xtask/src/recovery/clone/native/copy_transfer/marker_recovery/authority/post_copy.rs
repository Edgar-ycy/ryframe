//! 已完成复制但从未准备或启动产品的窄范围标记接续。
use super::*;
use crate::recovery::clone::native::seed;
mod failed_prepare;

pub(super) struct Completed {
    backend: PathBuf,
    run: PathBuf,
    manifest: Value,
    plan: Value,
    files: Inputs,
    failures: failed_prepare::Evidence,
    pub(super) binding: Value,
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
    eligibility(
        state::attempts(saved)?,
        request.attempt.ok_or("缺少标记阶段")?,
    )?;
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
    no_runtime(request.run_dir)?;
    let failures = failed_prepare::Evidence::load(
        request.backend,
        request.run_dir,
        manifest,
        saved,
        &descriptor,
    )?;
    let copy = Path::new(document::text(&manifest["copy_directory"], "原复制目录")?);
    let mut paths = vec![path];
    paths.extend(["result.json", "session.json", "input.json", "plan.json"].map(|n| copy.join(n)));
    paths.extend(["head.json", "events.ndjson"].map(|n| copy.join("ledger").join(n)));
    for row in state::attempts(saved)? {
        if row["stage"] == "post-copy" || row["stage"] == "copy" && row["status"] == "passed" {
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
    }
    let files = hold(request.backend, &paths)?;
    let plan = Checkpoint::CopyProof.check(seed::source_registration::copied::require(
        request.backend,
        request.run_dir,
        manifest,
    ))?;
    let binding = json!({"registration":descriptor,"failed_preparations":failures.binding,
        "files":paths.iter().map(|p| common::binding(p)).collect::<Result<Vec<_>>>()?});
    let complete = Completed {
        backend: request.backend.to_owned(),
        run: request.run_dir.to_owned(),
        manifest: manifest.clone(),
        plan,
        files,
        failures,
        binding,
    };
    complete.check()?;
    Ok(Some(complete))
}

impl Completed {
    pub(super) fn check(&self) -> Result<()> {
        self.files.check()?;
        self.failures.check()?;
        no_runtime(&self.run)?;
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

pub(super) fn eligibility(attempts: &[Value], current: u64) -> Result<()> {
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
    for row in attempts
        .iter()
        .filter(|r| r["number"].as_u64().is_some_and(|n| n > number))
    {
        if row["stage"] == "post-copy"
            && row["mode"] == "prepare"
            && row["status"] == "failed"
            && matches!(
                row["error_type"].as_str(),
                Some("ControllerInterrupted" | "PostCopyPreparationRejected")
            )
            && row["result"].is_null()
            && row["finished_at"].is_string()
        {
            failed_prepares.push(row);
        } else if row["stage"] != "target-cache-markers"
            || !matches!(row["mode"].as_str(), Some("run" | "reconcile"))
            || !(row["number"] == current && row["status"] == "running"
                || row["number"].as_u64().is_some_and(|n| n < current)
                    && matches!(row["status"].as_str(), Some("failed" | "passed")))
        {
            return Err("登记后已有产品、seed、成功准备或未知阶段，禁止标记接续".into());
        }
    }
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
        assert!(eligibility(&original, 30).is_ok());
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
}
