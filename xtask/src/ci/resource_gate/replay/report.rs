use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::Value;

use super::{
    environment::{ReplayTools, sccache_error_count},
    manifest::{MINIMUM_SUCCESSFUL_CASES, ReplayCase},
    path::{canonical_path, resolve_requested_path, validate_inside},
    workspace::RepositoryIdentity,
};

pub(crate) const REPORT_FORMAT_VERSION: u64 = 2;
pub(crate) const TARGETED_P95_LIMIT_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceGateDecision {
    pub(crate) recognized: bool,
    pub(crate) mode: String,
    pub(crate) fallback: Option<String>,
    pub(crate) steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CommandResult {
    pub(crate) passed: bool,
    pub(crate) return_code: i32,
    pub(crate) duration_ms: u64,
    pub(crate) order: i64,
    pub(crate) decision: ResourceGateDecision,
    pub(crate) target_fingerprint: String,
    pub(crate) output_fingerprint: String,
    pub(crate) failure_tail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayResult {
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) frontend_base: String,
    pub(crate) frontend_head: String,
    pub(crate) targeted_mode: String,
    pub(crate) expected: String,
    pub(crate) targeted: CommandResult,
    pub(crate) full: CommandResult,
    pub(crate) matches: bool,
}

impl ReplayResult {
    pub(crate) fn from_arms(
        case: &ReplayCase,
        targeted: CommandResult,
        full: CommandResult,
    ) -> Self {
        let expected_pass = case.expected_pass();
        let matches = targeted.passed == expected_pass
            && full.passed == expected_pass
            && targeted.decision.mode == case.targeted_mode
            && full.decision.mode == "full";
        Self {
            name: case.name.clone(),
            category: case.category.clone(),
            base: case.base.clone(),
            head: case.head.clone(),
            frontend_base: case.frontend_base.clone(),
            frontend_head: case.frontend_head.clone(),
            targeted_mode: case.targeted_mode.clone(),
            expected: case.expected.clone(),
            targeted,
            full,
            matches,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayPrime {
    pub(crate) case: String,
    pub(crate) rust_crates: Vec<String>,
    pub(crate) change_count: usize,
    pub(crate) result: CommandResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryRecord {
    pub(crate) root_fingerprint: String,
    pub(crate) common_dir_fingerprint: String,
    pub(crate) remote_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayEvidence {
    pub(crate) integration_files: Value,
    pub(crate) manifest_fingerprint: String,
    pub(crate) runner_fingerprint: String,
    pub(crate) runner_commit: String,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) environment_fingerprint: String,
    pub(crate) tools: BTreeMap<String, String>,
    pub(crate) tools_fingerprint: String,
    pub(crate) repositories: BTreeMap<String, RepositoryRecord>,
    pub(crate) primes: Vec<ReplayPrime>,
    pub(crate) sccache_before: Value,
    pub(crate) sccache_after: Value,
}

#[derive(Clone)]
pub(crate) struct ReplayRun {
    pub(crate) results: Vec<ReplayResult>,
    pub(crate) evidence: ReplayEvidence,
    pub(crate) activation: Option<std::sync::Arc<super::activation::Activation>>,
}

impl std::fmt::Debug for ReplayRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReplayRun")
            .field("results", &self.results)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayReport<'a> {
    format_version: u64,
    case_count: usize,
    successful_targeted_case_count: usize,
    zero_divergence: bool,
    targeted_p95_ms: Option<u64>,
    targeted_p95_limit_ms: u64,
    targeted_within_budget: bool,
    replay_coverage_eligible: bool,
    sccache_cache_errors: Option<u64>,
    sccache_healthy: bool,
    activation_eligible: bool,
    evidence: &'a ReplayEvidence,
    cases: &'a [ReplayResult],
}

pub(crate) fn validated_report_path(repository: &Path, value: &Path) -> Result<PathBuf, String> {
    let root = canonical_path(repository)
        .map_err(|error| format!("无法解析 replay 后端仓库 {}：{error}", repository.display()))?;
    let allowed = super::manifest::report_allowed_root(&root);
    let report = resolve_requested_path(&root, value)?;
    validate_inside(&report, &allowed, "replay report")?;
    if report.exists() {
        return Err("replay report 必须使用尚不存在的新文件".to_owned());
    }
    Ok(report)
}

pub(crate) fn successful_targeted_runs(results: &[ReplayResult]) -> Vec<u64> {
    results
        .iter()
        .filter(|result| {
            result.expected == "pass"
                && result.targeted_mode == "targeted"
                && result.targeted.passed
                && result.targeted.decision.mode == "targeted"
        })
        .map(|result| result.targeted.duration_ms)
        .collect()
}

pub(crate) fn write_report(
    path: &Path,
    run: &ReplayRun,
    activation_gate: bool,
) -> Result<(), String> {
    if let Some(activation) = &run.activation {
        activation.verify()?;
    }
    let successful = successful_targeted_runs(&run.results);
    let zero_divergence =
        !run.results.is_empty() && run.results.iter().all(|result| result.matches);
    let targeted_p95_ms = percentile_nearest_rank(&successful, 95);
    let replay_coverage_eligible = successful.len() >= MINIMUM_SUCCESSFUL_CASES;
    let targeted_within_budget = replay_coverage_eligible
        && targeted_p95_ms.is_some_and(|value| value <= TARGETED_P95_LIMIT_MS);
    let cache_errors = activation_gate
        .then(|| sccache_error_count(&run.evidence.sccache_after))
        .transpose()?;
    let cache_healthy = cache_errors == Some(0) || !activation_gate;
    let report = ReplayReport {
        format_version: REPORT_FORMAT_VERSION,
        case_count: run.results.len(),
        successful_targeted_case_count: successful.len(),
        zero_divergence,
        targeted_p95_ms,
        targeted_p95_limit_ms: TARGETED_P95_LIMIT_MS,
        targeted_within_budget,
        replay_coverage_eligible,
        sccache_cache_errors: cache_errors,
        sccache_healthy: cache_healthy,
        activation_eligible: replay_activation_eligible(
            activation_gate,
            zero_divergence,
            replay_coverage_eligible,
            cache_healthy,
            targeted_within_budget,
        ),
        evidence: &run.evidence,
        cases: &run.results,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "无法创建 replay report 父目录 {}：{error}",
                parent.display()
            )
        })?;
    }
    let mut body = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("无法序列化 replay report：{error}"))?;
    body.push(b'\n');
    if let Some(activation) = &run.activation {
        activation.verify()?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法创建 replay report {}：{error}", path.display()))?;
    output
        .write_all(&body)
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("无法写入 replay report {}：{error}", path.display()))?;
    if let Some(activation) = &run.activation {
        activation.verify()?;
    }
    Ok(())
}

