use serde::Serialize;

use crate::Result;

use super::{
    super::model::{CacheState, DevexSuite},
    Distribution, RunSummary, SccacheDelta, percentage_change,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComparisonCheck {
    pub(crate) name: &'static str,
    pub(crate) requirement: &'static str,
    pub(crate) observed: String,
    pub(crate) passed: bool,
}

pub(crate) fn checks(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_duration: &Distribution,
    candidate_duration: &Distribution,
) -> Result<Vec<ComparisonCheck>> {
    let mut checks = duration_acceptance(baseline, baseline_duration, candidate_duration);
    if baseline.suite.uses_sccache() {
        checks.extend(sccache_acceptance(baseline, candidate)?);
    }
    if baseline.suite.is_runtime() {
        checks.extend(runtime_checks(baseline, candidate)?);
    } else {
        checks.push(ComparisonCheck {
            name: "保留场景 P95 回退",
            requirement: "<= 10%",
            observed: percentage_change(baseline_duration.p95, candidate_duration.p95),
            passed: baseline_duration.p95 > 0.0
                && candidate_duration.p95 <= baseline_duration.p95 * 1.10,
        });
    }
    Ok(checks)
}

fn runtime_checks(baseline: &RunSummary, candidate: &RunSummary) -> Result<Vec<ComparisonCheck>> {
    let base = baseline.runtime.as_ref().ok_or("运行时基线缺少证据")?;
    let next = candidate.runtime.as_ref().ok_or("运行时候选缺少证据")?;
    if base.input_sha256 != next.input_sha256 || !base.scenarios.keys().eq(next.scenarios.keys()) {
        return Err("运行时环境、数据集或场景不可比".into());
    }
    let mut checks = vec![
        zero("基线失败周期", base.failed_cycles),
        zero("候选失败周期", next.failed_cycles),
        zero("基线会话失败", base.session_failures),
        zero("候选会话失败", next.session_failures),
        zero("基线采集失败", base.collector_failures),
        zero("候选采集失败", next.collector_failures),
    ];
    for (name, original) in &base.scenarios {
        let current = &next.scenarios[name];
        for (metric, before, after) in [
            ("端到端", Some(&original.p95_ms), Some(&current.p95_ms)),
            (
                "排队",
                original.queue_p95_ms.as_ref(),
                current.queue_p95_ms.as_ref(),
            ),
            (
                "执行",
                original.execution_p95_ms.as_ref(),
                current.execution_p95_ms.as_ref(),
            ),
        ] {
            match (before, after) {
                (Some(a), Some(b)) => checks.push(ComparisonCheck {
                    name: "保留业务周期 P95 的样本 P95 回退",
                    requirement: "<= 10%",
                    observed: format!("{name}/{metric}：{:.2} → {:.2} ms", a.p95, b.p95),
                    passed: b.p95 <= a.p95 * 1.10,
                }),
                (None, None) => {}
                _ => return Err("后台计时范围发生变化，不能比较".into()),
            }
        }
    }
    Ok(checks)
}

pub(crate) fn duration_acceptance(
    baseline: &RunSummary,
    baseline_duration: &Distribution,
    candidate_duration: &Distribution,
) -> Vec<ComparisonCheck> {
    match (baseline.suite, baseline.variant.as_str()) {
        (DevexSuite::RustColdBuild, "api") => vec![improvement_check(
            "API 冷构建 P50 改善",
            ">= 10%",
            baseline_duration.p50,
            candidate_duration.p50,
            0.90,
        )],
        (DevexSuite::RustColdBuild, "worker") => vec![improvement_check(
            "Worker 冷构建 P50 改善",
            ">= 20%",
            baseline_duration.p50,
            candidate_duration.p50,
            0.80,
        )],
        (DevexSuite::RustColdBuild, "migrate") => vec![improvement_check(
            "migrate 冷构建 P50 改善",
            ">= 25%",
            baseline_duration.p50,
            candidate_duration.p50,
            0.75,
        )],
        (DevexSuite::CargoDevSave, "config-only") => vec![improvement_check(
            "配置保存 P50 改善",
            ">= 70%",
            baseline_duration.p50,
            candidate_duration.p50,
            0.30,
        )],
        (DevexSuite::CargoDevSave, "api-only" | "worker-only") => {
            vec![improvement_check(
                "单目标保存 P50 改善",
                ">= 30%",
                baseline_duration.p50,
                candidate_duration.p50,
                0.70,
            )]
        }
        (DevexSuite::CargoDevSave, "cancellation") => vec![maximum_check(
            "过期构建取消 P95",
            "<= 1000 ms",
            candidate_duration.p95,
            1_000.0,
        )],
        (DevexSuite::RustIncremental, "application") => vec![maximum_check(
            "application 增量编辑 P50",
            "<= 12000 ms",
            candidate_duration.p50,
            12_000.0,
        )],
        (DevexSuite::FrontendFast, "default") => vec![maximum_check(
            "前端 check P95",
            "<= 12000 ms",
            candidate_duration.p95,
            12_000.0,
        )],
        (DevexSuite::ResourceGate, "auto") if baseline.cache_state == CacheState::Warm => {
            vec![maximum_check(
                "Resource gate P95",
                "<= 60000 ms",
                candidate_duration.p95,
                60_000.0,
            )]
        }
        (DevexSuite::RustGate, "default") => {
            duration_checks(baseline_duration, candidate_duration).to_vec()
        }
        _ => Vec::new(),
    }
}

fn sccache_acceptance(
    baseline: &RunSummary,
    candidate: &RunSummary,
) -> Result<Vec<ComparisonCheck>> {
    if baseline.cache_state != CacheState::Warm {
        return Err("sccache 验收只接受 warm cache".into());
    }
    let baseline_stats = baseline.sccache.as_ref().ok_or("基线缺少 sccache 统计")?;
    let candidate_stats = candidate.sccache.as_ref().ok_or("候选缺少 sccache 统计")?;
    let baseline_cacheable = cacheable_requests(baseline_stats)?;
    let candidate_cacheable = cacheable_requests(candidate_stats)?;
    Ok(vec![
        positive("基线编译请求", baseline_stats.compile_requests),
        positive("候选编译请求", candidate_stats.compile_requests),
        positive("基线可缓存请求", baseline_cacheable),
        positive("候选可缓存请求", candidate_cacheable),
        zero("基线缓存错误", baseline_stats.cache_errors),
        zero("候选缓存错误", candidate_stats.cache_errors),
        ComparisonCheck {
            name: "候选暖缓存命中率",
            requirement: ">= 80%",
            observed: format_rate(candidate_stats.hit_rate),
            passed: candidate_stats.hit_rate.is_some_and(|rate| rate >= 0.8),
        },
        ComparisonCheck {
            name: "实际不可缓存编译改善",
            requirement: ">= 50%（基线为 0 时候选也必须为 0）",
            observed: not_cacheable_observation(
                baseline_stats.not_cacheable_compilations,
                candidate_stats.not_cacheable_compilations,
            ),
            passed: not_cacheable_passes(
                baseline_stats.not_cacheable_compilations,
                candidate_stats.not_cacheable_compilations,
            ),
        },
    ])
}

fn improvement_check(
    name: &'static str,
    requirement: &'static str,
    baseline: f64,
    candidate: f64,
    maximum_ratio: f64,
) -> ComparisonCheck {
    ComparisonCheck {
        name,
        requirement,
        observed: improvement(baseline, candidate),
        passed: baseline > 0.0 && candidate <= baseline * maximum_ratio,
    }
}

fn maximum_check(
    name: &'static str,
    requirement: &'static str,
    candidate: f64,
    maximum: f64,
) -> ComparisonCheck {
    ComparisonCheck {
        name,
        requirement,
        observed: format!("{candidate:.1} ms"),
        passed: candidate <= maximum,
    }
}

fn duration_checks(baseline: &Distribution, candidate: &Distribution) -> [ComparisonCheck; 2] {
    [
        ComparisonCheck {
            name: "rust-gate P50 改善",
            requirement: ">= 10%",
            observed: improvement(baseline.p50, candidate.p50),
            passed: candidate.p50 <= baseline.p50 * 0.9,
        },
        ComparisonCheck {
            name: "rust-gate P95 回退",
            requirement: "<= 5%",
            observed: percentage_change(baseline.p95, candidate.p95),
            passed: candidate.p95 <= baseline.p95 * 1.05,
        },
    ]
}

fn positive(name: &'static str, value: u64) -> ComparisonCheck {
    ComparisonCheck {
        name,
        requirement: "> 0",
        observed: value.to_string(),
        passed: value > 0,
    }
}

fn zero(name: &'static str, value: u64) -> ComparisonCheck {
    ComparisonCheck {
        name,
        requirement: "= 0",
        observed: value.to_string(),
        passed: value == 0,
    }
}

fn cacheable_requests(stats: &SccacheDelta) -> Result<u64> {
    stats
        .cache_hits
        .checked_add(stats.cache_misses)
        .ok_or_else(|| "sccache 可缓存请求计数溢出".into())
}

fn format_rate(rate: Option<f64>) -> String {
    rate.map_or_else(|| "n/a".to_owned(), |rate| format!("{:.1}%", rate * 100.0))
}

fn not_cacheable_passes(baseline: u64, candidate: u64) -> bool {
    if baseline == 0 {
        candidate == 0
    } else {
        u128::from(candidate) * 2 <= u128::from(baseline)
    }
}

fn not_cacheable_observation(baseline: u64, candidate: u64) -> String {
    if baseline == 0 {
        return format!("基线 {baseline} / 候选 {candidate}");
    }
    format!(
        "基线 {baseline} / 候选 {candidate} / 改善 {:.1}%",
        (1.0 - candidate as f64 / baseline as f64) * 100.0
    )
}

fn improvement(baseline: f64, candidate: f64) -> String {
    if baseline == 0.0 {
        return "n/a".to_owned();
    }
    format!("{:.1}%", (1.0 - candidate / baseline) * 100.0)
}
