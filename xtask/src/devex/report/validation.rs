use std::collections::BTreeSet;

use crate::Result;

use super::{
    super::{
        metadata::RunMetadata,
        model::{DevexSuite, RESOURCE_GATE_TARGETED_ACTIVATION, RESOURCE_GATE_TARGETED_ENV},
    },
    SampleKind, SampleRecord, SampleStatus,
};

pub(super) fn validate_execution_contract(metadata: &RunMetadata) -> Result<()> {
    metadata
        .suite
        .validate_cache_state(metadata.cache_state)
        .map_err(|error| format!("DevEx metadata cache contract 无效：{error}"))?;
    if metadata.suite == DevexSuite::ResourceGate
        && metadata
            .environment
            .get(RESOURCE_GATE_TARGETED_ENV)
            .map(String::as_str)
            != Some(RESOURCE_GATE_TARGETED_ACTIVATION)
    {
        return Err("resource-gate metadata 缺少 replay-verified targeted 激活值".into());
    }
    Ok(())
}

pub(super) fn validate_samples(metadata: &RunMetadata, samples: &[SampleRecord]) -> Result<()> {
    let mut measurement_sequences = BTreeSet::new();
    for sample in samples {
        validate_sample(metadata, sample)?;
        if sample.kind == SampleKind::Measurement
            && (!measurement_sequences.insert(sample.sequence)
                || !(1..=metadata.requested_runs).contains(&sample.sequence))
        {
            return Err(format!("测量样本序号无效或重复：{}", sample.sequence).into());
        }
    }
    Ok(())
}

fn validate_sample(metadata: &RunMetadata, sample: &SampleRecord) -> Result<()> {
    if sample.schema_version != 1 {
        return Err(format!("不支持的 DevEx sample schema：{}", sample.schema_version).into());
    }
    if sample.run_id != metadata.run_id || sample.cache_state != metadata.cache_state {
        return Err(format!("样本 {} 与 metadata 不属于同一次运行", sample.sequence).into());
    }
    if !sample.duration_ms.is_finite() || sample.duration_ms < 0.0 {
        return Err(format!("样本 {} 的耗时无效", sample.sequence).into());
    }
    sample.memory.validate()?;
    let expected_steps = metadata.suite.definition(&metadata.variant)?.steps.len();
    if sample.memory.steps.len() > expected_steps
        || (sample.status == SampleStatus::Passed && sample.memory.steps.len() != expected_steps)
    {
        return Err("内存证据步骤数与实际测量命令不一致".into());
    }
    if metadata.suite == DevexSuite::CargoDevSave && sample.cargo_invocations.is_none() {
        return Err(format!("cargo-dev-save 样本 {} 缺少 Cargo 调用数", sample.sequence).into());
    }
    if metadata.suite == DevexSuite::CargoDevSave && sample.ready_kind.is_none() {
        return Err(format!("cargo-dev-save 样本 {} 缺少就绪结果", sample.sequence).into());
    }
    validate_decision(metadata.suite, sample)
}

fn validate_decision(suite: DevexSuite, sample: &SampleRecord) -> Result<()> {
    match (suite, sample.status, sample.resource_gate_decision.as_ref()) {
        (DevexSuite::ResourceGate, SampleStatus::Passed, Some(decision))
            if decision.is_targeted() =>
        {
            Ok(())
        }
        (DevexSuite::ResourceGate, SampleStatus::Passed, _) => Err(format!(
            "resource-gate 样本 {} 缺少有效 targeted decision 证据",
            sample.sequence
        )
        .into()),
        (DevexSuite::ResourceGate, SampleStatus::Failed, _) | (_, _, None) => Ok(()),
        (_, _, Some(_)) => Err(format!(
            "非 resource-gate 样本 {} 携带了 decision 证据",
            sample.sequence
        )
        .into()),
    }
}
