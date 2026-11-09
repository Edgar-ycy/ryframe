//! 归档完成与历史发布之间断开时，只核对原预留和实际成员。
use super::{
    Frame, PAIR, Producer, Role, claim_path, common, controller, document,
    history::{self, PendingStart},
    reserved_path, state, verify_claim,
};
use crate::Result;
use serde_json::Value;

pub(super) fn resolve(frame: &Frame<'_>) -> Result<Option<(Role, Value)>> {
    let runtime = &frame.runtime.runtime;
    let Some(pending) = history::pending_start(frame.backend, runtime, &frame.contract)? else {
        return Ok(None);
    };
    let authority = original_reservation(frame, &pending)?;
    let observe = || {
        controller::archived_start_bound(
            pending.role,
            &frame.product,
            runtime,
            &frame.runtime.handoff["api_environment"],
            &authority.executor,
            &pending.token,
        )
    };
    frame.check()?;
    let proof = observe()?.ok_or("原启动归档尚无完整可投影证据，保留意图")?;
    let check = || {
        frame.check()?;
        for descriptor in [&pending.binding, &authority.intent, &authority.reservation] {
            common::bound_file(frame.backend, descriptor, "原启动归档授权后像")?;
        }
        if observe()?.as_ref() != Some(&proof) {
            return Err("原启动归档或实际成员在投影期间变化".into());
        }
        Ok(())
    };
    let producer = Producer::new(
        frame.backend,
        runtime,
        &frame.contract,
        pending.role,
        &pending.token,
        &pending.ready_url,
    )?;
    match proof["outcome"].as_str() {
        Some("succeeded") => {
            check()?;
            producer.started(&proof["result"]["identity"])?;
        }
        Some("failed") => producer.failed(
            document::text(&proof["result"]["error_type"], "原启动失败类型")?,
            check,
        )?,
        _ => return Err("原启动归档尚无确定结果".into()),
    }
    frame.check()?;
    Ok(Some((pending.role, proof)))
}

struct Authority {
    executor: Value,
    intent: Value,
    reservation: Value,
}

fn original_reservation(frame: &Frame<'_>, pending: &PendingStart) -> Result<Authority> {
    let mut selected = None;
    for row in state::attempts(&frame.saved)?.iter().filter(|row| {
        row["stage"] == "seed-runtime"
            && row["mode"] == "start"
            && row["status"] == "failed"
            && row["number"]
                .as_u64()
                .is_some_and(|number| number < frame.number)
    }) {
        let number = row["number"].as_u64().ok_or("原启动阶段序号无效")?;
        let path = reserved_path(frame.run, number, pending.role);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        let reservation =
            common::binding(&common::common_file(frame.backend, &path, "原启动预留")?)?;
        let saved = common::read_bound(frame.backend, &reservation, "原启动预留")?;
        if saved["operation_id"] != pending.token {
            continue;
        }
        document::exact(
            &saved,
            &["format_version", "kind", "intent", "role", "operation_id"],
            "原启动预留",
        )?;
        let path = claim_path(frame.run, number, "start", PAIR);
        let intent = common::binding(&common::common_file(frame.backend, &path, "原启动授权")?)?;
        if saved["format_version"] != 1
            || saved["kind"] != "devex-seed-role-start-reservation"
            || saved["intent"] != intent
            || saved["role"] != pending.role.name()
        {
            return Err("原待定历史与原启动授权不同".into());
        }
        let claim = common::read_bound(frame.backend, &intent, "原启动授权")?;
        verify_claim(frame, row, &claim, "start", PAIR)?;
        let executor = common::validate_process_identity(&claim["executor"])?;
        if selected
            .replace(Authority {
                executor,
                intent,
                reservation,
            })
            .is_some()
        {
            return Err("原归档对应多个启动预留，保留原证据".into());
        }
    }
    selected.ok_or_else(|| "原待定历史缺少明确已失败外层阶段的持久预留".into())
}
