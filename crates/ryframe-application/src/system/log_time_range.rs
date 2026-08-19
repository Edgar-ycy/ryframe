use chrono::{DateTime, NaiveDateTime, Utc};

const DATE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

pub(super) fn parse_log_time_range(
    begin_time: Option<&str>,
    end_time: Option<&str>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    (
        parse_day_boundary(begin_time, "00:00:00"),
        parse_day_boundary(end_time, "23:59:59"),
    )
}

fn parse_day_boundary(date: Option<&str>, time: &str) -> Option<DateTime<Utc>> {
    date.and_then(|s| {
        NaiveDateTime::parse_from_str(&format!("{s} {time}"), DATE_TIME_FORMAT)
            .ok()
            .map(|d| d.and_utc())
    })
}
