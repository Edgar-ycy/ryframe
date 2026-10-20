use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, Timelike};
use serde_json::{Value, json};

type EvidenceResult = Result<Value, &'static str>;

pub(crate) fn identifier(value: &Value) -> EvidenceResult {
    let text = value.as_str().ok_or("invalid_job_identifier")?;
    if text.is_empty()
        || text.len() > 19
        || !matches!(text.as_bytes()[0], b'1'..=b'9')
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || text.parse::<i64>().is_err()
    {
        return Err("invalid_job_identifier");
    }
    Ok(value.clone())
}

pub(crate) fn selection(value: &Value) -> EvidenceResult {
    let object = value.as_object().ok_or("invalid_job_selection")?;
    if object.len() != 4
        || ["kind", "id", "tenant_id", "job_type"]
            .iter()
            .any(|key| !object.contains_key(*key))
    {
        return Err("invalid_job_selection");
    }
    let expected_type = match value["kind"].as_str() {
        Some("export") => Some("system.export.execute"),
        Some("import") => Some("system.user.import"),
        Some("message") => Some("system.message.dispatch"),
        Some("schedule") => None,
        _ => return Err("invalid_job_kind"),
    };
    identifier(&value["id"])?;
    let tenant = value["tenant_id"].as_str().ok_or("invalid_tenant")?;
    if tenant.is_empty()
        || tenant.len() > 64
        || !tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("invalid_tenant");
    }
    let job_type = value["job_type"].as_str().ok_or("wrong_job_type")?;
    if job_type.is_empty()
        || job_type.len() > 96
        || !job_type.as_bytes()[0].is_ascii_lowercase()
        || !job_type.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-')
        })
        || expected_type.is_some_and(|expected| job_type != expected)
    {
        return Err("wrong_job_type");
    }
    Ok(value.clone())
}

pub(crate) fn associated_job(rows: &[Value], expected: &Value) -> EvidenceResult {
    selection(expected)?;
    let row = match rows {
        [] => return Err("missing_job"),
        [row] => row,
        _ => return Err("ambiguous_job"),
    };
    if !row.is_object() {
        return Err("invalid_job_row");
    }
    identifier(&row["job_id"])?;
    if row["business_id"] != expected["id"] {
        return Err("wrong_business_identifier");
    }
    if row["business_tenant"] != expected["tenant_id"] {
        return Err("wrong_tenant");
    }
    let platform_schedule = expected["kind"] == "schedule"
        && expected["tenant_id"] == "system"
        && row["job_tenant"].is_null();
    if row["job_tenant"] != expected["tenant_id"] && !platform_schedule {
        return Err("wrong_job_tenant");
    }
    if row["job_type"] != expected["job_type"] {
        return Err("wrong_job_type");
    }
    if expected["kind"] == "schedule" && row["business_status"] != "enqueued" {
        return Err("schedule_not_enqueued");
    }
    if expected["kind"] == "message" && row["business_status"] != "published" {
        return Err("message_not_dispatched");
    }
    Ok(row.clone())
}

pub(crate) fn timestamp(value: &Value) -> Result<DateTime<FixedOffset>, &'static str> {
    let text = value.as_str().ok_or("missing_attempt_timestamp")?;
    // fromisoformat 只接受大写 Z；chrono 的 RFC 解析还接受小写 z。
    if text.ends_with('z') {
        return Err("invalid_attempt_timestamp");
    }
    let normalized = normalize_timestamp(text);
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&normalized) {
        return microsecond_timestamp(parsed);
    }
    // MySQL 时间使用扩展 ISO 格式；同时保留 Python 接受的空格和基本 ISO 写法。
    for date in ["%Y-%m-%d", "%Y%m%d", "%G-W%V-%u", "%GW%V%u"] {
        for separator in ['T', ' '] {
            for time in ["%H:%M:%S%.f", "%H%M%S%.f", "%H:%M", "%H%M", "%H"] {
                let format = format!("{date}{separator}{time}");
                for offset in ["%:z", "%z", "%#z"] {
                    if let Ok(parsed) =
                        DateTime::parse_from_str(&normalized, &format!("{format}{offset}"))
                    {
                        return microsecond_timestamp(parsed);
                    }
                }
                if let Ok(parsed) = NaiveDateTime::parse_from_str(&normalized, &format) {
                    if !(1..=9999).contains(&parsed.year()) {
                        return Err("invalid_attempt_timestamp");
                    }
                    return Err("timestamp_without_timezone");
                }
            }
        }
        if let Ok(parsed) = NaiveDate::parse_from_str(&normalized, date) {
            if !(1..=9999).contains(&parsed.year()) {
                return Err("invalid_attempt_timestamp");
            }
            return Err("timestamp_without_timezone");
        }
    }
    Err("invalid_attempt_timestamp")
}

fn normalize_timestamp(text: &str) -> String {
    // Python 接受逗号小数及任意位小数，并截断至微秒；先归一化再交给 chrono。
    let mut normalized = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            'Z' => normalized.push_str("+00:00"),
            '.' | ',' => {
                normalized.push('.');
                let mut digits = 0;
                while characters.peek().is_some_and(char::is_ascii_digit) {
                    if let Some(digit) = characters.next() {
                        if digits < 6 {
                            normalized.push(digit);
                        }
                        digits += 1;
                    }
                }
            }
            _ => normalized.push(character),
        }
    }
    normalized
}

