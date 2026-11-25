//! 完整复制后的 Redis 单独接续必须逐代绑定原事故和运行收据。
use super::*;

pub(super) struct Continuation<'a> {
    pub(super) ledger: Value,
    pub(super) attempts: Vec<Value>,
    pub(super) fifth: Option<&'a Value>,
    pub(super) sixth: Option<&'a Value>,
}

pub(super) fn split<'a>(
    ledger: &Value,
    attempts: &'a [Value],
    completed: bool,
    prepared_before_api: bool,
) -> Result<Continuation<'a>> {
    let rows: Vec<_> = attempts
        .iter()
        .filter(|r| {
            r["stage"] == "fixture-services"
                && r["mode"] == "restart-redis"
                && r["status"] == "passed"
        })
        .collect();
    if prepared_before_api && !completed {
        return Err("API 准备后的 Redis 标记接续缺少完整复制证明".into());
    }
    if !completed || (!prepared_before_api && rows.len() < 2) {
        return Ok(Continuation {
            ledger: ledger.clone(),
            attempts: attempts.to_vec(),
            fifth: None,
            sixth: None,
        });
    }
    let (fourth, fifth, sixth) = match (prepared_before_api, rows.as_slice()) {
        (false, [fourth, fifth]) => (*fourth, *fifth, None),
        (true, [fourth, fifth, sixth]) => (*fourth, *fifth, Some(*sixth)),
        _ => return Err("完整复制标记缺少此窗口要求的精确 Redis 单独接续链".into()),
    };
    if rows.windows(2).any(|pair| {
        !matches!(
            (pair[0]["number"].as_u64(), pair[1]["number"].as_u64()),
            (Some(previous), Some(current)) if previous < current
        ) || pair[0]["result"] == pair[1]["result"]
    }) || sixth.is_some_and(|row| row["result"] == fourth["result"])
        || ledger["active_generation"] != sixth.unwrap_or(fifth)["result"]
    {
        return Err("完整复制标记要求按顺序接续至唯一当前 Redis 代次".into());
    }
    let mut prior = ledger.clone();
    prior["active_generation"] = fourth["result"].clone();
    Ok(Continuation {
        ledger: prior,
        attempts: attempts
            .iter()
            .filter(|row| *row != fifth && Some(*row) != sixth)
            .cloned()
            .collect(),
        fifth: Some(fifth),
        sixth,
    })
}

pub(super) fn link(
    evidence: &mut Evidence<'_>,
    run: &Path,
    initial: &Value,
    previous: &Restart<'_>,
    row: &Value,
    active: &Restart<'_>,
) -> Result<Value> {
    let number = row["number"].as_u64().ok_or("Redis 单独接续缺少阶段号")?;
    let phase = run.join(format!("lifecycle-{number:04}"));
    redis_incident::check_generation(run, &phase, previous, row, active)?;
    let descriptor = &active.generation["incident"];
    require_path(descriptor, &phase.join("redis-incident.json"))?;
    let incident = evidence.read(descriptor, "原 Redis 不完整清理事故")?;
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
        "原 Redis 不完整清理事故",
    )?;
    let prior_phase = run.join(format!(
        "lifecycle-{:04}",
        previous.generation["attempt"]
            .as_u64()
            .ok_or("原 Redis 前驱阶段号无效")?
    ));
    require_path(&incident["failure"], &prior_phase.join("redis/failed.json"))?;
    require_path(
        &incident["runtime"],
        &prior_phase.join("redis/runtime.json"),
    )?;
    let runtime = evidence.read(&incident["runtime"], "前驱 Redis 原运行收据")?;
    let failed = evidence.read(&incident["failure"], "前驱 Redis 原失败收据")?;
    if incident["kind"] != "windows-redis-incomplete-cleanup-incident"
        || !incident["normal_job_exit_proof"].is_null()
        || runtime != previous.generation["redis"]["runtime"]
        || incident["supervisor"] != runtime["supervisor"]
        || incident["process"] != runtime["process"]
        || incident["port"] != runtime["redis"]["port"]
        || failed["cleanup_completed"] != false
        || failed["supervisor"] != runtime["supervisor"]
    {
        return Err("Redis 单独接续必须绑定直接前驱的原不完整清理失败".into());
    }
    let current = &active.generation["redis"]["runtime"];
    check_redis_successor(initial, &current["redis"], &runtime, &active.request)?;
    let runtime_binding = common::binding(&phase.join("redis/runtime.json"))?;
    if evidence.read(&runtime_binding, "后继 Redis 运行收据")? != *current {
        return Err("后继 Redis 运行后像不同于服务账本".into());
    }
    Ok(
        json!({"previous_restart":previous.result,"previous_generation":previous.receipt,
        "previous_runtime":incident["runtime"],"incident":descriptor,
        "current_request":active.generation["redis"]["request"],"current_runtime":runtime_binding}),
    )
}

fn require_path(binding: &Value, path: &Path) -> Result<()> {
    if Path::new(document::text(&binding["path"], "Redis 原接续绑定路径")?) != path {
        return Err("Redis 事故证据路径必须来自同一服务账本的精确前驱".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn succeeded(number: u64) -> Value {
        json!({"number":number,"stage":"fixture-services",
            "mode":"restart-redis","status":"passed","result":{"path":format!("result-{number}")}})
    }

    #[test]
    fn fifth_generation_requires_explicit_complete_window_and_exact_active_predecessor() {
        let rows = [17, 18].map(succeeded);
        let ledger = json!({"active_generation":rows[1]["result"]});
        let prior = split(&ledger, &rows, true, false).unwrap();
        assert_eq!(prior.ledger["active_generation"], rows[0]["result"]);
        assert_eq!(prior.attempts, rows[..1]);
        assert_eq!(prior.fifth, Some(&rows[1]));
        assert!(prior.sixth.is_none());
        assert!(split(&ledger, &rows, false, false).unwrap().fifth.is_none());
        assert!(
            split(
                &json!({"active_generation":rows[0]["result"]}),
                &rows,
                true,
                false
            )
            .is_err()
        );
        let mut wrong_order = rows.clone();
        wrong_order[1]["number"] = json!(16);
        assert!(split(&ledger, &wrong_order, true, false).is_err());
    }

    #[test]
    fn prepared_window_requires_three_ordered_redis_successors_and_selects_the_latest() {
        let rows = [17, 25, 27].map(succeeded);
        let ledger = json!({"active_generation":rows[2]["result"]});
        let prior = split(&ledger, &rows, true, true).unwrap();
        assert_eq!(prior.ledger["active_generation"], rows[0]["result"]);
        assert_eq!(prior.attempts, rows[..1]);
        assert_eq!(prior.fifth, Some(&rows[1]));
        assert_eq!(prior.sixth, Some(&rows[2]));
        assert!(split(&ledger, &rows, true, false).is_err());
        assert!(split(&ledger, &rows, false, true).is_err());
        assert!(split(&ledger, &rows[1..], true, true).is_err());
        assert!(split(&ledger, &[17, 25, 27, 29].map(succeeded), true, true).is_err());
        assert!(split(&ledger, &[25, 17, 27].map(succeeded), true, true).is_err());
        assert!(
            split(
                &json!({"active_generation":rows[1]["result"]}),
                &rows,
                true,
                true
            )
            .is_err()
        );
        let mut reused_result = rows.clone();
        reused_result[0]["result"] = rows[2]["result"].clone();
        assert!(split(&ledger, &reused_result, true, true).is_err());
    }
}
