//! 同一正式控制器持目标代次，核对业务前置与原 API 退场后启动 seed。
use super::seed_capacity::Guard;
use crate::{
    Result,
    recovery::clone::native::{
        lifecycle::state,
        resources::common,
        seed::{self, capacity, runtime_model, source_registration::identity},
    },
};
use serde_json::Value;

pub(super) fn execute(guard: &Guard<'_, '_>, request: &Value) -> Result<Value> {
    let context = guard.live.context;
    let backend = context.backend;
    let run = context.run;
    let number = context.number;
    let saved = state::load(run, true)?;
    let prior = prerequisite(&saved, number)?;
    let inputs = runtime_model::inputs(backend, run)?;
    let handoff = common::binding(&inputs.runtime.join("handoff.json"))?;
    if common::read_bound(backend, &handoff, "seed 启动交接")? != inputs.handoff {
        return Err("seed 启动交接在读取期间变化".into());
    }
    if prior["status"] != "passed" {
        runtime_model::restart_after_stop(
            backend,
            run,
            &inputs.runtime,
            &inputs.handoff,
            prior,
            number,
        )?;
    }
    let identity = || identity::verified(backend, run, request, Some(number));
    let capacity = || capacity::evidence(backend, run, request, Some(number));
    let proof = identity()?;
    let quota = capacity()?;
    let schedules = guard.full()?;
    require_proofs(&inputs.handoff, &proof, &quota, &schedules)?;
    if state::load(run, true)? != saved {
        return Err("seed 启动前原调度或阶段证据变化".into());
    }
    seed::start_runtime(
        backend,
        run,
        number,
        &handoff,
        &request["api_process"],
        || {
            if guard.full()? != schedules
                || identity()? != proof
                || capacity()? != quota
                || common::read_bound(backend, &handoff, "seed 启动交接")? != inputs.handoff
            {
                return Err("seed 启动期间原调度、身份、容量或交接发生变化".into());
            }
            Ok(())
        },
    )
}

fn require_proofs(
    handoff: &Value,
    identity: &Value,
    capacity: &Value,
    schedules: &[Value],
) -> Result<()> {
    if handoff["identity"] != *identity
        || handoff["capacity"] != *capacity
        || handoff["schedules"].as_array().map(Vec::as_slice) != Some(schedules)
    {
        return Err("seed 交接不同于最新身份、容量或调度验证结果".into());
    }
    Ok(())
}

fn prerequisite(saved: &Value, current: u64) -> Result<&Value> {
    state::attempts(saved)?
        .iter()
        .rfind(|row| {
            row["number"]
                .as_u64()
                .is_some_and(|number| number < current)
                && !matches!(
                    row["mode"].as_str(),
                    Some("stop" | "status" | "recover" | "recover-session")
                )
        })
        .ok_or_else(|| "最新阶段未通过，不能启动 Worker".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn startup_prerequisite_retains_latest_business_failure_across_cleanup() {
        let saved = json!({"attempts":[
            {"number":1,"stage":"seed-runtime","mode":"prepare","status":"passed"},
            {"number":2,"stage":"seed-runtime","mode":"start","status":"failed"},
            {"number":3,"stage":"seed-runtime","mode":"stop","status":"passed"},
            {"number":4,"stage":"seed-runtime","mode":"recover","status":"passed"},
            {"number":5,"stage":"seed-runtime","mode":"start","status":"running"}
        ]});
        assert_eq!(prerequisite(&saved, 5).unwrap()["number"], 2);
        assert_eq!(prerequisite(&saved, 2).unwrap()["number"], 1);
        assert!(prerequisite(&saved, 1).is_err());
    }

    #[test]
    fn startup_handoff_rejects_each_changed_proof() {
        let identity = json!({"verified":"identity"});
        let capacity = json!({"verified":"capacity"});
        let schedules = vec![json!({"verified":"schedule"})];
        let original = json!({"identity":identity,"capacity":capacity,"schedules":schedules});
        require_proofs(&original, &identity, &capacity, &schedules).unwrap();
        for field in ["identity", "capacity", "schedules"] {
            let mut handoff = original.clone();
            handoff[field] = Value::Null;
            assert!(require_proofs(&handoff, &identity, &capacity, &schedules).is_err());
        }
    }
}
