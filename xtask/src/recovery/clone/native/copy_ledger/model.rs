//! 复制账本事件的纯投影；不执行资源操作，也不补写缺失历史。
use crate::{
    Result,
    recovery::clone::native::{input, json as document},
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
#[path = "model/step.rs"]
mod step;

#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub steps: Map<String, Value>,
    pub session: Option<Session>,
}
#[derive(Debug)]
pub(crate) struct Session {
    pub id: String,
    pub mode: String,
    pub outcome: String,
    pub completed_at: Value,
    pub touched: BTreeSet<String>,
}

pub(crate) fn project(events: &[Value]) -> Result<Projection> {
    let mut state = Projection::default();
    let mut sessions = BTreeSet::new();
    for (index, event) in events.iter().enumerate() {
        document::object(event, "账本事件")?;
        let kind = document::text(&event["type"], "账本事件类型")?;
        if index == 0 {
            document::exact(
                event,
                &["type", "plan_sha256", "generation_sha256"],
                "账本创建绑定",
            )?;
            if kind != "created" {
                return Err("账本缺少创建绑定".into());
            }
            input::digest_shape(&event["plan_sha256"])?;
            input::digest_shape(&event["generation_sha256"])?;
            continue;
        }
        match kind {
            "opened" => opened(&mut state, event, &mut sessions)?,
            "intent" | "confirmed" | "reconciled" | "resumed" => step::apply(&mut state, event)?,
            "closed" => closed(&mut state, event)?,
            "fault" => {
                document::exact(event, &["type", "stage"], "账本失败事件")?;
                if !matches!(
                    event["stage"].as_str(),
                    Some("lock_release" | "receipt_write")
                ) {
                    return Err("未知账本失败事件".into());
                }
                state.session.as_mut().ok_or("未知账本失败事件")?.outcome = "failed".into();
            }
            _ => return Err("未知账本事件，拒绝忽略或兼容解码".into()),
        }
    }
    Ok(state)
}

fn opened(state: &mut Projection, event: &Value, sessions: &mut BTreeSet<String>) -> Result<()> {
    document::exact(event, &["type", "session", "mode"], "账本打开事件")?;
    let id = input::name(document::text(&event["session"], "账本会话")?)?;
    let mode = document::text(&event["mode"], "账本会话模式")?;
    if !matches!(mode, "apply" | "reconcile" | "resume") || sessions.contains(id) {
        return Err("账本会话类型无效或重复".into());
    }
    if state
        .session
        .as_ref()
        .is_some_and(|previous| previous.outcome != "partial")
        && mode != "reconcile"
    {
        return Err("未完成或已封存账本只能显式核对".into());
    }
    sessions.insert(id.to_owned());
    state.session = Some(Session {
        id: id.to_owned(),
        mode: mode.to_owned(),
        outcome: "open".into(),
        touched: BTreeSet::new(),
        completed_at: Value::Null,
    });
    Ok(())
}

fn closed(state: &mut Projection, event: &Value) -> Result<()> {
    document::exact(event, &["type", "outcome", "completed_at"], "账本关闭事件")?;
    let session = state
        .session
        .as_mut()
        .filter(|session| session.outcome == "open")
        .ok_or("未知或重复的会话关闭事件")?;
    let outcome = document::text(&event["outcome"], "账本收尾状态")?;
    match outcome {
        "publishing" => zoned_instant(document::text(&event["completed_at"], "账本完成时间")?)?,
        "partial" | "failed" if event["completed_at"].is_null() => {}
        _ => return Err("未知关闭状态或非完成步骤声明了完成时间".into()),
    }
    session.outcome = outcome.to_owned();
    session.completed_at = event["completed_at"].clone();
    Ok(())
}

fn image(value: &Value) -> Result<Value> {
    document::object(value, "账本前后像")?;
    match value["kind"].as_str() {
        Some("database") => {
            let fields = document::exact(
                value,
                &[
                    "kind",
                    "resource_sha256",
                    "schema_sha256",
                    "tables_sha256",
                    "preserved_sha256",
                ],
                "数据库前后像",
            )?;
            for (key, digest) in fields {
                if key != "kind" {
                    input::digest_shape(digest)?;
                }
            }
        }
        Some("object") => {
            document::exact(
                value,
                &[
                    "kind",
                    "resource_sha256",
                    "exists",
                    "bytes",
                    "sha256",
                    "metadata_sha256",
                ],
                "对象前后像",
            )?;
            input::digest_shape(&value["resource_sha256"])?;
            match value["exists"].as_bool() {
                Some(true) => {
                    if value["bytes"].as_u64().is_none() {
                        return Err("对象长度无效".into());
                    }
                    input::digest_shape(&value["sha256"])?;
                    input::digest_shape(&value["metadata_sha256"])?;
                }
                Some(false)
                    if ["bytes", "sha256", "metadata_sha256"]
                        .iter()
                        .all(|key| value[*key].is_null()) => {}
                _ => return Err("对象存在性或不存在时的内容摘要无效".into()),
            }
        }
        _ => return Err("账本只支持明确数据库或对象步骤".into()),
    }
    Ok(value.clone())
}

