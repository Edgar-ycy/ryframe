use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};

pub type ParsedLogTimeRange = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

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

fn parse_rfc3339(name: &str, value: Option<&str>) -> AppResult<Option<DateTime<Utc>>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(value)
        .map(|time| Some(time.with_timezone(&Utc)))
        .map_err(|_| AppError::Validation(format!("日志筛选 {name} 必须是包含时区的 RFC3339 时间")))
}
