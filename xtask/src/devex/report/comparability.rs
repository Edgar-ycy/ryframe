use crate::Result;

use super::{RunSummary, SampleRecord, ensure_complete, validate_paired_records};
use crate::devex::memory;

pub(super) fn ensure_comparable(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_records: &[SampleRecord],
    candidate_records: &[SampleRecord],
) -> Result<()> {
    ensure_complete("基线", baseline)?;
    ensure_complete("候选", candidate)?;
    memory::ensure_comparable(baseline.memory.as_ref(), candidate.memory.as_ref())?;
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
    ensure_compile_surface(
        &baseline.compile_surface_fingerprint,
        &candidate.compile_surface_fingerprint,
    )?;
    if baseline.pairing.as_ref().is_some_and(|pairing| {
        pairing
            .baseline_contract
            .is_some_and(|contract| contract.uses_common_toolchain())
    }) && (baseline.common_toolchain.is_none()
        || baseline.common_toolchain != candidate.common_toolchain)
    {
        return Err("共同 DevEx 比较缺少两侧一致的工具链证据".into());
    }
    if baseline.samples != candidate.samples {
        return Err("DevEx 对比要求两侧具有相同样本数".into());
    }
    validate_paired_records(baseline, candidate, baseline_records, candidate_records)
}

pub(in crate::devex) fn ensure_compile_surface(baseline: &str, candidate: &str) -> Result<()> {
    if baseline != candidate {
        return Err("DevEx 对比的 compile_surface_fingerprint 不一致，拒绝生成误导结论".into());
    }
    Ok(())
}
