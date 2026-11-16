use sha2::{Digest, Sha256};

use crate::{Result, dev::ReadyKind};

use super::{RunSummary, SampleRecord};
use crate::devex::metadata::RunMetadata;
use crate::devex::model::{
    BaselineContract, BaselineProvenance, DevexSuite, PairedArm, PairingMetadata,
};

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
        Some(
            contract @ (BaselineContract::LegacyStableReadinessB0V1
            | BaselineContract::StableReadinessB0CommonToolchainV1),
        ) => validate_stable_readiness_b0(baseline, candidate, baseline_pairing, contract),
    }
}

fn validate_stable_readiness_b0(
    baseline: &RunSummary,
    candidate: &RunSummary,
    pairing: &PairingMetadata,
    contract: BaselineContract,
) -> Result<()> {
    if baseline.suite != candidate.suite || baseline.variant != candidate.variant {
        return Err(format!("{} 只允许同一 suite 与变体", contract.as_str()).into());
    }
    if (baseline.suite.is_runtime()
        || (contract.uses_common_toolchain() && baseline.suite == DevexSuite::FrontendFast))
        && (baseline.source_fingerprints.runner_frontend.is_none()
            || baseline.source_fingerprints.runner_frontend
                != candidate.source_fingerprints.runner_frontend)
    {
        return Err("stable-readiness 两侧必须使用同一 runner 前端工具闭包".into());
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or_else(|| format!("{} 缺少 baseline 来源证据", contract.as_str()))?;
    if !valid_stable_readiness_provenance(provenance, contract) {
        return Err(format!("{} 来源或执行适配证据无效", contract.as_str()).into());
    }
    Ok(())
}

pub(super) fn validate_source_binding(metadata: &RunMetadata) -> Result<()> {
    let Some(pairing) = metadata.pairing.as_ref() else {
        return Ok(());
    };
    let Some(contract) = pairing
        .baseline_contract
        .filter(|value| value.is_stable_readiness_b0())
    else {
        return Ok(());
    };
    if contract.uses_common_toolchain() && metadata.suite.is_runtime() {
        return Err("DevEx B0 同工具链合同只接受非运行时测量".into());
    }
    validate_workload_binding(metadata, pairing, contract)?;
    if pairing.arm != PairedArm::Baseline {
        return Ok(());
    }
    let provenance = pairing
        .baseline_provenance
        .as_ref()
        .ok_or_else(|| format!("{} 缺少 baseline 来源证据", contract.as_str()))?;
    if !valid_stable_readiness_provenance(provenance, contract) {
        return Err(format!("{} 来源或执行适配证据无效", contract.as_str()).into());
    }
    if metadata.backend.commit.as_deref() != Some(provenance.adapter_commit.as_str())
        || metadata.backend.dirty != Some(false)
        || metadata.backend.worktree_fingerprint
            != clean_worktree_fingerprint(&provenance.adapter_commit)
    {
        return Err(format!("{} 报告与实际 B0 源码身份不一致", contract.as_str()).into());
    }
    let execution = provenance.frontend_adaptation.as_ref().map_or(
        BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT,
        |value| value.commit.as_str(),
    );
    let valid_frontend = match metadata.frontend.as_ref() {
        Some(frontend) => {
            frontend.commit.as_deref() == Some(execution)
                && frontend.dirty == Some(false)
                && frontend.worktree_fingerprint == clean_worktree_fingerprint(execution)
        }
        None => {
            !contract.uses_common_toolchain()
                || !metadata
                    .suite
                    .definition(&metadata.variant)?
                    .requires_frontend
        }
    };
    if !valid_frontend {
        return Err(format!("{} 报告与实际前端 B0 身份不一致", contract.as_str()).into());
    }
    Ok(())
}

fn validate_workload_binding(
    metadata: &RunMetadata,
    pairing: &PairingMetadata,
    contract: BaselineContract,
) -> Result<()> {
    let expected = contract.workload_contract(metadata.suite, &metadata.variant);
    if pairing.workload_contract != expected {
        return Err(format!("{} 实际任务集合合同不匹配", contract.as_str()).into());
    }
    if contract.uses_common_toolchain() && metadata.suite == DevexSuite::CargoDevSave {
        let definition =
            metadata
                .suite
                .paired_definition(&metadata.variant, Some(contract), pairing.arm)?;
        if expected.is_none()
            || metadata.commands
                != crate::devex::metadata::command_metadata(definition.steps, &metadata.variant)?
        {
            return Err("共同 DevEx 保存测量的实际执行入口不匹配".into());
        }
    }
    if metadata.suite != DevexSuite::FrontendFast {
        return Ok(());
    }
    if contract.uses_common_toolchain() {
        let commit = BaselineContract::COMMON_FRONTEND_RUNNER_COMMIT;
        let fingerprint = clean_worktree_fingerprint(commit);
        if !metadata.runner_frontend.as_ref().is_some_and(|runner| {
            runner.commit.as_deref() == Some(commit)
                && runner.dirty == Some(false)
                && runner.worktree_fingerprint == fingerprint
        }) {
            return Err("共同 DevEx 前端快速检查的工具来源不匹配".into());
        }
    }
    let arm = match pairing.arm {
        PairedArm::Baseline => "b0",
        PairedArm::Candidate => "b1",
    };
    let common_arguments = [
        "$RUNNER_FRONTEND/scripts/devex-fast.mjs",
        "--root",
        "$FRONTEND",
        "--arm",
        arm,
        "--cache-root",
        "$FRONTEND_CACHE",
    ];
    let (program, arguments): (&str, &[&str]) =
        match (contract.uses_common_toolchain(), pairing.arm) {
            (true, _) => ("node", &common_arguments),
            (false, PairedArm::Baseline) => ("corepack", &["pnpm", "check:fast"]),
            (false, PairedArm::Candidate) => ("corepack", &["pnpm", "check"]),
        };
    let valid = if let [command] = metadata.commands.as_slice() {
        command.program == program
            && (!contract.uses_common_toolchain() || command.working_directory == "$FRONTEND")
            && command
                .args
                .iter()
                .map(String::as_str)
                .eq(arguments.iter().copied())
    } else {
        false
    };
    if !valid {
        return Err(format!("{} frontend-fast 实际执行入口不匹配", contract.as_str()).into());
    }
    Ok(())
}

fn valid_stable_readiness_provenance(
    provenance: &BaselineProvenance,
    contract: BaselineContract,
) -> bool {
    provenance.base_commit == BaselineContract::STABLE_READINESS_B0_BASE_COMMIT
        && provenance.adapter_commit == BaselineContract::STABLE_READINESS_B0_ADAPTER_COMMIT
        && provenance.patch_sha256 == BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
        && provenance.adapter_tree.as_deref()
            == Some(BaselineContract::STABLE_READINESS_B0_ADAPTER_TREE)
        && provenance.frontend_commit.as_deref()
            == Some(BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT)
        && provenance.frontend_adaptation == contract.registered_frontend_adaptation()
        && provenance
            .adapter_paths
            .iter()
            .map(String::as_str)
            .eq(BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS)
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
