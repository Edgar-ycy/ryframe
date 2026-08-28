use crate::Result;

use super::{
    super::model::{CacheState, DevexSuite},
    Distribution, RunSummary, SccacheDelta, percentage_change,
};

#[derive(Debug)]
pub(super) struct ComparisonCheck {
    pub(super) name: &'static str,
    pub(super) requirement: &'static str,
    pub(super) observed: String,
    pub(super) passed: bool,
}

pub(super) fn checks(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_duration: &Distribution,
    candidate_duration: &Distribution,
) -> Result<Vec<ComparisonCheck>> {
    if baseline.cache_state != CacheState::Warm || !baseline.suite.uses_sccache() {
        return Ok(Vec::new());
    }
    let baseline_stats = baseline.sccache.as_ref().ok_or("基线缺少 sccache 统计")?;
    let candidate_stats = candidate.sccache.as_ref().ok_or("候选缺少 sccache 统计")?;
    let baseline_cacheable = cacheable_requests(baseline_stats)?;
    let candidate_cacheable = cacheable_requests(candidate_stats)?;
    let mut checks = vec![
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
            name: "不可缓存请求改善",
            requirement: ">= 50%（基线为 0 时候选也必须为 0）",
            observed: not_cacheable_observation(
                baseline_stats.not_cacheable,
                candidate_stats.not_cacheable,
            ),
            passed: not_cacheable_passes(
                baseline_stats.not_cacheable,
                candidate_stats.not_cacheable,
            ),
        },
    ];
    if baseline.suite == DevexSuite::RustGate {
        checks.extend(duration_checks(baseline_duration, candidate_duration));
    }
    Ok(checks)
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
