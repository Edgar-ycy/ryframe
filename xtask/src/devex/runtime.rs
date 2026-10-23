use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

use super::{
    model::{CacheState, DevexRunOptions, DevexSuite},
    report::{Distribution, SampleRecord, SampleStatus, distribution},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeEvidence {
    pub(super) network_requests: u64,
    pub(super) completed_cycles: u64,
    pub(super) failed_cycles: u64,
    pub(super) session_failures: u64,
    pub(super) input_sha256: String,
    pub(super) cache_state: CacheState,
    pub(super) scenarios: BTreeMap<String, RuntimeMetric>,
    pub(super) peak_resident_memory_bytes: f64,
    pub(super) cpu_seconds: f64,
    pub(super) peak_database_connections: f64,
    pub(super) collector_failures: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeMetric {
    pub(super) completed: u64,
    pub(super) failed: u64,
    pub(super) elapsed_ms: f64,
    pub(super) p50_ms: f64,
    pub(super) p95_ms: f64,
    pub(super) p99_ms: f64,
    pub(super) throughput: f64,
    pub(super) queue_p95_ms: Option<f64>,
    pub(super) execution_p95_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RuntimeSummary {
    pub(super) network_requests: u64,
    pub(super) completed_cycles: u64,
    pub(super) failed_cycles: u64,
    pub(super) cycle_failure_rate: f64,
    pub(super) session_failures: u64,
    pub(super) input_sha256: String,
    pub(super) scenarios: BTreeMap<String, ScenarioSummary>,
    pub(super) peak_resident_memory_bytes: f64,
    pub(super) cpu_seconds: Distribution,
    pub(super) peak_database_connections: f64,
    pub(super) collector_failures: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct ScenarioSummary {
    pub(super) p50_ms: Distribution,
    pub(super) p95_ms: Distribution,
    pub(super) p99_ms: Distribution,
    pub(super) throughput: Distribution,
    pub(super) queue_p95_ms: Option<Distribution>,
    pub(super) execution_p95_ms: Option<Distribution>,
}

/// 报告只保存无凭据的测量契约；不再借进程环境保存协议字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeContract {
    pub(crate) input_sha256: String,
    pub(crate) concurrency: u64,
}

pub(super) fn prepare(
    root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
    output: &Path,
) -> Result<super::runtime_native::Input> {
    let path = root.join(".local-tests/devex/runtime.json");
    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(&path)
            .map_err(|error| format!("运行时测量需要显式隔离配置 {}：{error}", path.display()))?,
    )?;
    let projected = super::runtime_native::configuration::selection_config(&config)?;
    let concurrency = if options.suite == DevexSuite::RuntimeHomepage {
        0
    } else {
        match options.variant.as_str() {
            "10" => 10,
            "50" => 50,
            "100" => 100,
            _ => return Err("运行时并发变体必须是 10、50 或 100".into()),
        }
    };
    let app_env = match std::env::var("APP_ENV") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "dev".into(),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("APP_ENV 必须是 UTF-8 环境名称".into());
        }
    };
    super::runtime_input::validate(root, &projected, options.suite, concurrency, &app_env)?;
    let contract = config.get("contract").ok_or("运行时配置缺少 contract")?;
    // contract 不含端点、scope、凭据或源码 SHA。
    let input_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(contract)?));
    let driver = crate::workspace::root_dir();
    let driver_fingerprint = super::metadata::collect_source_fingerprints(&driver, None)?.backend;
    let runner_frontend_fingerprint =
        super::metadata::collect_source_fingerprints(runner_frontend_root, None)?.backend;
    let mut source_fingerprints =
        super::metadata::collect_source_fingerprints(root, Some(frontend_root))?;
    source_fingerprints.runner_frontend = Some(runner_frontend_fingerprint.clone());
    let input = super::runtime_native::Input {
        backend: root.into(),
        frontend: frontend_root.into(),
        runner_frontend: runner_frontend_root.into(),
        driver,
        output: output.into(),
        suite: options
            .suite
            .as_str()
            .strip_prefix("runtime-")
            .ok_or("必须选择运行时 suite")?
            .into(),
        cache: options.cache_state.as_str().into(),
        concurrency,
        input_sha256,
        driver_fingerprint,
        runner_frontend_fingerprint,
        source_fingerprints,
        app_env,
        native_tool: super::job_evidence::artifact::current_binding()?,
    };
    input.validate()?;
    Ok(input)
}

