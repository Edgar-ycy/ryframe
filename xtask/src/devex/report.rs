use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::Result;

use super::{
    metadata::RunMetadata,
    model::{CacheState, DevexSuite},
};

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
    pub(super) status: SampleStatus,
    pub(super) exit_code: Option<i32>,
    pub(super) target_directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RunSummary {
    pub(crate) schema_version: u8,
    pub(crate) run_id: String,
    pub(crate) suite: DevexSuite,
    pub(crate) cache_state: CacheState,
    pub(crate) compile_surface_fingerprint: String,
    pub(crate) samples: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) duration_ms: Option<Distribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Distribution {
    pub(crate) min: f64,
    pub(crate) p50: f64,
    pub(crate) p95: f64,
    pub(crate) max: f64,
    pub(crate) mean: f64,
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
    let summary = RunSummary {
        schema_version: 1,
        run_id: metadata.run_id,
        suite: metadata.suite,
        cache_state: metadata.cache_state,
        compile_surface_fingerprint: metadata.compile_surface_fingerprint,
        samples: measurements.len(),
        passed,
        failed,
        duration_ms: distribution(&durations),
    };
    write_json(run_dir.join("summary.json"), &summary)?;
    fs::write(run_dir.join("summary.md"), summary_markdown(&summary))?;
    Ok(summary)
}

pub(crate) fn compare(baseline_dir: &Path, candidate_dir: &Path) -> Result<String> {
    let baseline = summarize(baseline_dir)?;
    let candidate = summarize(candidate_dir)?;
    ensure_comparable(&baseline, &candidate)?;
    let baseline_duration = baseline
        .duration_ms
        .as_ref()
        .ok_or("基线没有成功的测量样本")?;
    let candidate_duration = candidate
        .duration_ms
        .as_ref()
        .ok_or("候选没有成功的测量样本")?;
    Ok(format!(
        "# DevEx 对比\n\n\
         - suite：`{}`\n\
         - cache：`{}`\n\
         - compile surface：`{}`\n\n\
         | 指标 | 基线 | 候选 | 变化 |\n\
         | --- | ---: | ---: | ---: |\n\
         | P50 | {:.1} ms | {:.1} ms | {} |\n\
         | P95 | {:.1} ms | {:.1} ms | {} |\n",
        baseline.suite.as_str(),
        baseline.cache_state.as_str(),
        baseline.compile_surface_fingerprint,
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
    }
    Ok(())
}

fn ensure_comparable(baseline: &RunSummary, candidate: &RunSummary) -> Result<()> {
    if baseline.suite != candidate.suite {
        return Err("DevEx 对比要求 suite 相同".into());
    }
    if baseline.cache_state != candidate.cache_state {
        return Err("DevEx 对比要求 cache state 相同".into());
    }
    if baseline.compile_surface_fingerprint != candidate.compile_surface_fingerprint {
        return Err("DevEx 对比的 compile_surface_fingerprint 不一致，拒绝生成误导结论".into());
    }
    Ok(())
}

fn summary_markdown(summary: &RunSummary) -> String {
    let metrics = summary.duration_ms.as_ref().map_or_else(
        || "无成功测量样本".to_owned(),
        |duration| {
            format!(
                "P50 {:.1} ms，P95 {:.1} ms，均值 {:.1} ms，范围 {:.1}–{:.1} ms",
                duration.p50, duration.p95, duration.mean, duration.min, duration.max
            )
        },
    );
    format!(
        "# DevEx 测量摘要\n\n\
         - run：`{}`\n\
         - suite：`{}`\n\
         - cache：`{}`\n\
         - compile surface：`{}`\n\
         - 样本：{}（通过 {}，失败 {}）\n\
         - 耗时：{}\n",
        summary.run_id,
        summary.suite.as_str(),
        summary.cache_state.as_str(),
        summary.compile_surface_fingerprint,
        summary.samples,
        summary.passed,
        summary.failed,
        metrics,
    )
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