fn microsecond_timestamp(
    parsed: DateTime<FixedOffset>,
) -> Result<DateTime<FixedOffset>, &'static str> {
    // datetime 的精度为微秒，拒绝闰秒并截去更细精度，避免与原模型出现顺序差异。
    if !(1..=9999).contains(&parsed.year()) || parsed.nanosecond() >= 1_000_000_000 {
        return Err("invalid_attempt_timestamp");
    }
    parsed
        .with_nanosecond(parsed.nanosecond() / 1000 * 1000)
        .ok_or("invalid_attempt_timestamp")
}

pub(crate) fn attempt_times(row: &Value, attempts: &[Value]) -> EvidenceResult {
    if !row.is_object() {
        return Err("invalid_job_row");
    }
    let count = row["claim_sequence"]
        .as_u64()
        .ok_or("missing_attempt_history")?;
    if count < 1 || usize::try_from(count).ok() != Some(attempts.len()) {
        return Err("missing_attempt_history");
    }
    let mut queue_ms = 0.0;
    let mut execution_ms = 0.0;
    let mut unsuccessful_attempts = 0;
    let mut previous_end = None;
    for (index, attempt) in attempts.iter().enumerate() {
        if !attempt.is_object()
            || attempt["job_id"] != row["job_id"]
            || attempt["sequence"].as_u64() != Some(index as u64 + 1)
        {
            return Err("wrong_attempt_identity");
        }
        match attempt["outcome"].as_str() {
            Some("running") => return Err("unfinished_attempt"),
            Some("lease_expired") => return Err("attempt_result_unknown"),
            Some("succeeded") => {}
            Some("failed" | "dead" | "deferred") => unsuccessful_attempts += 1,
            _ => return Err("invalid_attempt_outcome"),
        }
        let queued = timestamp(&attempt["available_at"])?;
        let started = timestamp(&attempt["started_at"])?;
        let finished = timestamp(&attempt["finished_at"])?;
        let closed = timestamp(&attempt["closed_at"])?;
        if queued > started
            || started > finished
            || finished != closed
            || previous_end.is_some_and(|previous| started < previous)
        {
            return Err("unordered_attempt_timestamps");
        }
        queue_ms += milliseconds(started - queued);
        execution_ms += milliseconds(finished - started);
        previous_end = Some(finished);
    }
    if attempts
        .last()
        .is_none_or(|attempt| attempt["outcome"] != "succeeded")
    {
        return Err("last_attempt_not_successful");
    }
    Ok(json!({
        "queue_ms": queue_ms,
        "execution_ms": execution_ms,
        "attempt_count": count,
        "unsuccessful_attempts": unsuccessful_attempts,
    }))
}

fn milliseconds(duration: chrono::TimeDelta) -> f64 {
    // 分开整秒与微秒余数，避免整数毫秒截断及超长时间跨度的微秒计数溢出。
    (duration.num_seconds() as f64 + f64::from(duration.subsec_micros()) / 1_000_000.0) * 1000.0
}

fn business_result(row: &Value) -> Result<bool, &'static str> {
    if !row.is_object() {
        return Err("invalid_job_row");
    }
    match row["business_status"].as_str() {
        Some("cancelled" | "cancel_requested") => return Err("job_cancelled"),
        Some("failed" | "partial" | "dead" | "expired") => return Err("job_failed"),
        _ => {}
    }
    if row["status"] == "dead" {
        return Err("job_failed");
    }
    Ok(row["status"] == "succeeded"
        && matches!(
            row["business_status"].as_str(),
            Some("succeeded" | "published" | "enqueued")
        ))
}

pub(crate) fn evidence(row: &Value, attempts: &[Value]) -> Value {
    let mut result = json!({"job": row, "attempts": attempts, "valid": false});
    let times = business_result(row).and_then(|complete| {
        if complete {
            attempt_times(row, attempts)
        } else {
            Err("job_unfinished")
        }
    });
    match times {
        Ok(Value::Object(times)) => {
            for (key, value) in times {
                result[&key] = value;
            }
            result["valid"] = json!(true);
        }
        Ok(_) => result["reason"] = json!("invalid_attempt_history"),
        Err(reason) => result["reason"] = json!(reason),
    }
    result
}

pub(crate) fn completion(rows: &[Value], expected: &Value) -> Option<Value> {
    let complete = associated_job(rows, expected).and_then(|row| {
        if !matches!(row["status"].as_str(), Some("succeeded" | "dead")) {
            return Ok(Value::Null);
        }
        if !business_result(&row)? {
            return Ok(Value::Null);
        }
        Ok(json!({"valid": true, "job_id": row["job_id"], "status": row["status"]}))
    });
    match complete {
        Err("missing_job" | "message_not_dispatched" | "job_unfinished") | Ok(Value::Null) => None,
        Err(reason) => Some(json!({"valid": false, "reason": reason})),
        Ok(result) => Some(result),
    }
}
