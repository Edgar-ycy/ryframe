use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

use super::report::{Distribution, SampleRecord, SampleStatus, distribution};

#[path = "memory/collect.rs"]
mod collect;
#[cfg(target_os = "linux")]
#[path = "memory/linux.rs"]
pub(crate) mod linux;

pub(crate) use collect::execute;

#[cfg(target_os = "linux")]
pub(crate) fn run_trampoline_if_requested() -> Option<Result<()>> {
    linux::run_trampoline_if_requested()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MemoryMethod {
    WindowsJobCommit,
    LinuxCgroupCharge,
    Unsupported,
}

impl MemoryMethod {
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::WindowsJobCommit
        } else if cfg!(target_os = "linux") {
            Self::LinuxCgroupCharge
        } else {
            Self::Unsupported
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::WindowsJobCommit => "Windows Job 峰值提交内存",
            Self::LinuxCgroupCharge => "Linux cgroup v2 峰值计费内存（含文件缓存）",
            Self::Unsupported => "当前平台没有进程树内存采集器",
        }
    }

    const fn collector_contract(self) -> (&'static str, u16, &'static str) {
        match self {
            Self::WindowsJobCommit => (
                "processkit-job-accounting",
                1,
                "windows-job-object:peak-job-memory-used:complete-owned-command-tree:v1",
            ),
            Self::LinuxCgroupCharge => (
                "cgroup-v2-self-exec",
                1,
                "linux-cgroup-v2:memory-peak:join-and-readback-before-exec:complete-inherited-command-tree:v1",
            ),
            Self::Unsupported => (
                "unsupported-platform",
                1,
                "unsupported-platform:no-complete-command-tree-memory-collector:v1",
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemoryCollectorIdentity {
    implementation: String,
    version: u16,
    fingerprint: String,
}

impl MemoryCollectorIdentity {
    fn for_method(method: MemoryMethod) -> Self {
        let (implementation, version, contract) = method.collector_contract();
        let digest = Sha256::digest(contract.as_bytes());
        Self {
            implementation: implementation.to_owned(),
            version,
            fingerprint: format!(
                "sha256:{}",
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ),
        }
    }

    fn validate(&self, method: MemoryMethod) -> Result<()> {
        if self == &Self::for_method(method) {
            Ok(())
        } else {
            Err("内存采集器实现、版本或语义指纹不匹配".into())
        }
    }

    fn label(&self) -> String {
        format!(
            "{} v{}（{}）",
            self.implementation, self.version, self.fingerprint
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MemoryScope {
    CommandTree,
}

impl MemoryScope {
    pub(crate) const fn description(self) -> &'static str {
        "每步完整命令及其新建后代；多步取最大值；不含已有 sccache、API、Worker 或浏览器进程；保存测量包含整个测试命令"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum MemoryReading {
    Measured { peak_bytes: u64 },
    Unavailable { reason: String },
}

impl MemoryReading {
    pub(crate) fn from_result(result: Result<u64>) -> Self {
        match result {
            Ok(peak_bytes) if peak_bytes > 0 => Self::Measured { peak_bytes },
            Ok(_) => Self::unavailable("内核未提供非零峰值，不能作为有效内存样本"),
            Err(error) => Self::unavailable(error.to_string()),
        }
    }

    pub(crate) fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemoryEvidence {
    pub(crate) method: MemoryMethod,
    pub(crate) scope: MemoryScope,
    pub(crate) collector: MemoryCollectorIdentity,
    pub(crate) steps: Vec<MemoryReading>,
}

impl MemoryEvidence {
    pub(crate) fn new() -> Self {
        Self::for_method(MemoryMethod::current())
    }

    pub(crate) fn for_method(method: MemoryMethod) -> Self {
        Self {
            method,
            scope: MemoryScope::CommandTree,
            collector: MemoryCollectorIdentity::for_method(method),
            steps: Vec::new(),
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.collector.validate(self.method)?;
        if self.steps.is_empty() {
            return Err("内存证据缺少命令步骤".into());
        }
        for step in &self.steps {
            match step {
                MemoryReading::Measured { peak_bytes }
                    if *peak_bytes == 0 || self.method == MemoryMethod::Unsupported =>
                {
                    return Err("内存证据包含零值或不支持的采集方法".into());
                }
                MemoryReading::Unavailable { reason } if reason.trim().is_empty() => {
                    return Err("内存不可用证据缺少原因".into());
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn peak(&self) -> Option<u64> {
        self.steps
            .iter()
            .map(|step| match step {
                MemoryReading::Measured { peak_bytes } => Some(*peak_bytes),
                MemoryReading::Unavailable { .. } => None,
            })
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .max()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct MemorySummary {
    pub(crate) method: MemoryMethod,
    pub(crate) scope: MemoryScope,
    pub(crate) collector: MemoryCollectorIdentity,
    pub(crate) successful_peak_bytes: Option<Distribution>,
    pub(crate) failed_peak_bytes: Option<Distribution>,
    pub(crate) unavailable_samples: usize,
    pub(crate) unavailable_reasons: BTreeSet<String>,
}

pub(super) fn summarize(records: &[&SampleRecord]) -> Result<Option<MemorySummary>> {
    let Some(first) = records.first() else {
        return Ok(None);
    };
    let mut successful = Vec::new();
    let mut failed = Vec::new();
    let mut unavailable_samples = 0;
    let mut unavailable_reasons = BTreeSet::new();
    for record in records {
        let evidence = &record.memory;
        evidence.validate()?;
        if evidence.method != first.memory.method
            || evidence.scope != first.memory.scope
            || evidence.collector != first.memory.collector
        {
            return Err("同次测量的内存采集器或范围发生变化".into());
        }
        match evidence.peak() {
            Some(peak) if record.status == SampleStatus::Passed => successful.push(peak as f64),
            Some(peak) => failed.push(peak as f64),
            None => unavailable_samples += 1,
        }
        for step in &evidence.steps {
            if let MemoryReading::Unavailable { reason } = step {
                unavailable_reasons.insert(reason.clone());
            }
        }
    }
    Ok(Some(MemorySummary {
        method: first.memory.method,
        scope: first.memory.scope,
        collector: first.memory.collector.clone(),
        successful_peak_bytes: distribution(&successful),
        failed_peak_bytes: distribution(&failed),
        unavailable_samples,
        unavailable_reasons,
    }))
}

pub(crate) fn ensure_comparable(
    baseline: Option<&MemorySummary>,
    candidate: Option<&MemorySummary>,
) -> Result<()> {
    let baseline = baseline.ok_or("基线缺少内存统计")?;
    let candidate = candidate.ok_or("候选缺少内存统计")?;
    if baseline.method != candidate.method
        || baseline.scope != candidate.scope
        || baseline.collector != candidate.collector
    {
        return Err("进程树内存采集器或范围不同，拒绝混合口径比较".into());
    }
    if baseline.unavailable_samples > 0
        || candidate.unavailable_samples > 0
        || baseline.successful_peak_bytes.is_none()
        || candidate.successful_peak_bytes.is_none()
    {
        return Err("进程树内存存在不可用样本，不能宣称性能验收完整通过；详见 summary.md".into());
    }
    Ok(())
}

pub(super) fn render(summary: Option<&MemorySummary>) -> String {
    let Some(summary) = summary else {
        return "\n- 进程树内存：没有测量样本\n".into();
    };
    let values = |value: Option<&Distribution>| {
        value.map_or_else(
            || "不可用".to_owned(),
            |value| {
                format!(
                    "峰值 {:.0} bytes，样本 P50 {:.0} bytes，样本 P95 {:.0} bytes",
                    value.max, value.p50, value.p95
                )
            },
        )
    };
    let reasons = summary
        .unavailable_reasons
        .iter()
        .map(|reason| format!("\n  - {}", reason.replace(['\r', '\n'], " ")))
        .collect::<String>();
    format!(
        "\n## 命令进程树内存\n\n- 方法：{}\n- 采集器：{}\n- 范围：{}\n- 成功样本：{}\n- 失败样本：{}\n- 不可用样本：{}{}\n",
        summary.method.description(),
        summary.collector.label(),
        summary.scope.description(),
        values(summary.successful_peak_bytes.as_ref()),
        values(summary.failed_peak_bytes.as_ref()),
        summary.unavailable_samples,
        reasons,
    )
}

pub(super) fn render_comparison(
    baseline: Option<&MemorySummary>,
    candidate: Option<&MemorySummary>,
) -> String {
    let (Some(baseline), Some(candidate)) = (baseline, candidate) else {
        return String::new();
    };
    let (Some(before), Some(after)) = (
        &baseline.successful_peak_bytes,
        &candidate.successful_peak_bytes,
    ) else {
        return String::new();
    };
    format!(
        "\n- 内存方法：{}\n- 内存采集器：{}\n- 内存范围：{}\n- 成功样本最大峰值：{:.0} → {:.0} bytes（{:+.1}%）；不额外设置内存通过阈值。\n",
        baseline.method.description(),
        baseline.collector.label(),
        baseline.scope.description(),
        before.max,
        after.max,
        (after.max / before.max - 1.0) * 100.0,
    )
}
