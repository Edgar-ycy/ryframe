use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    Result,
    recovery::monitoring::{
        bind::verify_current,
        config::{run_logged, safe_environment},
        contract::{
            BASE_NAMES, RUNTIME_DIRECTORIES, file_descriptor as descriptor, read_binding,
            rule_alerts,
        },
        http,
        lifecycle::assert_running,
        receipts::{read_process_trees, read_start, validate_observation},
    },
};

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn observe(backend: &Path, binding_path: &Path) -> Result<Value> {
    let binding = read_binding(backend, binding_path)?;
    let inputs = super::execution::ProtectedInputs::binding(&binding.value)?;
    verify_current(backend, &binding.value)?;
    let run = &binding.run;
    let expected = BASE_NAMES
        .iter()
        .chain(RUNTIME_DIRECTORIES.iter())
        .chain(["start-intent.json", "start.json"].iter())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    super::lifecycle::expect_root(run, expected)?;
    let binding_receipt = binding.document.descriptor();
    let (start_document, start, _intent, operations) =
        read_start(run, &binding.value, &binding_receipt)?;
    inputs.finish(observe_runtime(
        backend,
        &binding,
        &start_document,
        &start,
        &operations,
    ))
}

pub(crate) fn observe_runtime(
    backend: &Path,
    binding: &crate::recovery::monitoring::contract::Binding,
    start_document: &crate::recovery::monitoring::contract::Document,
    start: &Value,
    operations: &Map<String, Value>,
) -> Result<Value> {
    let run = &binding.run;
    let result = (|| -> Result<Value> {
        super::contract::validate_runtime_layout(run, operations, false)?;
        assert_running(run, &binding.value, start, operations)?;
        let (secret, _) =
            crate::recovery::monitoring::bind::load_environment(backend, &binding.value)?;
        let evidence = run.join("evidence");
        let metrics = verify_metrics(backend, &binding.value, &secret, &evidence)?;
        let prometheus = verify_prometheus(backend, &binding.value, &evidence)?;
        let environment = safe_environment(run)?;
        let boundaries = verify_boundaries(&binding.value, run, &environment)?;
        let delivery = verify_delivery(&binding.value, &evidence.join("webhook-events.jsonl"))?;
        assert_running(run, &binding.value, start, operations)?;
        let observation = json!({
            "format_version": 1,
            "kind": "restore-monitoring-observation",
            "status": "passed",
            "binding": binding.document.descriptor(),
            "start": start_document.descriptor(),
            "run_id": binding.value["run_id"],
            "scope_id": binding.value["scope_id"],
            "metrics": metrics,
            "prometheus": prometheus,
            "boundaries": boundaries,
            "delivery": delivery,
            "external_contacts": 0,
            "observed_at": utc_now(),
        });
        validate_observation(
            &observation,
            &binding.value,
            &binding.document.descriptor(),
            &start_document.descriptor(),
        )?;
        super::contract::validate_runtime_layout(run, operations, true)?;
        crate::recovery::clone::native::json::write_new(
            &run.join("observation.json"),
            &observation,
        )?;
        Ok(observation)
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let trees = read_process_trees(
                run,
                binding.value["scope_id"].as_str().unwrap_or_default(),
                operations,
            );
            super::failure::finish(binding, "observation", error, trees)
        }
    }
}

pub(crate) fn verify_metrics(
    backend: &Path,
    binding: &Value,
    secret: &str,
    evidence: &Path,
) -> Result<Value> {
    let mut result = Map::new();
    let endpoints = &binding["authority"]["endpoints"];
    for (role, field) in [("api", "api_metrics"), ("worker", "worker_metrics")] {
        let url = endpoints[field].as_str().ok_or("监控指标端点无效")?;
        let missing = http::request(url, &[], None, HTTP_TIMEOUT)?;
        let invalid = http::request(
            url,
            &[("Authorization", "Bearer ryframe-invalid-probe")],
            None,
            HTTP_TIMEOUT,
        )?;
        let bearer = format!("Bearer {secret}");
        let accepted = http::request(url, &[("Authorization", &bearer)], None, HTTP_TIMEOUT)?;
        if missing.status != 401
            || invalid.status != 401
            || accepted.status != 200
            || accepted
                .content_type
                .split(';')
                .next()
                .map(str::trim)
                .is_none_or(|value| !value.eq_ignore_ascii_case("text/plain"))
        {
            return Err(format!("{role} 指标 Bearer 鉴权或媒体类型无效").into());
        }
        let (samples, ryframe) = metric_facts(&accepted.body)?;
        let path = evidence.join(format!("{role}-metrics.txt"));
        super::config::write_bytes_new(&path, &accepted.body)?;
        result.insert(
            role.to_owned(),
            json!({
                "artifact": descriptor(&path)?,
                "missing_status": missing.status,
                "invalid_status": invalid.status,
                "accepted_status": accepted.status,
                "sample_count": samples,
                "ryframe_sample_count": ryframe,
            }),
        );
    }
    let _ = backend;
    Ok(Value::Object(result))
}