fn image_pair(before: &Value, after: &Value) -> Result<()> {
    image(before)?;
    image(after)?;
    let mut stable = vec!["kind", "resource_sha256"];
    if before["kind"] == "database" {
        stable.extend(["schema_sha256", "preserved_sha256"]);
    }
    if before == after || stable.iter().any(|key| before[*key] != after[*key]) {
        return Err("前后像身份、schema、保留数据不同或无法唯一判定".into());
    }
    if before["kind"] == "object" && (before["exists"] != false || after["exists"] != true) {
        return Err("对象步骤必须从明确不存在到条件创建，不允许覆盖".into());
    }
    Ok(())
}

// Python fromisoformat 接受基本/扩展日历与周日期、单字符分隔符及显式偏移。
fn zoned_instant(text: &str) -> Result<()> {
    let end = date_end(text).ok_or("账本完成日期无效")?;
    let rest = text.get(end..).ok_or("账本完成时间无效")?;
    let separator = rest.chars().next().ok_or("账本完成时间必须包含时区")?;
    let time = &rest[separator.len_utf8()..];
    let offset = time
        .find(['+', '-', 'Z'])
        .ok_or("账本完成时间必须包含时区")?;
    let clock = clock_parts(&time[..offset]).ok_or("账本完成时间无效")?;
    if clock[0] > 23 || clock[1] > 59 || clock[2] > 59 {
        return Err("账本完成时间无效".into());
    }
    let zone = &time[offset..];
    if zone != "Z" {
        let parts = clock_parts(&zone[1..]).ok_or("账本完成时区无效")?;
        if parts[0] * 3600 + parts[1] * 60 + parts[2] >= 86400 {
            return Err("账本完成时区无效".into());
        }
    }
    Ok(())
}
fn digits(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}
fn date_end(text: &str) -> Option<usize> {
    let year = digits(text.get(..4)?)?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let (week_start, end) = if text.get(4..6) == Some("-W") {
        let day =
            text.get(8..9) == Some("-") && !text.as_bytes().get(10).is_some_and(u8::is_ascii_digit);
        (6, if day { 10 } else { 8 })
    } else if text.get(4..5) == Some("W") {
        let mut index = 7;
        while text.as_bytes().get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        (
            5,
            if index < 9 {
                index
            } else if index % 2 == 0 {
                7
            } else {
                8
            },
        )
    } else {
        (0, 0)
    };
    if week_start != 0 {
        let week = digits(text.get(week_start..week_start + 2)?)?;
        let day = if end == week_start + 2 {
            1
        } else {
            digits(text.get(end - 1..end)?)?
        };
        let weekday = match day {
            1 => chrono::Weekday::Mon,
            2 => chrono::Weekday::Tue,
            3 => chrono::Weekday::Wed,
            4 => chrono::Weekday::Thu,
            5 => chrono::Weekday::Fri,
            6 => chrono::Weekday::Sat,
            7 => chrono::Weekday::Sun,
            _ => return None,
        };
        let date = chrono::NaiveDate::from_isoywd_opt(year as i32, week, weekday)?;
        if !(1..=9999).contains(&chrono::Datelike::year(&date)) {
            return None;
        }
        return Some(end);
    }
    let (month, day, end) = if text.get(4..5) == Some("-") && text.get(7..8) == Some("-") {
        (digits(text.get(5..7)?)?, digits(text.get(8..10)?)?, 10)
    } else {
        (digits(text.get(4..6)?)?, digits(text.get(6..8)?)?, 8)
    };
    chrono::NaiveDate::from_ymd_opt(year as i32, month, day)?;
    Some(end)
}
fn clock_parts(text: &str) -> Option<[u32; 3]> {
    let mut split = text.split(['.', ',']);
    let main = split.next()?;
    let fractional = split.clone().next().is_some();
    if let Some(fraction) = split.next()
        && (fraction.is_empty()
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || split.next().is_some())
    {
        return None;
    }
    let parts = if main.contains(':') {
        main.split(':').collect::<Vec<_>>()
    } else {
        match main.len() {
            2 => vec![main],
            4 => vec![main.get(..2)?, main.get(2..4)?],
            6 => vec![main.get(..2)?, main.get(2..4)?, main.get(4..6)?],
            _ => return None,
        }
    };
    if parts.is_empty() || parts.len() > 3 || (fractional && parts.len() != 3) {
        return None;
    }
    let mut result = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return None;
        }
        result[index] = digits(part)?;
    }
    Some(result)
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_ledger/model.rs"]
mod tests;
