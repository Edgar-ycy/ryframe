use sha2::{Digest, Sha256};

use crate::{Result, dev::ReadyKind};

use super::{RunSummary, SampleRecord};
use crate::devex::metadata::RunMetadata;
use crate::devex::model::{BaselineContract, BaselineProvenance, DevexSuite, PairingMetadata};

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
    if baseline.suite != candidate.suite || baseline.variant != candidate.variant {
        return Err("legacy-stable-readiness-b0-v1 只允许同一 suite 与变体".into());
    }
    if baseline.suite.is_runtime()
        && (baseline.source_fingerprints.runner_frontend.is_none()
            || baseline.source_fingerprints.runner_frontend
                != candidate.source_fingerprints.runner_frontend)
    {
        return Err("stable-readiness 运行时两侧必须使用同一 runner 前端工具闭包".into());
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or("legacy-stable-readiness-b0-v1 缺少 baseline 来源证据")?;
    if !valid_stable_readiness_provenance(provenance) {
        return Err("legacy-stable-readiness-b0-v1 来源或工具层适配证据无效".into());
    }
    Ok(())
}

pub(super) fn validate_source_binding(metadata: &RunMetadata) -> Result<()> {
    let Some(pairing) = metadata.pairing.as_ref() else {
        return Ok(());
    };
    if pairing.baseline_contract != Some(BaselineContract::LegacyStableReadinessB0V1) {
        return Ok(());
    }
    validate_workload_binding(metadata, pairing)?;
    if pairing.arm != crate::devex::model::PairedArm::Baseline {
        return Ok(());
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or("legacy-stable-readiness-b0-v1 缺少 baseline 来源证据")?;
    if !valid_stable_readiness_provenance(provenance) {
        return Err("legacy-stable-readiness-b0-v1 来源或工具层适配证据无效".into());
    }
    if metadata.backend.commit.as_deref() != Some(provenance.adapter_commit.as_str())
        || metadata.backend.dirty != Some(false)
        || metadata.backend.worktree_fingerprint
            != clean_worktree_fingerprint(&provenance.adapter_commit)
    {
        return Err("legacy-stable-readiness-b0-v1 报告与实际 B0 源码身份不一致".into());
    }
    if let Some(frontend) = metadata.frontend.as_ref()
        && (frontend.commit.as_deref() != provenance.frontend_commit.as_deref()
            || frontend.dirty != Some(false)
            || frontend.worktree_fingerprint
                != clean_worktree_fingerprint(
                    BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT,
                ))
    {
        return Err("legacy-stable-readiness-b0-v1 报告与实际前端 B0 身份不一致".into());
    }
    Ok(())
}

fn validate_workload_binding(metadata: &RunMetadata, pairing: &PairingMetadata) -> Result<()> {
    let expected = BaselineContract::LegacyStableReadinessB0V1
        .workload_contract(metadata.suite, &metadata.variant);
    if pairing.workload_contract != expected {
        return Err("legacy-stable-readiness-b0-v1 实际任务集合合同不匹配".into());
    }
    if metadata.suite != DevexSuite::FrontendFast {
        return Ok(());
    }
    let expected_args: &[&str] = match pairing.arm {
        crate::devex::model::PairedArm::Baseline => &["pnpm", "check:fast"],
        crate::devex::model::PairedArm::Candidate => &["pnpm", "check"],
    };
    if metadata.commands.len() != 1
        || metadata.commands[0].program != "corepack"
        || metadata.commands[0]
            .args
            .iter()
            .map(String::as_str)
            .ne(expected_args.iter().copied())
    {
        return Err("legacy-stable-readiness-b0-v1 frontend-fast 实际执行入口不匹配".into());
    }
    Ok(())
}

fn valid_stable_readiness_provenance(provenance: &BaselineProvenance) -> bool {
    provenance.base_commit == BaselineContract::STABLE_READINESS_B0_BASE_COMMIT
        && valid_git_commit(&provenance.adapter_commit)
        && provenance.adapter_commit != provenance.base_commit
        && provenance.patch_sha256 == BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
        && provenance.adapter_tree.as_deref()
            == Some(BaselineContract::STABLE_READINESS_B0_ADAPTER_TREE)
        && provenance.frontend_commit.as_deref()
            == Some(BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT)
        && provenance.adapter_paths == BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS
}

fn clean_worktree_fingerprint(commit: &str) -> String {
    let mut digest = Sha256::new();
    update_digest(&mut digest, commit.as_bytes());
    update_digest(&mut digest, &[]);
    let hex = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}

fn update_digest(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
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
