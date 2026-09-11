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
        Some(BaselineContract::LegacyCargoDevV2) => validate_legacy_api_worker(
            baseline,
            candidate,
            baseline_records,
            candidate_records,
            baseline_pairing,
        ),
        Some(BaselineContract::LegacyStableReadinessB0V1) => {
            validate_stable_readiness_b0(baseline, candidate, baseline_pairing)
        }
    }
}

fn validate_stable_readiness_b0(
    baseline: &RunSummary,
    candidate: &RunSummary,
    pairing: &PairingMetadata,
) -> Result<()> {
    if baseline.suite != candidate.suite
        || baseline.variant != candidate.variant
        || !matches!(
            baseline.suite,
            DevexSuite::ResourceGenerator | DevexSuite::ResourceGate | DevexSuite::RustGate
        )
    {
        return Err(
            "legacy-stable-readiness-b0-v1 只允许同一 resource-generator、resource-gate 或 rust-gate 任务"
                .into(),
        );
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or("legacy-stable-readiness-b0-v1 缺少 baseline 来源证据")?;
    if provenance.base_commit != BaselineContract::STABLE_READINESS_B0_BASE_COMMIT
        || !valid_git_commit(&provenance.adapter_commit)
        || provenance.adapter_commit == provenance.base_commit
        || provenance.patch_sha256 != BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
        || provenance.frontend_commit.as_deref()
            != Some(BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT)
        || provenance.adapter_paths != BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS
    {
        return Err("legacy-stable-readiness-b0-v1 来源或工具层适配证据无效".into());
    }
    Ok(())
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

fn validate_legacy_api_worker(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_records: &[SampleRecord],
    candidate_records: &[SampleRecord],
    pairing: &PairingMetadata,
) -> Result<()> {
    if baseline.suite != DevexSuite::CargoDevSave
        || candidate.suite != DevexSuite::CargoDevSave
        || baseline.variant != candidate.variant
        || !matches!(baseline.variant.as_str(), "api-only" | "worker-only")
    {
        return Err("legacy-cargo-dev-v2 只允许 cargo-dev-save/api-only 或 worker-only".into());
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or("legacy-cargo-dev-v2 缺少 baseline 来源证据")?;
    if provenance.base_commit != BaselineContract::LEGACY_CARGO_DEV_V2_BASE_COMMIT
        || provenance.adapter_commit != BaselineContract::LEGACY_CARGO_DEV_V2_ADAPTER_COMMIT
        || !valid_sha256(&provenance.patch_sha256)
    {
        return Err("legacy-cargo-dev-v2 baseline 提交或补丁哈希无效".into());
    }
    validate_save_records("基线", baseline_records, 1, ReadyKind::Promoted)?;
    validate_save_records("候选", candidate_records, 1, ReadyKind::Promoted)
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

fn valid_git_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