pub(super) fn read(
    target: &Path,
    label: &str,
    suite: DevexSuite,
    success: bool,
) -> Result<Option<RuntimeEvidence>> {
    let path = target.join(format!("runtime-{label}.json"));
    if !path.exists() && !success {
        return Ok(None);
    }
    let evidence: RuntimeEvidence = serde_json::from_slice(&fs::read(path)?)?;
    validate(&evidence, suite, success)?;
    Ok(Some(evidence))
}

pub(super) fn validate(evidence: &RuntimeEvidence, suite: DevexSuite, success: bool) -> Result<()> {
    if evidence.input_sha256.len() != 64
        || !evidence.input_sha256.bytes().all(|v| v.is_ascii_hexdigit())
        || evidence.scenarios.is_empty()
        || evidence
            .completed_cycles
            .checked_add(evidence.failed_cycles)
            .is_none_or(|total| total == 0)
    {
        return Err("运行时证据缺少负载指纹、场景或有效周期计数".into());
    }
    if success && (evidence.failed_cycles != 0 || evidence.session_failures != 0) {
        return Err("运行时样本含失败周期或会话失败".into());
    }
    if success && evidence.network_requests == 0 {
        return Err("运行时成功样本没有真实网络请求".into());
    }
    validate_cycle_totals(evidence, suite)?;
    for value in [
        evidence.peak_resident_memory_bytes,
        evidence.cpu_seconds,
        evidence.peak_database_connections,
    ] {
        finite(value)?;
    }
    for (key, metric) in &evidence.scenarios {
        if key.is_empty()
            || key.len() > 64
            || metric
                .completed
                .checked_add(metric.failed)
                .is_none_or(|total| total == 0)
        {
            return Err("运行时场景名或周期数无效".into());
        }
        for value in [
            metric.elapsed_ms,
            metric.p50_ms,
            metric.p95_ms,
            metric.p99_ms,
            metric.throughput,
        ]
        .into_iter()
        .chain(metric.queue_p95_ms)
        .chain(metric.execution_p95_ms)
        {
            finite(value)?;
        }
        if metric.p50_ms > metric.p95_ms
            || metric.p95_ms > metric.p99_ms
            || (success && (metric.failed > 0 || metric.completed == 0))
            || (success
                && (metric.elapsed_ms == 0.0 || metric.p99_ms == 0.0 || metric.throughput == 0.0))
        {
            return Err("运行时样本含失败周期或无效端到端指标".into());
        }
    }
    if success && evidence.collector_failures != 0 {
        return Err("运行时指标采集失败".into());
    }
    Ok(())
}

fn validate_cycle_totals(evidence: &RuntimeEvidence, suite: DevexSuite) -> Result<()> {
    let expected = (evidence.completed_cycles, evidence.failed_cycles);
    let matches = match suite {
        DevexSuite::RuntimeHomepage => {
            evidence
                .scenarios
                .keys()
                .map(String::as_str)
                .eq(["content", "lcp", "navigation"])
                && evidence
                    .scenarios
                    .values()
                    .all(|row| (row.completed, row.failed) == expected)
        }
        DevexSuite::RuntimeApi | DevexSuite::RuntimeJobs | DevexSuite::RuntimeTenants => {
            evidence
                .scenarios
                .keys()
                .map(String::as_str)
                .eq(expected_scenarios(suite).iter().copied())
                && evidence
                    .scenarios
                    .values()
                    .try_fold((0u64, 0u64), |(completed, failed), row| {
                        Some((
                            completed.checked_add(row.completed)?,
                            failed.checked_add(row.failed)?,
                        ))
                    })
                    == Some(expected)
        }
        _ => false,
    };
    if !matches {
        return Err("运行时总周期数与当前 suite 的场景计数不一致".into());
    }
    Ok(())
}

