use std::{
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use chrono::Utc;

use crate::{
    Result,
    dev::{RESULT_FILE_NAME, SaveCase, SaveMeasurementContract, read_measurement_with_contract},
};

use super::{
    super::{
        memory::MemoryEvidence,
        model::{PAIRED_SAVE_OUTPUT, SuiteDefinition, uses_historical_save_protocol},
    },
    SampleOutcome, StepInvocation,
};

pub(super) fn bind_historical_protocol(
    command: &mut Command,
    target: &Path,
    invocation: &StepInvocation<'_>,
) -> Result<()> {
    SaveCase::parse(invocation.variant).ok_or("B0 保存测量场景无效")?;
    command
        .env_remove("RYFRAME_WORKSPACE_ROOT")
        .env("RYFRAME_DEVEX_SAVE_CASE", invocation.variant)
        .env(
            "RYFRAME_DEVEX_SAVE_RESULT_PATH",
            paired_result_path(target, invocation.label),
        );
    Ok(())
}

pub(super) fn result_path(
    target: &Path,
    label: &str,
    definition: SuiteDefinition,
) -> Result<PathBuf> {
    let paired = definition
        .steps
        .iter()
        .any(|step| uses_historical_save_protocol(step) || step.args.contains(&PAIRED_SAVE_OUTPUT));
    if !paired {
        return Ok(target.join(RESULT_FILE_NAME));
    }
    let result = paired_result_path(target, label);
    if result.try_exists()? {
        return Err("DevEx 保存样本已存在对应的原始结果文件".into());
    }
    Ok(result)
}

pub(super) fn paired_result_path(target: &Path, label: &str) -> PathBuf {
    target.join(
        PAIRED_SAVE_OUTPUT
            .trim_start_matches("{target}/")
            .replace("{label}", label),
    )
}

pub(super) fn measurement_outcome(
    result: &Path,
    variant: &str,
    status: ExitStatus,
    contract: SaveMeasurementContract,
    memory: MemoryEvidence,
) -> Result<SampleOutcome> {
    let measurement = read_measurement_with_contract(result, contract)?;
    let expected = SaveCase::parse(variant).ok_or("cargo-dev-save 变体无法映射到保存场景")?;
    if measurement.case != expected {
        return Err(format!(
            "cargo-dev-save 结果场景不匹配：期望 {variant}，实际 {:?}",
            measurement.case
        )
        .into());
    }
    let started_at = chrono::DateTime::parse_from_rfc3339(&measurement.started_at)
        .map_err(|error| format!("cargo-dev-save startedAt 无效：{error}"))?
        .with_timezone(&Utc);
    Ok(SampleOutcome {
        started_at,
        duration_ms: measurement.save_to_ready_ms,
        memory,
        cargo_invocations: Some(measurement.cargo_invocations),
        ready_kind: Some(measurement.ready_kind),
        resource_gate_decision: None,
        resource_gate_accepted: true,
        runtime: None,
        status,
    })
}
