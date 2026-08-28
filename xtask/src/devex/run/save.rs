use std::{path::Path, process::ExitStatus};

use chrono::Utc;

use crate::{
    Result,
    dev::{RESULT_FILE_NAME, SaveCase, SaveMeasurementContract, read_measurement_with_contract},
};

use super::SampleOutcome;

pub(super) fn measurement_outcome(
    target: &Path,
    variant: &str,
    status: ExitStatus,
    contract: SaveMeasurementContract,
) -> Result<SampleOutcome> {
    let measurement = read_measurement_with_contract(&target.join(RESULT_FILE_NAME), contract)?;
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
        cargo_invocations: Some(measurement.cargo_invocations),
        ready_kind: Some(measurement.ready_kind),
        status,
    })
}