fn metric_facts(raw: &[u8]) -> Result<(u64, u64)> {
    let text = std::str::from_utf8(raw).map_err(|_| "产品指标不是 UTF-8")?;
    let mut samples = 0_u64;
    let mut ryframe = 0_u64;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line
            .split(['{', ' '])
            .next()
            .filter(|value| {
                !value.is_empty()
                    && value.bytes().enumerate().all(|(index, byte)| {
                        byte.is_ascii_alphabetic()
                            || byte == b'_'
                            || byte == b':'
                            || (index > 0 && byte.is_ascii_digit())
                    })
            })
            .ok_or("产品指标不是有效 Prometheus 文本")?;
        samples += 1;
        ryframe += u64::from(name.starts_with("ryframe_"));
    }
    if samples == 0 || ryframe == 0 {
        return Err("产品指标没有真实 RyFrame 样本".into());
    }
    Ok((samples, ryframe))
}

pub(crate) fn verify_prometheus(backend: &Path, binding: &Value, evidence: &Path) -> Result<Value> {
    verify_prometheus_with_timeout(backend, binding, evidence, OBSERVE_TIMEOUT)
}

pub(crate) fn verify_prometheus_with_timeout(
    backend: &Path,
    binding: &Value,
    evidence: &Path,
    timeout: Duration,
) -> Result<Value> {
    let base = binding["endpoints"]["prometheus"]
        .as_str()
        .ok_or("Prometheus 端点无效")?;
    let targets_url = format!("{base}/api/v1/targets");
    let rules_url = format!("{base}/api/v1/rules?type=alert");
    let (targets_raw, targets) = wait_json(&targets_url, timeout, |value| {
        validate_targets(binding, value)
    })?;
    let (rules_raw, alerts) =
        wait_json(&rules_url, timeout, |value| validate_rules(binding, value))?;
    let targets_path = evidence.join("prometheus-targets.json");
    let rules_path = evidence.join("prometheus-rules.json");
    super::config::write_bytes_new(&targets_path, &targets_raw)?;
    super::config::write_bytes_new(&rules_path, &rules_raw)?;
    let _ = backend;
    Ok(json!({
        "targets": targets,
        "targets_artifact": descriptor(&targets_path)?,
        "alerts": alerts,
        "rules_artifact": descriptor(&rules_path)?,
    }))
}

