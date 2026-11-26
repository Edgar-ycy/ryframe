use ryframe_kernel::{AppError, AppResult};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

pub type ParsedLogTimeRange = (Option<OffsetDateTime>, Option<OffsetDateTime>);

pub fn parse_log_time_range(
    begin_time: Option<&str>,
    end_time: Option<&str>,
) -> AppResult<ParsedLogTimeRange> {
    let begin_time = parse_rfc3339("begin_time", begin_time)?;
    let end_time = parse_rfc3339("end_time", end_time)?;
    if begin_time
        .zip(end_time)
        .is_some_and(|(begin, end)| begin > end)
    {
        return Err(AppError::Validation(
            "日志筛选开始时间不能晚于结束时间".into(),
        ));
    }
    Ok((begin_time, end_time))
}

fn parse_rfc3339(name: &str, value: Option<&str>) -> AppResult<Option<OffsetDateTime>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let invalid = || AppError::Validation(format!("日志筛选 {name} 必须是包含时区的 RFC3339 时间"));
    if !matches!(value.as_bytes().get(10), Some(b'T' | b't' | b' '))
        || value.get(17..19) == Some("60")
    {
        return Err(invalid());
    }
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|time| time.checked_to_offset(UtcOffset::UTC))
        .filter(|time| (0..=9999).contains(&time.year()))
        .map(Some)
        .ok_or_else(invalid)
}
