use jiff_cron::Schedule;

use super::calendar::{Calendar, next_local_second, resolve_timezone};
use super::*;

pub(super) fn validate_persisted_schedule(
    next_run_at: Option<OffsetDateTime>,
    misfire_policy: &str,
    concurrency_policy: &str,
    max_runtime_seconds: i32,
    cron_expression: &str,
    timezone: &str,
    now: OffsetDateTime,
) -> Result<ParsedSchedule, String> {
    if next_run_at.is_none() {
        return Err("已启用计划缺少 next_run_at".into());
    }
    if !matches!(misfire_policy, MISFIRE_SKIP | MISFIRE_FIRE_ONCE) {
        return Err("错过执行策略只能是 skip 或 fire_once".into());
    }
    if !matches!(concurrency_policy, CONCURRENCY_FORBID | CONCURRENCY_ALLOW) {
        return Err("并发策略只能是 forbid 或 allow".into());
    }
    if !(1..=86_400).contains(&max_runtime_seconds) {
        return Err("最大运行时长必须在 1 到 86400 秒之间".into());
    }
    let parsed = ParsedSchedule::parse(cron_expression, timezone)
        .map_err(|error| error.message().to_owned())?;
    parsed
        .next_after(now)
        .map_err(|error| error.message().to_owned())?;
    Ok(parsed)
}

pub(super) struct ValidatedScheduleCommand {
    pub(super) name: String,
    pub(super) handler_key: String,
    pub(super) cron_expression: String,
    pub(super) timezone: String,
    pub(super) enabled: bool,
    pub(super) misfire_policy: String,
    pub(super) concurrency_policy: String,
    pub(super) max_runtime_seconds: i32,
    pub(super) parsed: ParsedSchedule,
}

pub(super) struct ParsedSchedule {
    pub(super) expression: String,
    calendar: Calendar,
}

impl ParsedSchedule {
    pub(super) fn parse(expression: &str, timezone: &str) -> AppResult<Self> {
        let expression = normalize_required(expression, MAX_CRON_BYTES, "Cron 表达式")?;
        let fields = expression.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 7 {
            return Err(AppError::Validation(
                "Cron 表达式必须包含秒、分、时、日、月、周、年七段".into(),
            ));
        }
        if fields[0] != "0" {
            return Err(AppError::Validation(
                "Cron 秒字段首版只允许 0，最小执行间隔为一分钟".into(),
            ));
        }
        if fields[6] != "*" {
            return Err(AppError::Validation("Cron 年字段首版只允许 *".into()));
        }
        let timezone = normalize_required(timezone, MAX_TIMEZONE_BYTES, "时区")?;
        let (name, timezone) = resolve_timezone(&timezone)?;
        if fields[3] != "*" && fields[5] != "*" {
            return Err(AppError::Validation(
                "Cron 日期字段和星期字段不能同时受限，其中一项必须为 *".into(),
            ));
        }
        let schedule = expression
            .parse::<Schedule>()
            .map_err(|error| AppError::Validation(format!("Cron 表达式无效: {error}")))?;
        Ok(Self {
            expression,
            calendar: Calendar {
                schedule,
                timezone,
                name,
            },
        })
    }

    pub(super) fn timezone_name(&self) -> &'static str {
        self.calendar.name
    }

    pub(super) fn next_after(&self, after: OffsetDateTime) -> AppResult<OffsetDateTime> {
        let cutoff = after.unix_timestamp();
        if let Some(mut start) = self.calendar.start_after(cutoff) {
            while let Some(occurrence) = self.calendar.first_from(start)? {
                if let Some(utc) = std::iter::once(occurrence.first)
                    .chain(occurrence.repeated)
                    .find(|utc| utc.unix_timestamp() > cutoff)
                {
                    return Ok(utc);
                }
                start = next_local_second(occurrence.local)?;
            }
        }
        Err(AppError::Validation("Cron 表达式没有未来执行时间".into()))
    }

    pub(super) fn future_occurrences(
        &self,
        after: OffsetDateTime,
        count: usize,
    ) -> AppResult<Vec<JobScheduleOccurrence>> {
        let mut previous = after.unix_timestamp();
        let mut repeated = None;
        let mut occurrences = Vec::with_capacity(count);
        for _ in 0..count {
            let utc = if let Some(utc) = repeated.take() {
                utc
            } else {
                let occurrence = self.calendar.first_after(previous)?.ok_or_else(|| {
                    AppError::Validation("Cron 表达式无法产生足够的未来执行时间".into())
                })?;
                // 预览沿用同一迭代器的顺序，重复的当地时刻先输出较早的 UTC，再输出较晚的 UTC。
                repeated = occurrence.repeated;
                occurrence.first
            };
            previous = utc.unix_timestamp();
            occurrences.push(JobScheduleOccurrence {
                utc,
                schedule_time: self.calendar.format_local(utc)?,
            });
        }
        Ok(occurrences)
    }
}
