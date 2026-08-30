use crate::{Result, dev::ReadyKind};

use super::{RunSummary, SampleRecord};
use crate::devex::model::{BaselineContract, DevexSuite, PairingMetadata};

pub(super) fn validate(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_records: &[SampleRecord],
    candidate_records: &[SampleRecord],
    baseline_pairing: &PairingMetadata,
    candidate_pairing: &PairingMetadata,
) -> Result<()> {
    if baseline_pairing.baseline_contract != candidate_pairing.baseline_contract
        || baseline_pairing.baseline_provenance != candidate_pairing.baseline_provenance
    {
        return Err("DevEx paired baseline contract 或来源证据不一致".into());
    }
    match baseline_pairing.baseline_contract {
        None if baseline_pairing.baseline_provenance.is_none() => Ok(()),
        None => Err("DevEx paired 未声明 baseline contract 却携带了来源证据".into()),
        Some(BaselineContract::LegacyCargoDevV1) => validate_legacy_cargo_dev(
            baseline,
            candidate,
            baseline_records,
            candidate_records,
            baseline_pairing,
        ),
    }
}

fn validate_legacy_cargo_dev(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_records: &[SampleRecord],
    candidate_records: &[SampleRecord],
    pairing: &PairingMetadata,
) -> Result<()> {
    if baseline.suite != DevexSuite::CargoDevSave
        || candidate.suite != DevexSuite::CargoDevSave
        || baseline.variant != "config-only"
        || candidate.variant != "config-only"
    {
        return Err("legacy-cargo-dev-v1 只允许 cargo-dev-save/config-only".into());
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or("legacy-cargo-dev-v1 缺少 baseline 来源证据")?;
    if provenance.base_commit != BaselineContract::LEGACY_CARGO_DEV_BASE_COMMIT
        || provenance.adapter_commit != BaselineContract::LEGACY_CARGO_DEV_ADAPTER_COMMIT
        || !valid_sha256(&provenance.patch_sha256)
    {
        return Err("legacy-cargo-dev-v1 baseline 提交或补丁哈希无效".into());
    }
    validate_save_records("基线", baseline_records, 1, ReadyKind::Promoted)?;
    validate_save_records("候选", candidate_records, 0, ReadyKind::VerifiedNoRestart)
}

fn validate_save_records(
    label: &str,
    records: &[SampleRecord],
    cargo_invocations: usize,
    ready_kind: ReadyKind,
) -> Result<()> {
    if records.iter().any(|sample| {
        sample.cargo_invocations != Some(cargo_invocations) || sample.ready_kind != Some(ready_kind)
    }) {
        return Err(format!(
            "legacy-cargo-dev-v1 {label}样本必须为 Cargo={cargo_invocations} 且 ready={ready_kind:?}"
        )
        .into());
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