pub(crate) fn percentile_nearest_rank(samples: &[u64], percentile: usize) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    assert!((1..=100).contains(&percentile), "percentile 必须在 1..=100");
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    let rank = (percentile * ordered.len()).div_ceil(100);
    ordered.get(rank.saturating_sub(1)).copied()
}

pub(crate) fn replay_activation_eligible(
    activation_gate: bool,
    zero_divergence: bool,
    coverage_eligible: bool,
    cache_healthy: bool,
    targeted_within_budget: bool,
) -> bool {
    activation_gate
        && zero_divergence
        && coverage_eligible
        && cache_healthy
        && targeted_within_budget
}

pub(crate) fn activation_cache_error(evidence: &ReplayEvidence) -> Result<Option<String>, String> {
    let errors = sccache_error_count(&evidence.sccache_after)?;
    Ok((errors > 0).then(|| format!("resource gate replay 出现 {errors} 个 sccache 缓存错误")))
}

pub(crate) fn replay_mismatches(results: &[ReplayResult]) -> Vec<String> {
    results
        .iter()
        .filter(|result| !result.matches)
        .map(|result| result.name.clone())
        .collect()
}

pub(crate) fn repository_record(identity: &RepositoryIdentity) -> RepositoryRecord {
    RepositoryRecord {
        root_fingerprint: super::sha256_text(&super::path::normalized_path(&identity.root)),
        common_dir_fingerprint: super::sha256_text(&super::path::normalized_path(
            &identity.common_dir,
        )),
        remote_fingerprint: identity.remote.clone(),
    }
}

pub(crate) fn tools_fingerprint(tools: &ReplayTools) -> Result<String, String> {
    super::sha256_json(&tools.versions)
}

pub(crate) fn environment_fingerprint(
    environment: &BTreeMap<String, String>,
) -> Result<String, String> {
    super::sha256_json(environment)
}
