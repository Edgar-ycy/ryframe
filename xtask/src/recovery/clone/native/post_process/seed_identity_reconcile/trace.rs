//! 从同一 API 创建代次的原日志恢复请求归属。日志只读，不收录请求或响应正文。

use std::{fs::File, io::Read, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{json as document, resources::common},
};

const MAX_LOG: u64 = 8 * 1024 * 1024;
const POST: &str = "http.method=POST http.route=/api/v1/system/users ";

pub(super) struct Trace {
    pub(super) evidence: Value,
    pub(super) audit: Value,
}

pub(super) fn read(
    backend: &Path,
    runtime: &Path,
    api_identity: &Value,
    admin: &Value,
    attempt: &Value,
    expected: &str,
) -> Result<Trace> {
    let history_path = runtime.join("producer-history.json");
    let history_binding = common::binding(&history_path)?;
    let history = common::read_bound(backend, &history_binding, "原 API 生产者历史")?;
    let log = log_name(&history, api_identity)?;
    let path = runtime.join(log);
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
    let file = File::open(&path)?;
    let opened = same_file::Handle::from_file(file.try_clone()?)?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LOG || same_file::Handle::from_path(&path)? != opened {
        return Err("原 API 请求日志过大或读取期间改变文件身份".into());
    }
    let start = instant(document::text(
        &attempt["started_at"],
        "原身份阶段开始时间",
    )?)?;
    let end = instant(document::text(
        &attempt["finished_at"],
        "原身份阶段结束时间",
    )?)?;
    let raw = std::str::from_utf8(&bytes).map_err(|_| "原 API 日志不是 UTF-8")?;
    let line = unique_line(raw, start, end)?;
    let (audit, evidence) = observation(&line, expected, admin, start)?;
    if common::binding(&history_path)? != history_binding {
        return Err("原 API 生产者历史在读取期间变化".into());
    }
    Ok(Trace {
        evidence: json!({"event":evidence,"api_history":history_binding,"log":path}),
        audit,
    })
}

fn log_name<'a>(history: &'a Value, identity: &Value) -> Result<&'a str> {
    let events = document::array(&history["events"], "原 API 生产者历史")?;
    let [.., intent, started] = events else {
        return Err("原 API 缺少同代启动意图与身份".into());
    };
    let token = document::text(&intent["token"], "原 API 启动令牌")?;
    let name = document::text(&intent["log"], "原 API 日志文件名")?;
    if token.len() != 32
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || intent["event"] != "start-intent"
        || intent["role"] != "api"
        || started["event"] != "started"
        || started["role"] != "api"
        || started["token"] != token
        || started["identity"] != *identity
        || name != format!("api-{token}.log")
    {
        return Err("原 API 日志不同于已登记且仍在线的创建代次".into());
    }
    Ok(name)
}

fn unique_line(raw: &str, start: OffsetDateTime, end: OffsetDateTime) -> Result<String> {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]")?;
    let mut found = None;
    for line in raw
        .lines()
        .filter(|line| line.contains("/api/v1/system/users"))
    {
        let line = ansi.replace_all(line, "");
        if !line.contains(POST) || !line.contains("finished processing request") {
            continue;
        }
        let at = instant(
            line.split_whitespace()
                .next()
                .ok_or("原 API 日志缺少时刻")?,
        )?;
        if (start..=end).contains(&at) && found.replace(line.into_owned()).is_some() {
            return Err("原身份阶段有多个用户创建请求，拒绝猜测成功归属".into());
        }
    }
    found.ok_or_else(|| "原身份阶段缺少唯一的用户创建请求日志".into())
}

fn observation(
    line: &str,
    expected: &str,
    admin: &Value,
    stage_start: OffsetDateTime,
) -> Result<(Value, Value)> {
    let sha = hex::encode(Sha256::digest(line.as_bytes()));
    let pattern = regex::Regex::new(
        r#"\A(\S+)\s+INFO request\{http\.method=POST http\.route=/api/v1/system/users http\.request_id=([a-f0-9-]{36}) http\.client_ip=(\S+) tenant\.id="([^"]+)" user\.id=([0-9]+) user\.name="([^"]+)"\}: tower_http::trace::on_response: finished processing request latency=([0-9]+) ms status=200\z"#,
    )?;
    let values = pattern
        .captures(line)
        .ok_or("原创建请求没有完整 HTTP 200 跟踪记录")?;
    let request_id = &values[2];
    if !sha.eq_ignore_ascii_case(expected)
        || !uuid::Uuid::parse_str(request_id).is_ok_and(|id| id.to_string() == request_id)
        || admin["client_address"].as_str() != Some(&values[3])
        || admin["tenant_id"].as_str() != Some(&values[4])
        || admin["subject_id"].as_str() != Some(&values[5])
        || admin["username"].as_str() != Some(&values[6])
    {
        return Err("原 HTTP 200 跟踪与唯一已登记管理员及请求摘要不同".into());
    }
    let at = instant(&values[1])?;
    let elapsed = values[7].parse::<u32>()?;
    let request_start = at - Duration::milliseconds(i64::from(elapsed) + 1);
    if request_start < stage_start || elapsed > 120_000 {
        return Err("原 HTTP 请求的执行时间超出已登记身份阶段".into());
    }
    // 冻结控制库的两个 created_at 均为 DATETIME，按实际秒精度覆盖原请求时段。
    let window_start = request_start.replace_nanosecond(0)?;
    let window_end = at.replace_nanosecond(0)? + Duration::seconds(i64::from(at.nanosecond() != 0));
    let window_start = window_start.format(&Rfc3339)?;
    let window_end = window_end.format(&Rfc3339)?;
    let audit =
        json!({"request_id":request_id,"window_start":window_start,"window_end":window_end});
    let evidence = json!({
        "line_sha256":sha,
        "request_id_sha256":hex::encode(Sha256::digest(request_id.as_bytes())),
        "method":"POST","route":"/api/v1/system/users","status":200,
        "finished_at":at.format(&Rfc3339)?,"latency_ms":elapsed,
        "sql_window_start":window_start,"sql_window_end":window_end,
    });
    Ok((audit, evidence))
}

fn instant(value: &str) -> Result<OffsetDateTime> {
    Ok(OffsetDateTime::parse(value, &Rfc3339)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_creation_binds_actor_and_unique_log_record_at_database_precision() {
        let line = r#"2026-01-01T15:39:54.704628Z  INFO request{http.method=POST http.route=/api/v1/system/users http.request_id=019bb36b-33e0-7c01-815b-e54c373777ab http.client_ip=192.0.2.1 tenant.id="system" user.id=1 user.name="fixture-admin"}: tower_http::trace::on_response: finished processing request latency=412 ms status=200"#;
        let start = instant("2026-01-01T15:38:00Z").unwrap();
        let end = instant("2026-01-01T15:40:00Z").unwrap();
        let sha = hex::encode(Sha256::digest(line.as_bytes()));
        let mut admin = json!({
            "client_address":"192.0.2.1","tenant_id":"system","subject_id":"1","username":"fixture-admin",
        });
        let observed = unique_line(line, start, end).unwrap();
        let (audit, _) = observation(&observed, &sha, &admin, start).unwrap();
        assert_eq!(audit["window_start"], "2026-01-01T15:39:54Z");
        assert_eq!(audit["window_end"], "2026-01-01T15:39:55Z");
        admin["subject_id"] = json!("2");
        assert!(observation(&observed, &sha, &admin, start).is_err());
        assert!(unique_line(&format!("{line}\n{line}"), start, end).is_err());
    }
}
