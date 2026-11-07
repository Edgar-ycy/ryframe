//! 既有服务端Lua原子语义暂按授权保留；Rust调用EVAL不代表Lua已替代。
use super::super::super::super::{
    export::transport::Transport, json as document, resources::common,
};
use super::super::super::resume::transition;
use super::super::{failure, receipts};
use super::{claims, window::Window};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;
const SCRIPT: &str = r###"local cursor = '0'
local rounds = 0
repeat
  local page = redis.call('SCAN', cursor, 'MATCH', ARGV[1], 'COUNT', '1000')
  cursor = page[1]
  if #page[2] ~= 0 then
    return redis.error_reply('scope namespace is not empty')
  end
  rounds = rounds + 1
  if rounds > 10000 then
    return redis.error_reply('scope scan exceeded limit')
  end
until cursor == '0'
if redis.call('EXISTS', KEYS[2]) ~= 0 then
  return redis.error_reply('sentinel exists')
end
redis.call('SET', KEYS[1], ARGV[2])
redis.call('SET', KEYS[2], ARGV[3])
return 2"###;

pub(super) struct Input<'a> {
    pub(super) attempt: &'a str,
    pub(super) resume: &'a Path,
    pub(super) started: &'a Path,
    pub(super) generation: &'a Value,
    pub(super) current_generation: &'a Value,
    pub(super) before: &'a Value,
}
pub(super) fn execute<T: Transport>(
    w: &Window<'_, T>,
    input: Input<'_>,
    active: &mut Value,
    trusted: &mut Value,
) -> Result<Value> {
    let Input {
        attempt,
        resume,
        started,
        generation,
        current_generation,
        before,
    } = input;
    let target = w.inputs.observation_dir;
    let intent = transition::execute(
        w.inputs.backend,
        target,
        "marker-intent",
        &json!({"attempt":attempt,"at":chrono::Utc::now().to_rfc3339(),
        "resume":common::binding(resume)?,"started":common::binding(started)?,"restart_generation":generation,"request":w.inputs.request,"before":before}),
    )?;
    let intent_path = target.join(format!("resume-redis-markers-{attempt}.intent.json"));
    claims::unchanged(w, trusted)?;
    receipts::write(&intent_path, &intent)?;
    *trusted = claims::one(w, trusted, &intent_path, Some(&intent))?;
    *active = json!({"id":"redis-marker-rebind","intent":common::binding(&intent_path)?,"restart_generation":generation,
        "confirmation":null,"failure":null,"write_started":true,"unknown_result":true});
    let scope = document::text(&w.inputs.request["target"]["scope_id"], "Redis scope")?;
    let namespace = format!("ryframe:{{{scope}}}:");
    let owner = format!("{namespace}.ryframe-owner");
    let owner_value = format!("ryframe-owner:v1:{scope}:redis");
    let result: Result<(Value, Value)> = (|| {
        let result = w.redis(
            current_generation,
            &[
                "EVAL",
                SCRIPT,
                "2",
                &owner,
                document::text(&w.inputs.request["reset"]["sentinel_key"], "sentinel")?,
                &format!("{namespace}*"),
                &owner_value,
                document::text(&w.inputs.request["reset"]["sentinel_value"], "sentinel值")?,
            ],
        )?;
        if result.as_str() != Some("2") {
            return Err("Redis原子marker重绑结果未确认，禁止重试".into());
        }
        // 标准命令保留字符串响应；确认精确值后写入既有整数收据。
        Ok((json!(2), w.redis_state(current_generation, true, true)?))
    })();
    match result {
        Ok((result, after)) => {
            let value = transition::execute(
                w.inputs.backend,
                target,
                "marker-confirmed",
                &json!({"attempt":attempt,"at":chrono::Utc::now().to_rfc3339(),
                "intent":common::binding(&intent_path)?,"restart_generation":generation,"after":after,"atomic_result":result}),
            )?;
            let path = target.join(format!("resume-redis-markers-{attempt}.confirmed.json"));
            receipts::write(&path, &value)?;
            *trusted = claims::one(w, trusted, &path, Some(&value))?;
            active["confirmation"] = common::binding(&path)?;
            active["unknown_result"] = json!(false);
            common::binding(&path)
        }
        Err(error) => {
            let check = record_failure(w, &input, &intent_path, active, error.as_ref());
            super::super::registration::combine(Err(error), check)
        }
    }
}
fn record_failure<T: Transport>(
    w: &Window<'_, T>,
    input: &Input<'_>,
    intent: &Path,
    active: &mut Value,
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    let record = transition::execute(
        w.inputs.backend,
        w.inputs.observation_dir,
        "marker-failure",
        &json!({"attempt":input.attempt,"at":chrono::Utc::now().to_rfc3339(),
            "intent":common::binding(intent)?,"restart_generation":input.generation,
            "error_type":failure::category(error)}),
    )?;
    let path = w.inputs.observation_dir.join(format!(
        "resume-redis-markers-{}.failure.json",
        input.attempt
    ));
    receipts::write(&path, &record)?;
    active["failure"] = common::binding(&path)?;
    Ok(())
}
