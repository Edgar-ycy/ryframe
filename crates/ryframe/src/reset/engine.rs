use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;

use crate::reset::{
    ResetError, ResetResult,
    ledger::{LedgerStore, PhaseRecord, PhaseStatus, ResetLedger, ResetPhase, ResourceRecord},
    model::ResetManifest,
};

pub type PhaseEvidence = BTreeMap<String, String>;

#[async_trait]
pub trait ResetPhases: Send {
    /// 完成全部只读安全检查并持有所有 MySQL 环境锁。
    async fn preflight(
        &mut self,
        manifest: &ResetManifest,
        ledger: &ResetLedger,
    ) -> ResetResult<PhaseEvidence>;
    async fn purge_object_storage(
        &mut self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence>;
    async fn purge_redis(
        &mut self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence>;
    async fn recreate_databases(
        &mut self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence>;
    async fn migrate_control(&mut self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence>;
    async fn migrate_tenants(&mut self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence>;
    async fn verify(&mut self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence>;
    /// 释放预检阶段持有的锁。实现必须允许在部分预检失败后调用。
    async fn release(&mut self) -> ResetResult<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetStatus {
    Completed,
    Reused,
    Failed,
}

impl ResetStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Reused => "reused",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ResetReport {
    pub report_version: u32,
    pub plan_hash: String,
    pub environment: String,
    pub scope_id: String,
    pub code_sha: String,
    pub config_sha: String,
    pub credential_version: String,
    pub status: ResetStatus,
    pub completed_at: Option<String>,
    pub reported_at: String,
    pub failed_phase: Option<&'static str>,
    pub phases: BTreeMap<ResetPhase, PhaseRecord>,
    pub resources: BTreeMap<String, ResourceRecord>,
    pub object_prefixes: Vec<String>,
    pub redis_namespace: Option<String>,
    pub databases: Vec<String>,
}

impl ResetReport {
    fn from_ledger(
        manifest: &ResetManifest,
        plan_hash: &str,
        ledger: &ResetLedger,
        status: ResetStatus,
        failed_phase: Option<ResetPhase>,
    ) -> Self {
        Self {
            report_version: 2,
            plan_hash: plan_hash.to_owned(),
            environment: manifest.environment.clone(),
            scope_id: manifest.scope_id.clone(),
            code_sha: manifest.code_sha.clone(),
            config_sha: manifest.config_sha.clone(),
            credential_version: manifest.credential_version.clone(),
            status,
            completed_at: (status != ResetStatus::Failed)
                .then(|| ledger.phase(ResetPhase::Release).completed_at.clone())
                .flatten(),
            reported_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            failed_phase: failed_phase.map(ResetPhase::as_str),
            phases: ledger.phases.clone(),
            resources: ledger.resources.clone(),
            object_prefixes: manifest
                .object_storage
                .prefixes
                .iter()
                .map(|item| format!("{}:{}", item.bucket, item.prefix))
                .collect(),
            redis_namespace: manifest.redis.as_ref().map(|redis| redis.namespace.clone()),
            databases: manifest
                .databases
                .iter()
                .map(|database| {
                    format!("{}:{}/{}", database.host, database.port, database.database)
                })
                .collect(),
        }
    }
}

pub async fn execute<R: ResetPhases>(
    runtime: &mut R,
    manifest: &ResetManifest,
    plan_hash: &str,
    store: &LedgerStore,
) -> ResetResult<ResetReport> {
    let mut ledger = store.load_or_create(manifest, plan_hash)?;
    if let Some(result) = reuse_completed(manifest, plan_hash, &ledger, store) {
        return result;
    }
    if ledger.rewind_interrupted_baselines(manifest)? {
        store.save(&ledger)?;
    }
    let phase_result = execute_phases(runtime, manifest, &mut ledger, store).await;
    let release_result = release_locks(runtime, &mut ledger, store).await;
    let failure = match (phase_result, release_result) {
        (Err((phase, error)), Err(release)) => Some((
            phase,
            ResetError::new(format!("{error}；锁释放阶段：{release}")),
        )),
        (Err(failure), Ok(())) => Some(failure),
        (Ok(()), Err(error)) => Some((ResetPhase::Release, error)),
        (Ok(()), Ok(())) => None,
    };
    let report = ResetReport::from_ledger(
        manifest,
        plan_hash,
        &ledger,
        if failure.is_some() {
            ResetStatus::Failed
        } else {
            ResetStatus::Completed
        },
        failure.as_ref().map(|(phase, _)| *phase),
    );
    store.write_report(&report)?;
    failure.map_or(Ok(report), |(_, error)| Err(error))
}

fn reuse_completed(
    manifest: &ResetManifest,
    plan_hash: &str,
    ledger: &ResetLedger,
    store: &LedgerStore,
) -> Option<ResetResult<ResetReport>> {
    let business_complete = ResetPhase::ORDERED
        .into_iter()
        .filter(|phase| *phase != ResetPhase::Release)
        .all(|phase| ledger.phase_complete(phase));
    if business_complete && ledger.phase_complete(ResetPhase::Release) {
        let report =
            ResetReport::from_ledger(manifest, plan_hash, ledger, ResetStatus::Reused, None);
        return Some(store.write_report(&report).map(|()| report));
    }
    if business_complete
        || matches!(
            ledger.phase(ResetPhase::Release).status,
            PhaseStatus::Running | PhaseStatus::Failed
        )
    {
        let failed = ResetPhase::ORDERED
            .into_iter()
            .find(|phase| ledger.phase(*phase).status == PhaseStatus::Failed)
            .unwrap_or(ResetPhase::Release);
        let report = ResetReport::from_ledger(
            manifest,
            plan_hash,
            ledger,
            ResetStatus::Failed,
            Some(failed),
        );
        return Some(store.write_report(&report).and_then(|()| Err(ResetError::new(
            "原 reset 锁释放失败或缺少完成证据，必须人工核对服务停止状态、MySQL 环境锁与精确资源；保留原账本，禁止删除账本或重复执行来伪造新完成",
        ))));
    }
    None
}

async fn execute_phases<R: ResetPhases>(
    runtime: &mut R,
    manifest: &ResetManifest,
    ledger: &mut ResetLedger,
    store: &LedgerStore,
) -> Result<(), (ResetPhase, ResetError)> {
    for phase in ResetPhase::ORDERED
        .into_iter()
        .take_while(|phase| *phase != ResetPhase::Release)
    {
        if phase != ResetPhase::Preflight && ledger.phase_complete(phase) {
            continue;
        }
        ledger.mark_running(phase);
        let result = match store.save(ledger) {
            Ok(()) if phase == ResetPhase::Preflight => runtime.preflight(manifest, ledger).await,
            Ok(()) => {
                run_phase(
                    runtime,
                    manifest,
                    phase,
                    &mut ResourceProgress::new(ledger, store),
                )
                .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(evidence) => ledger.mark_complete(phase, evidence),
            Err(error) => {
                ledger.mark_failed(phase, &error);
                return Err((phase, error));
            }
        }
        if let Err(error) = store.save(ledger) {
            ledger.mark_failed(phase, &error);
            return Err((phase, error));
        }
    }
    Ok(())
}

async fn release_locks<R: ResetPhases>(
    runtime: &mut R,
    ledger: &mut ResetLedger,
    store: &LedgerStore,
) -> ResetResult<()> {
    ledger.mark_running(ResetPhase::Release);
    let persisted = store.save(ledger);
    // 即使阶段记账失败也尝试关闭实际连接，但不能把缺失的耐久证据记为成功。
    let result = match (persisted, runtime.release().await) {
        (Err(error), Err(release)) => Err(ResetError::new(format!("{error}；{release}"))),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    };
    match &result {
        Ok(()) => ledger.mark_complete(ResetPhase::Release, PhaseEvidence::new()),
        Err(error) => ledger.mark_failed(ResetPhase::Release, error),
    }
    store.save(ledger)?;
    result
}

async fn run_phase<R: ResetPhases>(
    runtime: &mut R,
    manifest: &ResetManifest,
    phase: ResetPhase,
    progress: &mut ResourceProgress<'_>,
) -> ResetResult<PhaseEvidence> {
    match phase {
        ResetPhase::Preflight | ResetPhase::Release => {
            Err(ResetError::new("preflight/release 不能作为普通 phase 执行"))
        }
        ResetPhase::ObjectStorage => runtime.purge_object_storage(manifest, progress).await,
        ResetPhase::Redis => runtime.purge_redis(manifest, progress).await,
        ResetPhase::Databases => runtime.recreate_databases(manifest, progress).await,
        ResetPhase::ControlBaseline => runtime.migrate_control(manifest).await,
        ResetPhase::TenantBaselines => runtime.migrate_tenants(manifest).await,
        ResetPhase::Verification => runtime.verify(manifest).await,
    }
}

pub struct ResourceProgress<'a> {
    ledger: &'a mut ResetLedger,
    store: &'a LedgerStore,
}

impl<'a> ResourceProgress<'a> {
    fn new(ledger: &'a mut ResetLedger, store: &'a LedgerStore) -> Self {
        Self { ledger, store }
    }

    pub fn is_complete(&self, key: &str) -> bool {
        self.ledger.resource_complete(key)
    }

    pub fn is_started(&self, key: &str) -> bool {
        self.ledger.resource_started(key)
    }

    pub fn identity(&self, key: &str) -> Option<&str> {
        self.ledger.resource_identity(key)
    }

    pub fn begin(&mut self, key: &str) -> ResetResult<()> {
        self.ledger.mark_resource_running(key, None)?;
        self.store.save(self.ledger)
    }

    pub fn begin_with_identity(&mut self, key: &str, identity: &str) -> ResetResult<()> {
        self.ledger.mark_resource_running(key, Some(identity))?;
        self.store.save(self.ledger)
    }

    pub fn complete(&mut self, key: &str) -> ResetResult<()> {
        self.ledger.mark_resource_complete(key);
        self.store.save(self.ledger)
    }
}
