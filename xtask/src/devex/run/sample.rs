use std::{fs, path::Path, process::ExitStatus, time::Instant};

use chrono::Utc;

use crate::{
    Result,
    dev::{ReadyKind, SaveMeasurementContract},
};

use super::{
    RunSession,
    process::{StepInvocation, execute_steps},
    resource_gate, save,
};
use crate::devex::{
    incremental::with_source_edit,
    memory::MemoryEvidence,
    metadata::{PathNormalizer, SourceFingerprints},
    model::{CacheState, DevexRunOptions, DevexSuite, PairedArm},
    report::{ResourceGateDecisionEvidence, SampleKind, SampleRecord, SampleStatus},
    runtime,
};

#[derive(Debug, Clone)]
pub(in crate::devex) struct SampleOutcome {
    pub(super) started_at: chrono::DateTime<Utc>,
    pub(super) duration_ms: f64,
    pub(super) memory: MemoryEvidence,
    pub(super) cargo_invocations: Option<usize>,
    pub(super) ready_kind: Option<ReadyKind>,
    pub(super) resource_gate_decision: Option<ResourceGateDecisionEvidence>,
    pub(super) resource_gate_accepted: bool,
    pub(super) runtime: Option<runtime::RuntimeEvidence>,
    pub(super) status: ExitStatus,
}

impl SampleOutcome {
    pub(in crate::devex) fn passed(&self) -> bool {
        self.status.success() && self.resource_gate_accepted
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::devex) fn execute_sample(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    target: &Path,
    session: &RunSession,
    options: &DevexRunOptions,
    label: &str,
    save_contract: SaveMeasurementContract,
) -> Result<SampleOutcome> {
    fs::create_dir_all(target)?;
    let suite = options.suite;
    let variant = &options.variant;
    let save_result = (suite == DevexSuite::CargoDevSave)
        .then(|| save::result_path(target, label, session.definition))
        .transpose()?;
    let strict_gate = session
        .pairing
        .as_ref()
        .and_then(|pairing| pairing.targeted_pair());
    if strict_gate.is_some() {
        resource_gate::check_new_decision(target, label)?;
    }
    let started_at = Utc::now();
    let started = Instant::now();
    let (status, memory) = run_sample_commands(
        backend_root,
        frontend_root,
        runner_frontend_root,
        target,
        session,
        options,
        label,
    )?;
    let (resource_gate_decision, resource_gate_accepted) = if suite == DevexSuite::ResourceGate {
        resource_gate::sample_decision(
            target,
            label,
            status.success(),
            strict_gate.map(|pair| pair.steps.as_slice()),
        )?
    } else {
        (None, true)
    };
    let outcome = SampleOutcome {
        started_at,
        duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
        memory,
        cargo_invocations: None,
        ready_kind: None,
        resource_gate_decision,
        resource_gate_accepted,
        runtime: if suite.is_runtime() {
            runtime::read(target, label, suite, status.success())?
        } else {
            None
        },
        status,
    };
    if let Some(result) = save_result.filter(|_| outcome.passed()) {
        save::measurement_outcome(
            &result,
            variant,
            outcome.status,
            save_contract,
            outcome.memory,
        )
    } else {
        Ok(outcome)
    }
}

fn run_sample_commands(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    target: &Path,
    session: &RunSession,
    options: &DevexRunOptions,
    label: &str,
) -> Result<(ExitStatus, MemoryEvidence)> {
    let source = options
        .suite
        .incremental_source(&options.variant)
        .map_err(|error| format!("DevEx 增量源码选择失败：{error}"))?
        .map(|relative| backend_root.join(relative));
    let execute = || {
        execute_steps(
            backend_root,
            frontend_root,
            runner_frontend_root,
            target,
            session.definition,
            &session.environment,
            StepInvocation {
                variant: &options.variant,
                label,
                runtime_input: session.runtime_input.as_ref(),
                pairing: session.pairing.as_ref(),
            },
        )
    };
    match source {
        Some(source) => with_source_edit(&source, label, execute),
        None => execute(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::devex) fn sample_record(
    run_id: &str,
    sequence: usize,
    kind: SampleKind,
    cache_state: CacheState,
    target: &Path,
    outcome: &SampleOutcome,
    normalizer: &PathNormalizer,
    audit: Option<SampleAudit>,
) -> SampleRecord {
    let (arm, pair, order, source_fingerprints) = audit.map_or((None, None, None, None), |audit| {
        (
            Some(audit.arm),
            audit.pair,
            audit.order,
            Some(audit.source_fingerprints),
        )
    });
    SampleRecord {
        schema_version: 1,
        run_id: run_id.to_owned(),
        sequence,
        kind,
        cache_state,
        started_at: outcome.started_at.to_rfc3339(),
        duration_ms: outcome.duration_ms,
        memory: outcome.memory.clone(),
        cargo_invocations: outcome.cargo_invocations,
        ready_kind: outcome.ready_kind,
        status: if outcome.passed() {
            SampleStatus::Passed
        } else {
            SampleStatus::Failed
        },
        exit_code: outcome.status.code(),
        target_directory: normalizer.normalize(&target.to_string_lossy()),
        arm,
        pair,
        order,
        source_fingerprints,
        resource_gate_decision: outcome.resource_gate_decision.clone(),
        runtime: outcome.runtime.clone(),
    }
}

pub(in crate::devex) struct SampleAudit {
    pub(in crate::devex) arm: PairedArm,
    pub(in crate::devex) pair: Option<usize>,
    pub(in crate::devex) order: Option<usize>,
    pub(in crate::devex) source_fingerprints: SourceFingerprints,
}
