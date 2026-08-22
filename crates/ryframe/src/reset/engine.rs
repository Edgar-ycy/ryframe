use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;

use crate::reset::{
    ResetError, ResetResult,
    ledger::{LedgerStore, PhaseRecord, ResetLedger, ResetPhase, ResourceRecord},
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

#[derive(Clone, Debug, Serialize)]
pub struct ResetReport {
    pub report_version: u32,
    pub plan_hash: String,
    pub environment: String,
    pub scope_id: String,
    pub code_sha: String,
    pub config_sha: String,
    pub credential_version: String,
    pub status: &'static str,
    pub finished_at: String,
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
        status: &'static str,
        failed_phase: Option<ResetPhase>,
    ) -> Self {
        Self {
            report_version: 1,
            plan_hash: plan_hash.to_owned(),
            environment: manifest.environment.clone(),
            scope_id: manifest.scope_id.clone(),
            code_sha: manifest.code_sha.clone(),
            config_sha: manifest.config_sha.clone(),
            credential_version: manifest.credential_version.clone(),
            status,
            finished_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
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
    if ledger.rewind_interrupted_baselines(manifest)? {
        store.save(&ledger)?;
    }
    if phases_after_preflight_complete(&ledger) {
        let report = ResetReport::from_ledger(manifest, plan_hash, &ledger, "succeeded", None);
        store.write_report(&report)?;
        return Ok(report);
    }

    ledger.mark_running(ResetPhase::Preflight);
    store.save(&ledger)?;
    let preflight_result = runtime.preflight(manifest, &ledger).await;
    match preflight_result {
        Ok(evidence) => {
            ledger.mark_complete(ResetPhase::Preflight, evidence);
            store.save(&ledger)?;
        }
        Err(error) => {
            ledger.mark_failed(ResetPhase::Preflight, &error);
            store.save(&ledger)?;
            let _ = runtime.release().await;
            let report = ResetReport::from_ledger(
                manifest,
                plan_hash,
                &ledger,
                "failed",
                Some(ResetPhase::Preflight),
            );
            store.write_report(&report)?;
            return Err(error);
        }
    }

    for phase in ResetPhase::ORDERED.into_iter().skip(1) {
        if ledger.phase_complete(phase) {
            continue;
        }
        ledger.mark_running(phase);
        store.save(&ledger)?;
        let result = {
            let mut progress = ResourceProgress::new(&mut ledger, store);
            run_phase(runtime, manifest, phase, &mut progress).await
        };
        match result {
            Ok(evidence) => {
                ledger.mark_complete(phase, evidence);
                store.save(&ledger)?;
            }
            Err(error) => {
                ledger.mark_failed(phase, &error);
                store.save(&ledger)?;
                let _ = runtime.release().await;
                let report =
                    ResetReport::from_ledger(manifest, plan_hash, &ledger, "failed", Some(phase));
                store.write_report(&report)?;
                return Err(error);
            }
        }
    }

    if let Err(error) = runtime.release().await {
        let report = ResetReport::from_ledger(
            manifest,
            plan_hash,
            &ledger,
            "failed",
            Some(ResetPhase::Verification),
        );
        store.write_report(&report)?;
        return Err(error);
    }
    let report = ResetReport::from_ledger(manifest, plan_hash, &ledger, "succeeded", None);
    store.write_report(&report)?;
    Ok(report)
}

async fn run_phase<R: ResetPhases>(
    runtime: &mut R,
    manifest: &ResetManifest,
    phase: ResetPhase,
    progress: &mut ResourceProgress<'_>,
) -> ResetResult<PhaseEvidence> {
    match phase {
        ResetPhase::Preflight => Err(ResetError::new("preflight 不能作为普通 phase 执行")),
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

fn phases_after_preflight_complete(ledger: &ResetLedger) -> bool {
    ResetPhase::ORDERED
        .into_iter()
        .skip(1)
        .all(|phase| ledger.phase_complete(phase))
}
