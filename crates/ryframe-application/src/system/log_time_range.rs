use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};

pub(super) type ParsedLogTimeRange = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

pub(super) fn parse_log_time_range(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_and_normalizes_to_utc() {
        let (begin, end) = parse_log_time_range(
            Some(" 2026-08-20T10:00:00+08:00 "),
            Some("2026-08-20T03:00:00Z"),
        )
        .expect("有效时间区间应通过");
        assert_eq!(
            begin.map(|time| time.to_rfc3339()),
            Some("2026-08-20T02:00:00+00:00".into())
        );
        assert_eq!(
            end.map(|time| time.to_rfc3339()),
            Some("2026-08-20T03:00:00+00:00".into())
        );
    }

    #[test]
    fn treats_blank_time_as_absent() {
        assert_eq!(
            parse_log_time_range(Some("  "), None).expect("空值应转为缺省"),
            (None, None)
        );
    }

    #[test]
    fn rejects_invalid_or_reversed_time_range() {
        assert!(matches!(
            parse_log_time_range(Some("2026-08-20"), None),
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            parse_log_time_range(Some("2026-08-20T04:00:00Z"), Some("2026-08-20T03:00:00Z")),
            Err(AppError::Validation(_))
        ));
    }
}
