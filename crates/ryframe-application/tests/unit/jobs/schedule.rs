use chrono::{DateTime, Utc};

use super::ParsedSchedule;

fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn preview_utc(value: &str) -> ::time::OffsetDateTime {
    ::time::OffsetDateTime::parse(value, &::time::format_description::well_known::Rfc3339).unwrap()
}

#[test]
fn schedule_preserves_calendar_fields_at_whole_and_fractional_boundaries() {
    let tuesday = ParsedSchedule::parse("0 0 0 * * Tue *", "UTC").unwrap();
    for start in ["2026-11-22T23:59:59Z", "2026-11-22T23:59:59.500Z"] {
        assert_eq!(
            tuesday.next_after(utc(start)).unwrap(),
            utc("2026-11-24T00:00:00Z")
        );
    }

    let midnight = ParsedSchedule::parse("0 0 0 * * * *", "UTC").unwrap();
    assert_eq!(
        midnight
            .next_after(utc("1969-12-31T23:59:59.500Z"))
            .unwrap(),
        utc("1970-01-01T00:00:00Z")
    );
    assert_eq!(
        midnight.next_after(utc("1970-01-01T00:00:00Z")).unwrap(),
        utc("1970-01-02T00:00:00Z")
    );
    assert!(midnight.next_after(utc("2100-12-31T00:00:00Z")).is_err());

    let numeric_weekday = ParsedSchedule::parse("0 0 0 * * 1 *", "UTC").unwrap();
    assert_eq!(
        numeric_weekday
            .next_after(utc("2026-11-22T00:00:00Z"))
            .unwrap(),
        utc("2026-11-29T00:00:00Z")
    );
    let regular = ParsedSchedule::parse("0 10-40/15 8,12 ? Jan-Mar Mon-Fri *", "UTC");
    assert!(regular.is_err()); // 日期和星期字段同时受限。
    assert!(ParsedSchedule::parse("0 0 0 ? * * *", "UTC").is_ok());
    for invalid in ["0 0 * * * *", "1 0 0 * * * *", "0 0 0 * * * 2030"] {
        assert!(ParsedSchedule::parse(invalid, "UTC").is_err());
    }
    let regular = ParsedSchedule::parse("0 10-40/15 8,12 * Jan-Mar Mon-Fri *", "UTC").unwrap();
    let next = regular
        .future_occurrences(utc("2026-01-05T08:10:00Z"), 3)
        .unwrap();
    assert_eq!(
        next.into_iter()
            .map(|occurrence| occurrence.utc)
            .collect::<Vec<_>>(),
        [
            "2026-01-05T08:25:00Z",
            "2026-01-05T08:40:00Z",
            "2026-01-05T12:10:00Z"
        ]
        .map(preview_utc)
    );
}

#[test]
fn preview_preserves_fold_order_and_execution_rejects_old_utc_occurrences() {
    let schedule = ParsedSchedule::parse("0 0,30 1 * * * *", "America/Chicago").unwrap();
    let preview = schedule
        .future_occurrences(utc("2022-11-06T05:59:59Z"), 4)
        .unwrap();
    let actual = preview
        .iter()
        .map(|occurrence| (occurrence.utc, occurrence.schedule_time.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            (
                preview_utc("2022-11-06T06:00:00Z"),
                "2022-11-06T01:00:00-05:00"
            ),
            (
                preview_utc("2022-11-06T07:00:00Z"),
                "2022-11-06T01:00:00-06:00"
            ),
            (
                preview_utc("2022-11-06T06:30:00Z"),
                "2022-11-06T01:30:00-05:00"
            ),
            (
                preview_utc("2022-11-06T07:30:00Z"),
                "2022-11-06T01:30:00-06:00"
            ),
        ]
    );
    for (start, expected) in [
        ("2022-11-06T06:00:00Z", "2022-11-06T06:30:00Z"),
        ("2022-11-06T06:59:59.500Z", "2022-11-06T07:00:00Z"),
        ("2022-11-06T07:00:00Z", "2022-11-06T07:30:00Z"),
        ("2022-11-06T07:30:00Z", "2022-11-07T07:00:00Z"),
    ] {
        assert_eq!(schedule.next_after(utc(start)).unwrap(), utc(expected));
    }
}

#[test]
fn scheduling_skips_gaps_and_keeps_legacy_timezone_names_and_historical_offsets() {
    let gap = ParsedSchedule::parse("0 30 2 * * * *", "US/Central").unwrap();
    assert_eq!(gap.timezone_name(), "US/Central");
    let start = utc("2024-03-09T08:30:00Z");
    let expected = utc("2024-03-11T07:30:00Z");
    assert_eq!(gap.next_after(start).unwrap(), expected);
    assert_eq!(
        gap.future_occurrences(start, 1).unwrap()[0].utc,
        preview_utc("2024-03-11T07:30:00Z")
    );

    for invalid_name in ["us/central", "Factory", "+05:00"] {
        assert!(ParsedSchedule::parse("0 30 2 * * * *", invalid_name).is_err());
    }
    assert!(ParsedSchedule::parse("0 0 0 * * * *", "America/Coyhaique").is_ok());

    let historic = ParsedSchedule::parse("0 0 5 * * * *", "Africa/Monrovia").unwrap();
    let result = historic
        .future_occurrences(utc("1970-01-02T00:00:00Z"), 1)
        .unwrap();
    assert_eq!(result[0].utc, preview_utc("1970-01-02T05:44:30Z"));
    assert_eq!(result[0].schedule_time, "1970-01-02T05:00:00-00:45");
}
