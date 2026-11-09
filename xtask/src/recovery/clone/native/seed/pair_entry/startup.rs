//! 仅由同进程的正式阶段消费 seed 双角色启动授权。
use super::{Frame, PAIR, control};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    run: &Path,
    number: u64,
    handoff: &Value,
    original_api: &Value,
    finish: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    let frame = Frame::load(
        backend,
        run,
        "start",
        PAIR,
        number,
        std::process::id(),
        Some(handoff),
    )?;
    finish_start(
        control(&frame, "start", PAIR)?,
        handoff,
        original_api,
        || {
            finish()?;
            frame.check()
        },
    )
}

fn finish_start(
    result: Value,
    handoff: &Value,
    original_api: &Value,
    finish: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    let authority = &result["authority"];
    if authority["handoff"] != *handoff
        || authority["original_api"] != *original_api
        || authority["remote_writes"] != 0
    {
        return Err("seed 启动授权不同于原交接或进程收据".into());
    }
    finish()?;
    Ok(
        json!({"status":"seed_runtime_started","runtime":result["runtime"],"handoff":handoff,
        "retirement":authority["retirement"],"outbox_drained":false,"restore_qualified":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_receipt_requires_same_authority_and_post_start_guard() {
        let handoff = json!({"sha256":"handoff"});
        let api = json!({"sha256":"api"});
        let authority = json!({"handoff":handoff,"original_api":api,
            "retirement":{"stage":"original"},"remote_writes":0});
        let control = json!({"authority":authority,"runtime":{"operation":"start"}});
        let mut changed = control.clone();
        changed["authority"]["original_api"] = Value::Null;
        assert!(
            finish_start(changed, &handoff, &api, || panic!(
                "错误授权不能进入后置核验"
            ))
            .is_err()
        );
        assert!(
            finish_start(control.clone(), &handoff, &api, || Err(
                "后置调度变化".into()
            ))
            .is_err()
        );
        let receipt = finish_start(control, &handoff, &api, || Ok(())).unwrap();
        assert_eq!(receipt["status"], "seed_runtime_started");
        assert_eq!(receipt["retirement"], authority["retirement"]);
        assert_eq!(receipt["outbox_drained"], false);
        assert_eq!(receipt["restore_qualified"], false);
    }
}
