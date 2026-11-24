//! 完整复制后的第五代只接原第四代，不跳过任何既有事故或标记祖先。
use super::*;

pub(super) fn split(
    ledger: &Value,
    attempts: &[Value],
    enabled: bool,
) -> Result<(Value, Vec<Value>, Option<Value>)> {
    let rows: Vec<_> = attempts
        .iter()
        .filter(|r| {
            r["stage"] == "fixture-services"
                && r["mode"] == "restart-redis"
                && r["status"] == "passed"
        })
        .collect();
    if !enabled || rows.len() < 2 {
        return Ok((ledger.clone(), attempts.to_vec(), None));
    }
    let [fourth, fifth] = rows.as_slice() else {
        return Err("完整复制标记只接受第四及第五两个 Redis 单独后继".into());
    };
    if !matches!((fourth["number"].as_u64(), fifth["number"].as_u64()), (Some(a), Some(b)) if a < b)
        || ledger["active_generation"] != fifth["result"]
    {
        return Err("第五 Redis 代次不是第四代之后唯一当前后继".into());
    }
    let mut prior = ledger.clone();
    prior["active_generation"] = fourth["result"].clone();
    Ok((
        prior,
        attempts.iter().filter(|r| *r != *fifth).cloned().collect(),
        Some((*fifth).clone()),
    ))
}

pub(super) fn link(
    evidence: &mut Evidence<'_>,
    run: &Path,
    initial: &Value,
    previous: &Restart<'_>,
    row: &Value,
    active: &Restart<'_>,
) -> Result<Value> {
    let number = row["number"].as_u64().ok_or("第五代缺少阶段号")?;
    let phase = run.join(format!("lifecycle-{number:04}"));
    redis_incident::check_generation(run, &phase, previous, row, active)?;
    let descriptor = &active.generation["incident"];
    require_path(descriptor, &phase.join("redis-incident.json"))?;
    let incident = evidence.read(descriptor, "第五代原不完整清理事故")?;
    document::exact(
        &incident,
        &[
            "kind",
            "failure",
            "runtime",
            "supervisor",
            "process",
            "port",
            "normal_job_exit_proof",
        ],
        "第五代原不完整清理事故",
    )?;
    let prior_phase = run.join(format!(
        "lifecycle-{:04}",
        previous.generation["attempt"]
            .as_u64()
            .ok_or("第四代阶段号无效")?
    ));
    require_path(&incident["failure"], &prior_phase.join("redis/failed.json"))?;
    require_path(
        &incident["runtime"],
        &prior_phase.join("redis/runtime.json"),
    )?;
    let runtime = evidence.read(&incident["runtime"], "第四代原 Redis 运行收据")?;
    let failed = evidence.read(&incident["failure"], "第四代原失败收据")?;
    if incident["kind"] != "windows-redis-incomplete-cleanup-incident"
        || !incident["normal_job_exit_proof"].is_null()
        || runtime != previous.generation["redis"]["runtime"]
        || incident["supervisor"] != runtime["supervisor"]
        || incident["process"] != runtime["process"]
        || incident["port"] != runtime["redis"]["port"]
        || failed["cleanup_completed"] != false
        || failed["supervisor"] != runtime["supervisor"]
    {
        return Err("第五代未绑定第四代原不完整清理失败，不允许替换普通清理合同".into());
    }
    let current = &active.generation["redis"]["runtime"];
    check_redis_successor(initial, &current["redis"], &runtime, &active.request)?;
    let runtime_binding = common::binding(&phase.join("redis/runtime.json"))?;
    if evidence.read(&runtime_binding, "第五代 Redis 运行收据")? != *current {
        return Err("第五代 Redis 运行后像不同于服务账本".into());
    }
    Ok(
        json!({"previous_restart":previous.result,"previous_generation":previous.receipt,
        "previous_runtime":incident["runtime"],"incident":descriptor,
        "current_request":active.generation["redis"]["request"],"current_runtime":runtime_binding}),
    )
}

fn require_path(binding: &Value, path: &Path) -> Result<()> {
    if Path::new(document::text(&binding["path"], "第五代原绑定路径")?) != path {
        return Err("第五代事故证据路径不是同一服务账本中的精确前驱".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifth_generation_requires_explicit_complete_window_and_exact_active_predecessor() {
        let rows: Vec<_> = [17, 18]
            .map(|n| {
                json!({"number":n,"stage":"fixture-services",
            "mode":"restart-redis","status":"passed","result":{"path":format!("result-{n}")}})
            })
            .into();
        let ledger = json!({"active_generation":rows[1]["result"]});
        let (prior, retained, last) = split(&ledger, &rows, true).unwrap();
        assert_eq!(prior["active_generation"], rows[0]["result"]);
        assert_eq!(retained, rows[..1]);
        assert_eq!(last.as_ref(), Some(&rows[1]));
        assert!(split(&ledger, &rows, false).unwrap().2.is_none());
        assert!(split(&json!({"active_generation":rows[0]["result"]}), &rows, true).is_err());
        let mut repeated = rows.clone();
        repeated.push(rows[1].clone());
        assert!(split(&ledger, &repeated, true).is_err());
        let mut wrong_order = rows;
        wrong_order[1]["number"] = json!(16);
        assert!(split(&ledger, &wrong_order, true).is_err());
    }
}
