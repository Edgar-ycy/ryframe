use jiff::{
    SignedDuration, Timestamp,
    civil::{DateTime, Time, date, time},
    tz::{AmbiguousOffset, TimeZone},
};
use jiff_cron::{Schedule, TimeUnitSpec};
use ryframe_kernel::{AppError, AppResult};
use time::OffsetDateTime;

pub(super) struct Calendar {
    pub(super) schedule: Schedule,
    pub(super) timezone: TimeZone,
    pub(super) name: &'static str,
}

pub(super) struct Occurrence {
    pub(super) local: DateTime,
    pub(super) first: OffsetDateTime,
    pub(super) repeated: Option<OffsetDateTime>,
}

pub(super) fn resolve_timezone(name: &str) -> AppResult<(&'static str, TimeZone)> {
    // 2025b 保留所有旧名称。旧入口区分大小写，且没有 Factory 条目。
    let (canonical, tzif) = jiff_tzdb::get(name)
        .filter(|(canonical, _)| *canonical == name && name != "Factory")
        .ok_or_else(|| AppError::Validation("时区必须是有效的 IANA 时区名称".into()))?;
    let timezone = TimeZone::tzif(canonical, tzif).map_err(calendar_error)?;
    Ok((canonical, timezone))
}

impl Calendar {
    pub(super) fn start_after(&self, after: i64) -> Option<DateTime> {
        // 从 UTC 下一整秒换算当地时间。跨回拨边界时，当地时刻可能向前回退。
        let next = Timestamp::from_second(after.checked_add(1)?).ok()?;
        Some(self.timezone.to_datetime(next))
    }

    pub(super) fn first_after(&self, after: i64) -> AppResult<Option<Occurrence>> {
        let Some(start) = self.start_after(after) else {
            return Ok(None);
        };
        self.first_from(start)
    }

    pub(super) fn first_from(&self, mut start: DateTime) -> AppResult<Option<Occurrence>> {
        while let Some(local) = self.next_local(start) {
            let candidate = self.timezone.to_ambiguous_timestamp(local);
            let (first, repeated) = match candidate.offset() {
                AmbiguousOffset::Gap { .. } => {
                    start = next_local_second(local)?;
                    continue;
                }
                AmbiguousOffset::Unambiguous { .. } => (candidate.unambiguous(), None),
                AmbiguousOffset::Fold { .. } => (candidate.earlier(), Some(candidate.later())),
            };
            return Ok(Some(Occurrence {
                local,
                first: utc_time(first)?,
                repeated: repeated.map(utc_time).transpose()?,
            }));
        }
        Ok(None)
    }

    pub(super) fn format_local(&self, utc: OffsetDateTime) -> AppResult<String> {
        let instant = Timestamp::from_second(utc.unix_timestamp()).map_err(calendar_error)?;
        let local = self.timezone.to_datetime(instant);
        let offset = self.timezone.to_offset(instant).seconds();
        let sign = if offset < 0 { '-' } else { '+' };
        // 预览的 RFC 3339 字段保留按分钟取整的偏移量和原当地时分秒。
        let minutes = (offset.unsigned_abs() + 30) / 60;
        Ok(format!(
            "{local}{sign}{:02}:{:02}",
            minutes / 60,
            minutes % 60
        ))
    }

    fn next_local(&self, start: DateTime) -> Option<DateTime> {
        let start_year = u32::try_from(start.year()).ok()?;
        for year in self.schedule.years().range(start_year..) {
            let first_month = if year == start_year {
                start.month() as u32
            } else {
                1
            };
            for month in self.schedule.months().range(first_month..) {
                let first = date(year as i16, month as i8, 1);
                let first_day = if year == start_year && month == start.month() as u32 {
                    start.day() as u32
                } else {
                    1
                };
                for day in self
                    .schedule
                    .days_of_month()
                    .range(first_day..=first.days_in_month() as u32)
                {
                    let date = date(year as i16, month as i8, day as i8);
                    if !self
                        .schedule
                        .days_of_week()
                        .includes(date.weekday().to_sunday_one_offset() as u32)
                    {
                        continue;
                    }
                    let earliest = if date == start.date() {
                        start.time()
                    } else {
                        Time::MIN
                    };
                    if let Some(time) = self.next_minute(earliest) {
                        return Some(DateTime::from_parts(date, time));
                    }
                }
            }
        }
        None
    }

    fn next_minute(&self, earliest: Time) -> Option<Time> {
        for hour in self.schedule.hours().range(earliest.hour() as u32..) {
            let first_minute = if hour == earliest.hour() as u32 {
                earliest.minute() as u32
            } else {
                0
            };
            for minute in self.schedule.minutes().range(first_minute..) {
                let candidate = time(hour as i8, minute as i8, 0, 0);
                if candidate >= earliest {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

pub(super) fn next_local_second(local: DateTime) -> AppResult<DateTime> {
    local
        .checked_add(SignedDuration::from_secs(1))
        .map_err(calendar_error)
}

fn utc_time(timestamp: Result<Timestamp, jiff::Error>) -> AppResult<OffsetDateTime> {
    let timestamp = timestamp.map_err(calendar_error)?;
    OffsetDateTime::from_unix_timestamp(timestamp.as_second()).map_err(calendar_error)
}

fn calendar_error(error: impl std::fmt::Display) -> AppError {
    AppError::Validation(format!("无法计算 Cron 执行时间（{error}）"))
}
