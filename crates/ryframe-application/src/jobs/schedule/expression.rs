use super::*;

pub(super) fn validate_persisted_schedule(
    next_run_at: Option<DateTime<Utc>>,
    misfire_policy: &str,
    concurrency_policy: &str,
    max_runtime_seconds: i32,
    cron_expression: &str,
    timezone: &str,
    now: DateTime<Utc>,
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
    pub(super) timezone: Tz,
    schedule: Schedule,
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
        let timezone = normalize_required(timezone, MAX_TIMEZONE_BYTES, "时区")?
            .parse::<Tz>()
            .map_err(|_| AppError::Validation("时区必须是有效的 IANA 时区名称".into()))?;
        if fields[3] != "*" && fields[5] != "*" {
            return Err(AppError::Validation(
                "Cron 日期字段和星期字段不能同时受限，其中一项必须为 *".into(),
            ));
        }
        let schedule = Schedule::from_str(&expression)
            .map_err(|error| AppError::Validation(format!("Cron 表达式无效: {error}")))?;
        Ok(Self {
            expression,
            timezone,
            schedule,
        })
    }

    pub(super) fn next_after(&self, after: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
        self.schedule
            .after(&after.with_timezone(&self.timezone))
            .next()
            .map(|date| date.with_timezone(&Utc))
            .ok_or_else(|| AppError::Validation("Cron 表达式没有未来执行时间".into()))
    }

    pub(super) fn future_occurrences(
        &self,
        after: DateTime<Utc>,
        count: usize,
    ) -> AppResult<Vec<DateTime<Utc>>> {
        let occurrences = self
            .schedule
            .after(&after.with_timezone(&self.timezone))
            .take(count)
            .map(|date| date.with_timezone(&Utc))
            .collect::<Vec<_>>();
        if occurrences.len() != count {
            return Err(AppError::Validation(
                "Cron 表达式无法产生足够的未来执行时间".into(),
            ));
        }
        Ok(occurrences)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{CONCURRENCY_FORBID, MISFIRE_SKIP, validate_persisted_schedule};

    #[test]
    fn persisted_schedule_validation_uses_application_values() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 0, 0, 0).unwrap();
        let next_run_at = Utc.with_ymd_and_hms(2026, 8, 22, 0, 0, 0).unwrap();

        assert!(
            validate_persisted_schedule(
                Some(next_run_at),
                MISFIRE_SKIP,
                CONCURRENCY_FORBID,
                300,
                "0 0 0 * * * *",
                "Asia/Shanghai",
                now,
            )
            .is_ok()
        );
        let invalid = validate_persisted_schedule(
            Some(next_run_at),
            "unknown",
            CONCURRENCY_FORBID,
            300,
            "0 0 0 * * * *",
            "Asia/Shanghai",
            now,
        );
        assert_eq!(
            invalid.err().as_deref(),
            Some("错过执行策略只能是 skip 或 fire_once")
        );
    }
}
