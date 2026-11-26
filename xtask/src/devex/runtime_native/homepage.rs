use super::{
    Input, artifacts, configuration,
    failure::{Failure, NativeResult},
    load::{self, Measurement},
    process::{self, Cancellation},
    request::endpoint,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub(crate) fn visitors(config: &Value) -> NativeResult<Vec<Value>> {
    let cycles = config["contract"]["cycles"]
        .as_u64()
        .filter(|value| (10..=10000).contains(value))
        .ok_or_else(|| Failure::new("homepage_cycles"))?;
    config["contract"]["homepage"]["visitor_interval_ms"]
        .as_u64()
        .filter(|value| *value <= 60000)
        .ok_or_else(|| Failure::new("homepage_interval"))?;
    for key in ["api_url", "frontend_url"] {
        if !matches!(
            endpoint(&config["bindings"][key])?.host_str(),
            Some("127.0.0.1" | "[::1]")
        ) {
            return Err(Failure::new("homepage_loopback"));
        }
    }
    configuration::identities(config, "homepage", "homepage", cycles)
}

/// 浏览器操作由前端工具承担；后端不复制或内嵌可执行 JS。
pub(crate) async fn measure(
    config: &Value,
    input: &Input,
    directory: &Path,
    cancel: &Cancellation,
) -> NativeResult<Measurement> {
    let identities = visitors(config)?;
    let tool = input.runner_frontend.join("scripts/devex-homepage.mjs");
    // 缺失外部工具时保留失败，不退回旧后端浏览器实现。
    let before = artifacts::read(&tool, 1024 * 1024)
        .map_err(|_| Failure::new("frontend_homepage_tool_missing"))?;
    let node = crate::process::resolved_executable(&input.driver, "node")
        .map_err(|_| Failure::new("frontend_node_configuration"))?;
    let timeout_ms = config["contract"]["timeout_ms"]
        .as_u64()
        .ok_or_else(|| Failure::new("homepage_timeout"))?;
    let cycles = identities.len() as u64;
    let interval = config["contract"]["homepage"]["visitor_interval_ms"]
        .as_u64()
        .unwrap_or(0);
    let deadline = cycles
        .checked_mul(
            timeout_ms
                .checked_mul(8)
                .and_then(|value| value.checked_add(interval))
                .ok_or_else(|| Failure::new("homepage_timeout"))?,
        )
        .and_then(|value| value.checked_add(15000))
        .ok_or_else(|| Failure::new("homepage_timeout"))?;
    let request = json!({"format_version":1,"kind":"ryframe-devex-homepage","cache":input.cache,
        "frontend_url":config["bindings"]["frontend_url"],"timeout_ms":timeout_ms,"visitor_interval_ms":interval,
        "identities":identities,"artifacts":directory,"runner_frontend_fingerprint":input.runner_frontend_fingerprint});
    let response = process::json(
        Path::new(&node),
        &[tool.to_string_lossy().into_owned()],
        &input.runner_frontend,
        &request,
        Duration::from_millis(deadline),
        16 * 1024 * 1024,
        cancel,
    )
    .await?;
    if artifacts::read(&tool, 1024 * 1024)? != before {
        return Err(Failure::new("frontend_homepage_tool_changed"));
    }
    let measurement = validate(&response, cycles, &input.cache, &identities)?;
    let rows = response["cycles"]
        .as_array()
        .ok_or_else(|| Failure::new("homepage_output"))?;
    artifacts::append(&directory.join("homepage-cycles.jsonl"), rows)?;
    Ok(measurement)
}

pub(crate) fn validate(
    response: &Value,
    cycles: u64,
    cache: &str,
    identities: &[Value],
) -> NativeResult<Measurement> {
    if response["format_version"] != 1
        || response["kind"] != "ryframe-devex-homepage"
        || response["cache"] != cache
        || response["cycles"]
            .as_array()
            .is_none_or(|rows| rows.len() as u64 != cycles)
    {
        return Err(Failure::new("homepage_output"));
    }
    let elapsed = number(&response["elapsed_ms"])?;
    let mut times: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
    let mut failed = 0;
    let mut sessions = 0;
    let mut requests = 0;
    for (index, row) in response["cycles"]
        .as_array()
        .ok_or_else(|| Failure::new("homepage_output"))?
        .iter()
        .enumerate()
    {
        if row["cycle"].as_u64() != Some(index as u64)
            || row["cache"] != cache
            || row["client_address"] != identities[index]["client_address"]
            || !row["succeeded"].is_boolean()
            || !row["session_failure"].is_boolean()
            || row["session_failure"].as_bool() != Some(row["phase"] == "login")
            || !matches!(
                row["phase"].as_str(),
                Some("preparing" | "login" | "warming" | "measuring")
            )
            || row.as_object().is_none_or(|fields| {
                fields.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "cycle"
                            | "cache"
                            | "client_address"
                            | "phase"
                            | "succeeded"
                            | "session_failure"
                            | "preparation_requests"
                            | "measured_requests"
                            | "static_cache"
                            | "diagnostics"
                            | "preparation_duration_ms"
                            | "times"
                            | "failure"
                            | "duration_ms"
                    )
                })
            })
        {
            return Err(Failure::new("homepage_cycle_binding"));
        }
        validate_static_cache(row, cache)?;
        requests += row["measured_requests"]
            .as_u64()
            .ok_or_else(|| Failure::new("homepage_network"))?;
        if row["succeeded"] == true {
            if row["phase"] != "measuring"
                || row["session_failure"] == true
                || row["diagnostics"]
                    .as_array()
                    .is_none_or(|rows| !rows.is_empty())
                || row["measured_requests"] == 0
            {
                return Err(Failure::new("homepage_diagnostics"));
            }
            for (slot, key) in ["navigation", "lcp", "content"].iter().enumerate() {
                let value = number(&row["times"][*key])?;
                if value == 0.0 {
                    return Err(Failure::new("homepage_real_timing"));
                }
                times[slot].push(value);
            }
        } else {
            failed += 1;
            if row["session_failure"] == true {
                sessions += 1;
            }
            if !matches!(
                row["failure"].as_str(),
                Some("timeout" | "network_or_binding" | "browser_validation_failed")
            ) {
                return Err(Failure::new("homepage_failure_category"));
            }
        }
    }
    let scenarios = ["navigation", "lcp", "content"]
        .into_iter()
        .zip(times)
        .map(|(key, values)| {
            (
                key.into(),
                load::summarize(&values, failed, elapsed, &[], &[]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Ok(Measurement {
        scenarios,
        network_requests: requests,
        completed_cycles: cycles - failed,
        failed_cycles: failed,
        session_failures: sessions,
        pending: Vec::new(),
    })
}

fn validate_static_cache(row: &Value, cache: &str) -> NativeResult<()> {
    let invalid = || Failure::new("homepage_cache_evidence");
    let fields = row["static_cache"]
        .as_object()
        .filter(|fields| fields.len() == 3)
        .ok_or_else(invalid)?;
    let [Some(responses), Some(hits), Some(primed_hits)] =
        ["responses", "hits", "primed_hits"].map(|key| fields.get(key).and_then(Value::as_u64))
    else {
        return Err(invalid());
    };
    let observed = row["succeeded"] == false
        || match cache {
            "cold" => responses > 0 && hits == 0,
            "warm" => primed_hits > 0,
            _ => false,
        };
    if primed_hits > hits || hits > responses || !observed {
        return Err(invalid());
    }
    Ok(())
}

fn number(value: &Value) -> NativeResult<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| Failure::new("homepage_timing"))
}
