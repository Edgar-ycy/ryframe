use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::{Result, dev::ReadyKind};

use super::{
    metadata::{RunMetadata, SourceFingerprints},
    model::{CacheState, DevexSuite, PairedArm, PairingMetadata},
};

#[path = "report/baseline_contract.rs"]
mod baseline_contract;
#[path = "report/markdown.rs"]
mod markdown;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum SampleKind {
    Warmup,
    Measurement,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum SampleStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct SampleRecord {
    pub(super) schema_version: u8,
    pub(super) run_id: String,
    pub(super) sequence: usize,
    pub(super) kind: SampleKind,
    pub(super) cache_state: CacheState,
    pub(super) started_at: String,
    pub(super) duration_ms: f64,
    #[serde(default)]
    pub(super) cargo_invocations: Option<usize>,
    #[serde(default)]
    pub(super) ready_kind: Option<ReadyKind>,
    pub(super) status: SampleStatus,
    pub(super) exit_code: Option<i32>,
    pub(super) target_directory: String,
    #[serde(default)]
    pub(super) arm: Option<PairedArm>,
    #[serde(default)]
    pub(super) pair: Option<usize>,
    #[serde(default)]
    pub(super) order: Option<usize>,
    #[serde(default)]
    pub(super) source_fingerprints: Option<SourceFingerprints>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RunSummary {
    pub(crate) schema_version: u8,
    pub(crate) run_id: String,
    pub(crate) suite: DevexSuite,
    pub(crate) variant: String,
    pub(crate) cache_state: CacheState,
    pub(crate) compile_surface_fingerprint: String,
    pub(crate) input_fingerprint: String,
    pub(crate) requested_runs: usize,
    pub(crate) samples: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) duration_ms: Option<Distribution>,
    #[serde(default)]
    pub(crate) sccache_version: Option<String>,
    pub(crate) sccache: Option<SccacheDelta>,
    #[serde(default)]
    pub(crate) pairing: Option<PairingMetadata>,
    #[serde(default)]
    pub(super) source_fingerprints: SourceFingerprints,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Distribution {
    pub(crate) min: f64,
    pub(crate) p50: f64,
    pub(crate) p95: f64,
    pub(crate) max: f64,
    pub(crate) mean: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SccacheDelta {
    pub(crate) compile_requests: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    pub(crate) not_cacheable: u64,
    pub(crate) cache_errors: u64,
    pub(crate) hit_rate: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct SccacheCounters {
    compile_requests: u64,
    cache_hits: u64,
    cache_misses: u64,
    not_cacheable: u64,
    cache_errors: u64,
}

pub(super) fn write_metadata(run_dir: &Path, metadata: &RunMetadata) -> Result<()> {
    write_json(run_dir.join("metadata.json"), metadata)
}

pub(super) fn append_sample(run_dir: &Path, sample: &SampleRecord) -> Result<()> {
    let path = run_dir.join("samples.jsonl");
    let mut output = OpenOptions::new().create(true).append(true).open(&path)?;
    serde_json::to_writer(&mut output, sample)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

pub(crate) fn summarize(run_dir: &Path) -> Result<RunSummary> {
    let metadata = read_metadata(run_dir)?;
    let records = read_samples(run_dir)?;
    validate_samples(&metadata, &records)?;
    let measurements = records
        .iter()
        .filter(|sample| sample.kind == SampleKind::Measurement)
        .collect::<Vec<_>>();
    let passed = measurements
        .iter()
        .filter(|sample| sample.status == SampleStatus::Passed)
        .count();
    let failed = measurements.len().saturating_sub(passed);
    let durations = measurements
        .iter()
        .filter(|sample| sample.status == SampleStatus::Passed)
        .map(|sample| sample.duration_ms)
        .collect::<Vec<_>>();
    let source_fingerprints = metadata.source_fingerprints();
    let pairing = metadata.pairing.clone();
    let summary = RunSummary {
        schema_version: 1,
        run_id: metadata.run_id,
        suite: metadata.suite,
        variant: metadata.variant,
        cache_state: metadata.cache_state,
        compile_surface_fingerprint: metadata.compile_surface_fingerprint,
        input_fingerprint: metadata.input_fingerprint,
        requested_runs: metadata.requested_runs,
        samples: measurements.len(),
        passed,
        failed,
        duration_ms: distribution(&durations),
        sccache_version: metadata.toolchain.sccache,
        sccache: read_sccache_delta(run_dir)?,
        pairing,
        source_fingerprints,
    };
    write_json(run_dir.join("summary.json"), &summary)?;
    fs::write(run_dir.join("summary.md"), markdown::render(&summary))?;
    Ok(summary)
}

pub(crate) fn compare(baseline_dir: &Path, candidate_dir: &Path) -> Result<String> {
    let baseline = summarize(baseline_dir)?;
    let candidate = summarize(candidate_dir)?;
    let baseline_records = read_samples(baseline_dir)?;
    let candidate_records = read_samples(candidate_dir)?;
    ensure_comparable(&baseline, &candidate, &baseline_records, &candidate_records)?;
    let baseline_duration = baseline
        .duration_ms
        .as_ref()
        .ok_or("基线没有成功的测量样本")?;
    let candidate_duration = candidate
        .duration_ms
        .as_ref()
        .ok_or("候选没有成功的测量样本")?;
    let sccache_version = baseline
        .sccache_version
        .as_deref()
        .map_or_else(|| "n/a".to_owned(), ToOwned::to_owned);
    Ok(format!(
        "# DevEx 对比\n\n\
         - suite：`{}`\n\
         - variant：`{}`\n\
         - cache：`{}`\n\
         - paired comparison：`{}`\n\
         - execution surface：`{}`\n\
         - sccache executable：`{}`\n\
         - input：基线 `{}` / 候选 `{}`\n\n\
         | 指标 | 基线 | 候选 | 变化 |\n\
         | --- | ---: | ---: | ---: |\n\
         | P50 | {:.1} ms | {:.1} ms | {} |\n\
         | P95 | {:.1} ms | {:.1} ms | {} |\n",
        baseline.suite.as_str(),
        baseline.variant,
        baseline.cache_state.as_str(),
        baseline
            .pairing
            .as_ref()
            .expect("ensure_comparable 已校验 pairing")
            .comparison_id,
        baseline.compile_surface_fingerprint,
        sccache_version,
        baseline.input_fingerprint,
        candidate.input_fingerprint,
        baseline_duration.p50,
        candidate_duration.p50,
        percentage_change(baseline_duration.p50, candidate_duration.p50),
        baseline_duration.p95,
        candidate_duration.p95,
        percentage_change(baseline_duration.p95, candidate_duration.p95),
    ))
}

pub(crate) fn distribution(values: &[f64]) -> Option<Distribution> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let sum = sorted.iter().sum::<f64>();
    Some(Distribution {
        min: sorted[0],
        p50: nearest_rank(&sorted, 0.50),
        p95: nearest_rank(&sorted, 0.95),
        max: sorted[sorted.len() - 1],
        mean: sum / sorted.len() as f64,
    })
}

fn nearest_rank(sorted: &[f64], percentile: f64) -> f64 {
    let rank = (percentile * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn read_metadata(run_dir: &Path) -> Result<RunMetadata> {
    let path = run_dir.join("metadata.json");
    let metadata: RunMetadata = serde_json::from_slice(&fs::read(&path)?)?;
    if metadata.schema_version != 1 {
        return Err(format!(
            "不支持的 DevEx metadata schema：{}",
            metadata.schema_version
        )
        .into());
    }
    Ok(metadata)
}

fn read_samples(run_dir: &Path) -> Result<Vec<SampleRecord>> {
    let path = run_dir.join("samples.jsonl");
    let input = fs::File::open(&path)?;
    BufReader::new(input)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| match line {
            Ok(line) if line.trim().is_empty() => None,
            line => Some((index, line)),
        })
        .map(|(index, line)| {
            let line = line?;
            serde_json::from_str(&line).map_err(|error| {
                format!(
                    "{} 第 {} 行不是有效样本：{error}",
                    path.display(),
                    index + 1
                )
                .into()
            })
        })
        .collect()
}

fn validate_samples(metadata: &RunMetadata, samples: &[SampleRecord]) -> Result<()> {
    let mut measurement_sequences = BTreeSet::new();
    for sample in samples {
        if sample.schema_version != 1 {
            return Err(format!("不支持的 DevEx sample schema：{}", sample.schema_version).into());
        }
        if sample.run_id != metadata.run_id || sample.cache_state != metadata.cache_state {
            return Err(format!("样本 {} 与 metadata 不属于同一次运行", sample.sequence).into());
        }
        if !sample.duration_ms.is_finite() || sample.duration_ms < 0.0 {
            return Err(format!("样本 {} 的耗时无效", sample.sequence).into());
        }
        if metadata.suite == DevexSuite::CargoDevSave && sample.cargo_invocations.is_none() {
            return Err(
                format!("cargo-dev-save 样本 {} 缺少 Cargo 调用数", sample.sequence).into(),
            );
        }
        if metadata.suite == DevexSuite::CargoDevSave && sample.ready_kind.is_none() {
            return Err(format!("cargo-dev-save 样本 {} 缺少就绪结果", sample.sequence).into());
        }
        if sample.kind == SampleKind::Measurement
            && (!measurement_sequences.insert(sample.sequence)
                || !(1..=metadata.requested_runs).contains(&sample.sequence))
        {
            return Err(format!("测量样本序号无效或重复：{}", sample.sequence).into());
        }
    }
    Ok(())
}

fn ensure_comparable(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_records: &[SampleRecord],
    candidate_records: &[SampleRecord],
) -> Result<()> {
    ensure_complete("基线", baseline)?;
    ensure_complete("候选", candidate)?;
    if baseline.suite != candidate.suite {
        return Err("DevEx 对比要求 suite 相同".into());
    }
    if baseline.variant != candidate.variant {
        return Err("DevEx 对比要求工作负载 variant 相同".into());
    }
    if baseline.cache_state != candidate.cache_state {
        return Err("DevEx 对比要求 cache state 相同".into());
    }
    if baseline.sccache_version != candidate.sccache_version {
        return Err("DevEx 对比要求 sccache 可执行版本相同".into());
    }
    if baseline.compile_surface_fingerprint != candidate.compile_surface_fingerprint {
        return Err("DevEx 对比的 compile_surface_fingerprint 不一致，拒绝生成误导结论".into());
    }
    if baseline.samples != candidate.samples {
        return Err("DevEx 对比要求两侧具有相同样本数".into());
    }
    validate_paired_records(baseline, candidate, baseline_records, candidate_records)
}

fn validate_paired_records(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_records: &[SampleRecord],
    candidate_records: &[SampleRecord],
) -> Result<()> {
    let baseline_pairing = baseline
        .pairing
        .as_ref()
        .ok_or("DevEx compare 只接受 paired runner 生成的基线")?;
    let candidate_pairing = candidate
        .pairing
        .as_ref()
        .ok_or("DevEx compare 只接受 paired runner 生成的候选")?;
    if baseline_pairing.comparison_id.trim().is_empty()
        || baseline_pairing.comparison_id != candidate_pairing.comparison_id
        || baseline_pairing.arm != PairedArm::Baseline
        || candidate_pairing.arm != PairedArm::Candidate
    {
        return Err("DevEx paired comparison id 或 arm 不匹配".into());
    }
    baseline_contract::validate(
        baseline,
        candidate,
        baseline_records,
        candidate_records,
        baseline_pairing,
        candidate_pairing,
    )?;
    validate_source_fingerprints(baseline)?;
    validate_source_fingerprints(candidate)?;
    let baseline_samples = measurement_samples(baseline_records);
    let candidate_samples = measurement_samples(candidate_records);
    for pair in 1..=baseline.requested_runs {
        validate_paired_sample(
            baseline_samples[pair - 1],
            baseline,
            PairedArm::Baseline,
            pair,
            paired_order(pair, PairedArm::Baseline),
        )?;
        validate_paired_sample(
            candidate_samples[pair - 1],
            candidate,
            PairedArm::Candidate,
            pair,
            paired_order(pair, PairedArm::Candidate),
        )?;
    }
    validate_timestamp_order(&baseline_samples, &candidate_samples)
}

fn validate_source_fingerprints(summary: &RunSummary) -> Result<()> {
    let frontend_required = summary
        .suite
        .definition(&summary.variant)
        .map_err(|error| format!("paired metadata 变体无效：{error}"))?
        .requires_frontend;
    if summary.source_fingerprints.backend.trim().is_empty()
        || (frontend_required
            && summary
                .source_fingerprints
                .frontend
                .as_deref()
                .is_none_or(|fingerprint| fingerprint.trim().is_empty()))
    {
        return Err("DevEx paired 源码指纹缺失".into());
    }
    Ok(())
}

fn measurement_samples(records: &[SampleRecord]) -> Vec<&SampleRecord> {
    let mut samples = records
        .iter()
        .filter(|sample| sample.kind == SampleKind::Measurement)
        .collect::<Vec<_>>();
    samples.sort_by_key(|sample| sample.sequence);
    samples
}

fn validate_paired_sample(
    sample: &SampleRecord,
    summary: &RunSummary,
    arm: PairedArm,
    pair: usize,
    order: usize,
) -> Result<()> {
    if sample.sequence != pair
        || sample.arm != Some(arm)
        || sample.pair != Some(pair)
        || sample.order != Some(order)
        || sample.source_fingerprints.as_ref() != Some(&summary.source_fingerprints)
    {
        return Err(format!(
            "DevEx paired 样本审计字段不匹配：arm={} pair={pair} order={order}",
            arm.as_str()
        )
        .into());
    }
    Ok(())
}

fn paired_order(pair: usize, arm: PairedArm) -> usize {
    let first = pair * 2 - 1;
    match (pair % 2, arm) {
        (1, PairedArm::Baseline) | (0, PairedArm::Candidate) => first,
        _ => first + 1,
    }
}

fn validate_timestamp_order(baseline: &[&SampleRecord], candidate: &[&SampleRecord]) -> Result<()> {
    let mut ordered = baseline
        .iter()
        .chain(candidate)
        .copied()
        .collect::<Vec<_>>();
    ordered.sort_by_key(|sample| sample.order);
    let mut previous = None;
    for sample in ordered {
        let started = chrono::DateTime::parse_from_rfc3339(&sample.started_at)
            .map_err(|error| format!("paired 样本时间戳无效：{error}"))?;
        if previous.is_some_and(|previous| started < previous) {
            return Err("DevEx paired 样本时间戳与 ABBA order 不一致".into());
        }
        previous = Some(started);
    }
    Ok(())
}

fn ensure_complete(label: &str, summary: &RunSummary) -> Result<()> {
    if summary.samples != summary.requested_runs
        || summary.passed != summary.requested_runs
        || summary.failed != 0
    {
        return Err(format!(
            "DevEx {label}样本不完整：请求 {}，记录 {}，通过 {}，失败 {}",
            summary.requested_runs, summary.samples, summary.passed, summary.failed
        )
        .into());
    }
    Ok(())
}

fn read_sccache_delta(run_dir: &Path) -> Result<Option<SccacheDelta>> {
    let before_path = run_dir.join("sccache-before.json");
    let after_path = run_dir.join("sccache-after.json");
    if !before_path.is_file() && !after_path.is_file() {
        return Ok(None);
    }
    if !before_path.is_file() || !after_path.is_file() {
        return Err("sccache 统计快照不完整".into());
    }
    let before = parse_sccache_counters(&fs::read(before_path)?)?;
    let after = parse_sccache_counters(&fs::read(after_path)?)?;
    let cache_hits = after.cache_hits.saturating_sub(before.cache_hits);
    let cache_misses = after.cache_misses.saturating_sub(before.cache_misses);
    let cacheable = cache_hits.saturating_add(cache_misses);
    Ok(Some(SccacheDelta {
        compile_requests: after
            .compile_requests
            .saturating_sub(before.compile_requests),
        cache_hits,
        cache_misses,
        not_cacheable: after.not_cacheable.saturating_sub(before.not_cacheable),
        cache_errors: after.cache_errors.saturating_sub(before.cache_errors),
        hit_rate: (cacheable > 0).then(|| cache_hits as f64 / cacheable as f64),
    }))
}

fn parse_sccache_counters(document: &[u8]) -> Result<SccacheCounters> {
    let value: serde_json::Value = serde_json::from_slice(document)?;
    let stats = value.get("stats").ok_or("sccache JSON 缺少 stats")?;
    let scalar = |name: &str| {
        stats
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let counts = |name: &str| {
        stats
            .get(name)
            .and_then(|value| value.get("counts"))
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|values| values.values())
            .filter_map(serde_json::Value::as_u64)
            .sum()
    };
    Ok(SccacheCounters {
        compile_requests: scalar("compile_requests"),
        cache_hits: counts("cache_hits"),
        cache_misses: counts("cache_misses"),
        not_cacheable: scalar("requests_not_cacheable"),
        cache_errors: counts("cache_errors")
            + scalar("cache_timeouts")
            + scalar("cache_read_errors")
            + scalar("cache_write_errors")
            + scalar("dist_errors"),
    })
}

fn percentage_change(baseline: f64, candidate: f64) -> String {
    if baseline == 0.0 {
        return "n/a".to_owned();
    }
    format!("{:+.1}%", (candidate / baseline - 1.0) * 100.0)
}

fn write_json(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    let mut document = serde_json::to_vec_pretty(value)?;
    document.push(b'\n');
    fs::write(path, document)?;
    Ok(())
}