fn wait_json(
    url: &str,
    timeout: Duration,
    validate: impl Fn(&Value) -> Result<Value>,
) -> Result<(Vec<u8>, Value)> {
    let deadline = Instant::now() + timeout;
    let mut last;
    loop {
        let error = match http::request(url, &[], None, HTTP_TIMEOUT) {
            Ok(response) if response.status == 200 && response.body.len() <= 1024 * 1024 => {
                match crate::recovery::clone::native::json::parse_fragment(
                    std::str::from_utf8(&response.body).unwrap_or_default(),
                    "监控状态",
                )
                .and_then(|value| validate(&value))
                {
                    Ok(value) => return Ok((response.body, value)),
                    Err(error) => error,
                }
            }
            Ok(response) => format!("监控状态 HTTP {}", response.status).into(),
            Err(error) => error,
        };
        last = Some(error);
        if Instant::now() >= deadline {
            return Err(format!(
                "监控状态未在期限内满足绑定：{}",
                last.map(|error| error.to_string())
                    .unwrap_or_else(|| "未知错误".into())
            )
            .into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn validate_targets(binding: &Value, value: &Value) -> Result<Value> {
    if value.get("status").and_then(Value::as_str) != Some("success") {
        return Err("Prometheus target API 返回失败".into());
    }
    let active = value
        .pointer("/data/activeTargets")
        .and_then(Value::as_array)
        .ok_or("Prometheus target API 缺少活动目标")?;
    let expected = [
        (
            "ryframe-api",
            binding["authority"]["endpoints"]["api_metrics"]
                .as_str()
                .ok_or("API 指标端点无效")?,
        ),
        (
            "ryframe-worker",
            binding["authority"]["endpoints"]["worker_metrics"]
                .as_str()
                .ok_or("Worker 指标端点无效")?,
        ),
    ]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let mut observed = Map::new();
    for item in active {
        let Some(job) = item
            .pointer("/labels/job")
            .and_then(Value::as_str)
            .filter(|job| expected.contains_key(*job))
        else {
            continue;
        };
        if observed.contains_key(job)
            || item.get("health").and_then(Value::as_str) != Some("up")
            || item.get("scrapeUrl").and_then(Value::as_str) != Some(expected[job])
            || item
                .get("lastError")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        {
            return Err("Prometheus target 与真实指标端点不同或未就绪".into());
        }
        observed.insert(
            job.to_owned(),
            json!({"health": "up", "scrape_url": expected[job]}),
        );
    }
    if observed.len() != expected.len() {
        return Err("Prometheus 没有同时采集真实 API 与 Worker".into());
    }
    Ok(Value::Object(observed))
}

fn validate_rules(binding: &Value, value: &Value) -> Result<Value> {
    if value.get("status").and_then(Value::as_str) != Some("success") {
        return Err("Prometheus rule API 返回失败".into());
    }
    let groups = value
        .pointer("/data/groups")
        .and_then(Value::as_array)
        .ok_or("Prometheus rule API 缺少规则组")?;
    let mut names = Vec::new();
    for rule in groups
        .iter()
        .filter_map(|group| group.get("rules").and_then(Value::as_array))
        .flatten()
    {
        if let Some(name) = rule.get("name").and_then(Value::as_str) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    let expected = rule_alerts(binding)?.into_iter().collect::<Vec<_>>();
    if names != expected {
        return Err("Prometheus 没有各加载一次固定恢复告警".into());
    }
    Ok(json!(names))
}

fn verify_boundaries(
    binding: &Value,
    run: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<Value> {
    let log = run.join("evidence").join("promtool-boundaries.log");
    let command = vec![
        binding["tools"]["promtool"]["path"]
            .as_str()
            .ok_or("promtool 路径无效")?
            .to_owned(),
        "test".into(),
        "rules".into(),
        super::config::path_text(
            &run.join("configs").join("ryframe-alerts.test.yml"),
            "告警规则测试",
        )?,
    ];
    run_logged(
        command,
        &run.join("configs"),
        environment,
        &log,
        Duration::from_secs(300),
    )?;
    Ok(json!({
        "passed": true,
        "cases": ["23h", "23h30s", "24h30s"],
        "artifact": descriptor(&log)?,
    }))
}

pub(crate) fn verify_delivery(binding: &Value, events_path: &Path) -> Result<Value> {
    verify_delivery_with_timeout(binding, events_path, OBSERVE_TIMEOUT)
}

pub(crate) fn verify_delivery_with_timeout(
    binding: &Value,
    events_path: &Path,
    timeout: Duration,
) -> Result<Value> {
    let probe = Uuid::new_v4().simple().to_string();
    let started = Utc::now();
    let started_text = started.to_rfc3339_opts(SecondsFormat::Micros, true);
    let endpoint = format!(
        "{}/api/v2/alerts",
        binding["endpoints"]["alertmanager"]
            .as_str()
            .ok_or("Alertmanager 端点无效")?
    );
    let firing = alert_body(binding, &probe, started, false)?;
    let response = http::request(
        &endpoint,
        &[("Content-Type", "application/json")],
        Some(&firing),
        HTTP_TIMEOUT,
    )?;
    if response.status != 200 {
        return Err("Alertmanager 拒绝隔离 firing 告警".into());
    }
    wait_event(
        events_path,
        binding,
        &probe,
        &started_text,
        "firing",
        timeout,
    )?;
    let resolved = alert_body(binding, &probe, started, true)?;
    let response = http::request(
        &endpoint,
        &[("Content-Type", "application/json")],
        Some(&resolved),
        HTTP_TIMEOUT,
    )?;
    if response.status != 200 {
        return Err("Alertmanager 拒绝隔离 resolved 告警".into());
    }
    wait_event(
        events_path,
        binding,
        &probe,
        &started_text,
        "resolved",
        timeout,
    )?;
    let first = fs::read(events_path)?;
    thread::sleep(Duration::from_millis(100));
    if fs::read(events_path)? != first {
        return Err("隔离 webhook 事件在证据绑定时仍在变化".into());
    }
    let delivered = event_values(&first, binding, &probe, &started_text)?;
    if delivered != ["firing", "resolved"] {
        return Err("隔离 webhook 必须各接收一次 firing 与 resolved probe".into());
    }
    Ok(json!({
        "probe_id": probe,
        "firing": true,
        "resolved": true,
        "receiver": format!(
            "{}/alerts",
            binding["endpoints"]["webhook"]
                .as_str()
                .ok_or("Webhook 端点无效")?
        ),
        "events": descriptor(events_path)?,
    }))
}

fn alert_body(
    binding: &Value,
    probe: &str,
    started: chrono::DateTime<Utc>,
    resolved: bool,
) -> Result<Vec<u8>> {
    let now = Utc::now();
    let end = if resolved {
        now - chrono::Duration::seconds(1)
    } else {
        now + chrono::Duration::minutes(10)
    };
    Ok(serde_json::to_vec(&json!([{
        "labels": {
            "alertname": "RyFrameMonitoringDeliveryProbe",
            "service": "ryframe",
            "scope_id": binding["scope_id"],
            "probe_id": probe,
        },
        "annotations": {"summary": "RyFrame 隔离监控投递验收"},
        "startsAt": started.to_rfc3339_opts(SecondsFormat::Micros, true),
        "endsAt": end.to_rfc3339_opts(SecondsFormat::Micros, true),
        "generatorURL": binding["endpoints"]["prometheus"],
    }]))?)
}

fn wait_event(
    path: &Path,
    binding: &Value,
    probe: &str,
    started: &str,
    status: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let raw = fs::read(path)?;
        if event_values(&raw, binding, probe, started)?.contains(&status) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("Alertmanager 没有向隔离 webhook 投递 {status}").into())
}

fn event_values(
    raw: &[u8],
    binding: &Value,
    probe: &str,
    started: &str,
) -> Result<Vec<&'static str>> {
    if raw.len() > http::MAX_RESPONSE_BYTES {
        return Err("隔离 webhook 事件超过 4 MiB".into());
    }
    let mut result = Vec::new();
    let text = std::str::from_utf8(raw).map_err(|_| "隔离 webhook 事件不是 UTF-8")?;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let event =
            crate::recovery::clone::native::json::parse_fragment(line, "隔离 webhook 事件")?;
        let Some(status) = probe_event(&event, binding, probe, started)? else {
            continue;
        };
        result.push(status);
    }
    Ok(result)
}

pub(crate) fn probe_event(
    event: &Value,
    binding: &Value,
    probe: &str,
    started: &str,
) -> Result<Option<&'static str>> {
    let Some(payload) = event.get("payload").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(alerts) = payload.get("alerts").and_then(Value::as_array) else {
        return Ok(None);
    };
    let candidates = alerts
        .iter()
        .filter(|item| {
            item.pointer("/labels/alertname").and_then(Value::as_str)
                == Some("RyFrameMonitoringDeliveryProbe")
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(None);
    }
    let Some(status) = payload
        .get("status")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "firing" | "resolved"))
    else {
        return Err("隔离 webhook probe 状态无效".into());
    };
    let expected_labels = json!({
        "alertname": "RyFrameMonitoringDeliveryProbe",
        "service": "ryframe",
        "scope_id": binding["scope_id"],
        "probe_id": probe,
    });
    let item = candidates[0];
    if !matches!(
        event.get("remote").and_then(Value::as_str),
        Some("127.0.0.1" | "::1")
    ) || payload.get("receiver").and_then(Value::as_str) != Some("ryframe-local-webhook")
        || alerts.len() != 1
        || candidates.len() != 1
        || item.get("status").and_then(Value::as_str) != Some(status)
        || item.get("labels") != Some(&expected_labels)
        || item.get("annotations") != Some(&json!({"summary": "RyFrame 隔离监控投递验收"}))
        || item.get("startsAt").and_then(Value::as_str) != Some(started)
        || item.get("generatorURL")
            != binding
                .get("endpoints")
                .and_then(|value| value.get("prometheus"))
    {
        return Err("隔离 webhook probe 投递形状、接收器或身份无效".into());
    }
    Ok(Some(if status == "firing" {
        "firing"
    } else {
        "resolved"
    }))
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
