//! 迁移执行驱动器的纯恢复状态机；确认后才前移，并保留未确认写入。
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Session {
    next_index: usize,
    confirmed_writes: u64,
    active_write: Option<String>,
    active_operation: Option<Value>,
    completed: Vec<Value>,
}

pub(super) fn execute(
    backend: &Path,
    target: &Path,
    transition: &str,
    args: &Value,
) -> Result<Value> {
    if transition == "session-start" {
        json_io::exact(args, &["state"], "续作初始状态")?;
        let state = &args["state"];
        let next = state["next_index"].as_u64().ok_or("目标续作状态请求无效")? as usize;
        check(
            state["resumable"] == true
                && next > 0
                && next < 10
                && next % 2 == 1
                && matches!(
                    state["mode"].as_str(),
                    Some("migration" | "migration-successor")
                ),
            "目标续作状态请求无效",
        )?;
        return Ok(serde_json::to_value(Session {
            next_index: next,
            confirmed_writes: u64::from(state["mode"] == "migration-successor"),
            active_write: None,
            active_operation: None,
            completed: Vec::new(),
        })?);
    }
    let fields = match transition {
        "session-operation" => &["session", "operation", "index"][..],
        "session-completed" => &["session", "operation", "completed"][..],
        "session-markers" => &["session"][..],
        "session-markers-completed" => &["session", "confirmation"][..],
        _ => return Err("目标续作状态请求无效".into()),
    };
    json_io::exact(args, fields, "续作状态转换")?;
    let mut session: Session =
        serde_json::from_value(args["session"].clone()).map_err(|_| "目标续作状态请求无效")?;
    match transition {
        "session-operation" => {
            check(
                session.active_operation.is_none()
                    && session.active_write.is_none()
                    && args["index"].as_u64() == Some(session.next_index as u64),
                "目标续作状态转换不能越过未确认操作",
            )?;
            let operation = operation(backend, target, &args["operation"], session.next_index)?;
            session.active_write = operation["write"]
                .as_bool()
                .filter(|write| *write)
                .map(|_| operation["id"].as_str().unwrap_or_default().to_owned());
            session.active_operation = Some(json!({}));
        }
        "session-completed" => complete(backend, target, &mut session, args)?,
        "session-markers" => {
            check(
                session.active_operation.is_none()
                    && session.active_write.is_none()
                    && session.confirmed_writes == 0
                    && session.completed.is_empty(),
                "目标续作状态转换不能越过未确认操作",
            )?;
            session.active_operation = Some(json!({}));
        }
        "session-markers-completed" => {
            check(
                session.active_operation.is_some()
                    && session.active_write.is_none()
                    && session.confirmed_writes == 0
                    && session.completed.is_empty(),
                "目标续作状态转换不能越过未确认操作",
            )?;
            let path = bound(backend, &args["confirmation"])?;
            let receipt = read(backend, &path)?;
            check(
                path.parent() == Some(target)
                    && receipt["kind"] == "devex-clone-redis-marker-rebind-confirmed"
                    && receipt["remote_write_operations"].as_u64() == Some(1)
                    && receipt["unknown_result"] == false,
                "目标续作收据状态与固定前像不同",
            )?;
            session.confirmed_writes = 1;
            session.active_operation = None;
        }
        _ => unreachable!(),
    }
    Ok(serde_json::to_value(session)?)
}

fn operation<'a>(
    backend: &Path,
    target: &Path,
    value: &'a Value,
    index: usize,
) -> Result<&'a Value> {
    let sequence = super::super::resume_prefix::operations(
        json_io::text(&value["command"][0], "迁移程序")?,
        &read(backend, &target.join("request.json"))?,
    )?;
    check(
        sequence["operations"].get(index) == Some(value),
        "目标续作收据状态与固定前像不同",
    )?;
    Ok(value)
}

fn complete(backend: &Path, target: &Path, session: &mut Session, args: &Value) -> Result<()> {
    let operation = operation(backend, target, &args["operation"], session.next_index)?;
    check(
        session.active_operation.is_some()
            && session.active_write.as_deref()
                == operation["write"]
                    .as_bool()
                    .filter(|write| *write)
                    .and_then(|_| operation["id"].as_str()),
        "目标续作状态转换不能越过未确认操作",
    )?;
    let item = &args["completed"];
    json_io::exact(item, &["id", "receipt", "confirmation"], "已完成续作操作")?;
    let confirmation_path = bound(backend, &item["confirmation"])?;
    let confirmation = read(backend, &confirmation_path)?;
    json_io::exact(
        &confirmation,
        super::super::resume_model::OP_CONFIRMED,
        "续作操作 confirmed",
    )?;
    check(
        confirmation_path.parent() == Some(target)
            && confirmation["index"].as_u64() == Some(session.next_index as u64)
            && confirmation["receipt"] == item["receipt"]
            && confirmation["write"] == operation["write"]
            && item["id"] == operation["id"]
            && super::super::resume_prefix::operation_record(backend, target, operation)?
                == item["receipt"],
        "目标续作收据状态与固定前像不同",
    )?;
    session.completed.push(item.clone());
    session.confirmed_writes = session
        .confirmed_writes
        .checked_add(u64::from(operation["write"] == true))
        .ok_or("目标续作状态请求无效")?;
    session.next_index += 1;
    session.active_write = None;
    session.active_operation = None;
    Ok(())
}