fn expected_scenarios(suite: DevexSuite) -> &'static [&'static str] {
    match suite {
        DevexSuite::RuntimeApi => &["filter", "list", "login-refresh", "page", "write"],
        DevexSuite::RuntimeJobs => &["export", "import", "message", "schedule"],
        DevexSuite::RuntimeTenants => &["export", "list", "write"],
        _ => unreachable!("已限定运行时 suite"),
    }
}

fn finite(value: f64) -> Result<()> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err("运行时指标必须是非负有限数".into())
    }
}

pub(super) fn summarize(records: &[&SampleRecord]) -> Result<Option<RuntimeSummary>> {
    let samples = records
        .iter()
        .filter_map(|record| record.runtime.as_ref())
        .collect::<Vec<_>>();
    let Some(first) = samples.first() else {
        return Ok(None);
    };
    for sample in &samples {
        if sample.input_sha256 != first.input_sha256
            || sample.cache_state != first.cache_state
            || !sample.scenarios.keys().eq(first.scenarios.keys())
        {
            return Err("运行时采样的负载、冷暖条件或场景集合发生变化".into());
        }
    }
    let successful = records
        .iter()
        .filter(|r| r.status == SampleStatus::Passed)
        .filter_map(|r| r.runtime.as_ref())
        .collect::<Vec<_>>();
    let mut scenarios = BTreeMap::new();
    for key in first.scenarios.keys() {
        let rows = successful
            .iter()
            .map(|sample| &sample.scenarios[key])
            .collect::<Vec<_>>();
        if rows.is_empty() {
            continue;
        }
        for row in &rows {
            if row.queue_p95_ms.is_some() != rows[0].queue_p95_ms.is_some()
                || row.execution_p95_ms.is_some() != rows[0].execution_p95_ms.is_some()
            {
                return Err("运行时样本的排队或执行指标存在部分缺失".into());
            }
        }
        let values = |pick: fn(&RuntimeMetric) -> f64| {
            distribution(&rows.iter().map(|v| pick(v)).collect::<Vec<_>>()).expect("存在成功样本")
        };
        scenarios.insert(
            key.clone(),
            ScenarioSummary {
                p50_ms: values(|v| v.p50_ms),
                p95_ms: values(|v| v.p95_ms),
                p99_ms: values(|v| v.p99_ms),
                throughput: values(|v| v.throughput),
                queue_p95_ms: distribution(
                    &rows
                        .iter()
                        .filter_map(|v| v.queue_p95_ms)
                        .collect::<Vec<_>>(),
                ),
                execution_p95_ms: distribution(
                    &rows
                        .iter()
                        .filter_map(|v| v.execution_p95_ms)
                        .collect::<Vec<_>>(),
                ),
            },
        );
    }
    let completed_cycles = samples.iter().map(|sample| sample.completed_cycles).sum();
    let failed_cycles = samples.iter().map(|sample| sample.failed_cycles).sum();
    Ok(Some(RuntimeSummary {
        network_requests: samples.iter().map(|v| v.network_requests).sum(),
        completed_cycles,
        failed_cycles,
        cycle_failure_rate: failed_cycles as f64 / (completed_cycles as f64 + failed_cycles as f64),
        session_failures: samples.iter().map(|sample| sample.session_failures).sum(),
        input_sha256: first.input_sha256.clone(),
        scenarios,
        peak_resident_memory_bytes: samples
            .iter()
            .map(|v| v.peak_resident_memory_bytes)
            .fold(0.0, f64::max),
        peak_database_connections: samples
            .iter()
            .map(|v| v.peak_database_connections)
            .fold(0.0, f64::max),
        cpu_seconds: distribution(&samples.iter().map(|v| v.cpu_seconds).collect::<Vec<_>>())
            .expect("存在运行时样本"),
        collector_failures: samples.iter().map(|v| v.collector_failures).sum(),
    }))
}
