use super::{Projection, Session, image, image_pair};
use crate::{
    Result,
    recovery::clone::native::{input, json as document},
};
use serde_json::{Map, Value, json};

pub(super) fn apply(state: &mut Projection, event: &Value) -> Result<()> {
    let session = state
        .session
        .as_mut()
        .filter(|session| session.outcome == "open")
        .ok_or("步骤事件不在持锁执行会话内")?;
    let id = input::name(document::text(&event["step_id"], "账本步骤编号")?)?;
    if event["type"] == "intent" {
        intent(&mut state.steps, session, event, id)?;
    } else {
        observe(&mut state.steps, session, event, id)?;
    }
    session.touched.insert(id.to_owned());
    Ok(())
}
fn intent(
    steps: &mut Map<String, Value>,
    session: &Session,
    event: &Value,
    id: &str,
) -> Result<()> {
    document::exact(
        event,
        &["type", "step_id", "before", "after", "artifact_sha256"],
        "账本写入意图",
    )?;
    if session.mode == "reconcile" || steps.contains_key(id) {
        return Err("核对会话不能写资源；重复步骤不能自动重放".into());
    }
    if steps.values().any(|step| step["phase"] != "confirmed") {
        return Err("既有步骤未确认，禁止开始下一资源写入".into());
    }
    image_pair(&event["before"], &event["after"])?;
    input::digest_shape(&event["artifact_sha256"])?;
    if steps
        .values()
        .any(|step| step["before"]["resource_sha256"] == event["before"]["resource_sha256"])
    {
        return Err("不同步骤重复引用同一物理资源".into());
    }
    let mut step = document::object(event, "账本写入意图")?.clone();
    step.insert("phase".into(), json!("unknown"));
    step.insert("attempt".into(), json!(1));
    step.insert("session".into(), json!(session.id));
    steps.insert(id.to_owned(), Value::Object(step));
    Ok(())
}
fn observe(
    steps: &mut Map<String, Value>,
    session: &Session,
    event: &Value,
    id: &str,
) -> Result<()> {
    document::exact(event, &["type", "step_id", "observed"], "账本步骤观察")?;
    let step = steps.get(id).ok_or("没有本账本 intent，不能接管已有资源")?;
    let observed = image(&event["observed"])?;
    let mut attempt = None;
    let phase = match event["type"].as_str() {
        Some("confirmed") => {
            if session.mode == "reconcile"
                || step["phase"] != "unknown"
                || step["session"] != session.id
                || observed != step["after"]
            {
                return Err("确认必须对应本次写入 intent 和完整预期后像".into());
            }
            "confirmed"
        }
        Some("reconciled") => {
            if session.mode != "reconcile" || session.touched.contains(id) {
                return Err("只允许独立显式核对，不重复消费同一观察".into());
            }
            if observed == step["after"] {
                "confirmed"
            } else if observed == step["before"] {
                "reconciled_before"
            } else {
                "mismatch"
            }
        }
        Some("resumed") => {
            if session.mode != "resume"
                || step["phase"] != "reconciled_before"
                || observed != step["before"]
            {
                return Err("显式续跑必须重新证明同代次完整前像".into());
            }
            if steps.iter().any(|(key, other)| {
                key != id && matches!(other["phase"].as_str(), Some("unknown" | "mismatch"))
            }) {
                return Err("其他步骤结果未知或不匹配，禁止开始下一次续跑".into());
            }
            attempt = Some(
                step["attempt"]
                    .as_u64()
                    .and_then(|n| n.checked_add(1))
                    .ok_or("账本步骤尝试次数溢出")?,
            );
            "unknown"
        }
        _ => return Err("未知账本步骤事件".into()),
    };
    let step = steps.get_mut(id).ok_or("账本步骤缺失")?;
    step["phase"] = json!(phase);
    if event["type"] != "confirmed" {
        step["session"] = json!(session.id);
    }
    if let Some(attempt) = attempt {
        step["attempt"] = json!(attempt);
    }
    Ok(())
}
