//! 已采集创建时间的纯解析与裕量比较，不采集或探测进程。
use crate::{Result, recovery::clone::native::json as document};
use serde_json::Value;
use std::path::Path;
use time::{Date, Month, PrimitiveDateTime};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Creation {
    pub pid: u32,
    pub started: u64,
    pub precision_ticks: u64,
}

pub(crate) fn cim(pid: u32, rows: &[Value]) -> Result<Creation> {
    if pid <= 1 || rows.len() != 1 {
        return Err("CIM 必须唯一返回明确 PID 的创建身份".into());
    }
    document::exact(&rows[0], &["ProcessId", "CreationDate"], "CIM 创建身份")?;
    if rows[0]["ProcessId"].as_u64() != Some(u64::from(pid)) {
        return Err("CIM 返回的 PID 与请求不一致".into());
    }
    let text = document::text(&rows[0]["CreationDate"], "CIM 创建时间")?;
    if !text.is_ascii()
        || text.len() != 25
        || text.as_bytes()[14] != b'.'
        || !matches!(text.as_bytes()[21], b'+' | b'-')
    {
        return Err("CIM 创建时间必须是完整且时区明确的 DMTF 时间".into());
    }
    let year = number(text, 0, 4)?;
    if year == 0 {
        return Err("CIM 创建日期无效".into());
    }
    let month = number(text, 4, 6)? as u8;
    let day = number(text, 6, 8)? as u8;
    let month = Month::try_from(month).map_err(|_| "CIM 创建日期无效")?;
    let date = Date::from_calendar_date(year as i32, month, day).map_err(|_| "CIM 创建日期无效")?;
    let local: PrimitiveDateTime = date
        .with_hms_micro(
            number(text, 8, 10)? as u8,
            number(text, 10, 12)? as u8,
            number(text, 12, 14)? as u8,
            number(text, 15, 21)?,
        )
        .map_err(|_| "CIM 创建时间无效")?;
    let offset = i64::from(number(text, 22, 25)?);
    let offset = if text.as_bytes()[21] == b'-' {
        -offset
    } else {
        offset
    };
    let offset_micros = offset.checked_mul(60_000_000).ok_or("CIM 时区转换溢出")?;
    let epoch = Date::from_calendar_date(1601, Month::January, 1)
        .map(Date::midnight)
        .map_err(|_| "FILETIME 起点无效")?;
    // 直接校正微秒差值，保留时区校正跨出四位年份时的 FILETIME 范围校验。
    let elapsed_micros = (local - epoch)
        .whole_microseconds()
        .checked_sub(i128::from(offset_micros))
        .ok_or("CIM 时区转换溢出")?;
    let ticks = i64::try_from(elapsed_micros)
        .ok()
        .and_then(|n| n.checked_mul(10))
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n > 0 && n.to_string().len() <= 18)
        .ok_or("CIM 创建时间不能转换为正 FILETIME")?;
    Ok(Creation {
        pid,
        started: ticks,
        precision_ticks: 10,
    })
}

pub(crate) fn require_newer(expected: &Value, observed: &Creation) -> Result<()> {
    let (pid, started) = recorded(expected)?;
    if observed.pid != pid || observed.precision_ticks != 10 {
        return Err("观察创建身份的 PID 或 CIM 时间精度无效".into());
    }
    let boundary = started
        .checked_add(observed.precision_ticks)
        .ok_or("创建时间裕量溢出")?;
    if observed.started <= boundary {
        return Err("观察创建时间不能证明为裕量之后的新代次".into());
    }
    Ok(())
}

pub(super) fn recorded(expected: &Value) -> Result<(u32, u64)> {
    document::exact(expected, &["pid", "started", "executable"], "历史进程身份")?;
    let pid = expected["pid"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 1)
        .ok_or("历史 PID 无效")?;
    let text = document::text(&expected["started"], "历史创建时间")?;
    if text.is_empty()
        || text.len() > 18
        || text.starts_with('0')
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("历史创建时间必须为明确 ASCII 正整数".into());
    }
    let started = text.parse::<u64>().map_err(|_| "历史创建时间溢出")?;
    let executable = document::text(&expected["executable"], "历史可执行路径")?;
    if !Path::new(executable).is_absolute() {
        return Err("历史可执行路径必须为绝对路径".into());
    }
    Ok((pid, started))
}

fn number(text: &str, start: usize, end: usize) -> Result<u32> {
    text.get(start..end)
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "CIM 创建时间含未知或无效数字字段".into())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/source_registration/quiet/timing.rs"]
mod tests;
