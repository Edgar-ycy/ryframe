use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{ResetError, ResetResult, model::ResetManifest};

const LEDGER_VERSION: u32 = 3;
const STATE_DIR_ENV: &str = "RYFRAME_RESET_STATE_DIR";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetPhase {
    Preflight,
    ObjectStorage,
    Redis,
    Databases,
    ControlBaseline,
    TenantBaselines,
    Verification,
}

impl ResetPhase {
    pub const ORDERED: [Self; 7] = [
        Self::Preflight,
        Self::ObjectStorage,
        Self::Redis,
        Self::Databases,
        Self::ControlBaseline,
        Self::TenantBaselines,
        Self::Verification,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::ObjectStorage => "object_storage",
            Self::Redis => "redis",
            Self::Databases => "databases",
            Self::ControlBaseline => "control_baseline",
            Self::TenantBaselines => "tenant_baselines",
            Self::Verification => "verification",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Pending,
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceStatus {
    Running,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceRecord {
    pub status: ResourceStatus,
    pub attempts: u32,
    pub physical_identity_sha256: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PhaseRecord {
    pub status: PhaseStatus,
    pub attempts: u32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_error: Option<String>,
    pub evidence: BTreeMap<String, String>,
}

impl Default for PhaseRecord {
    fn default() -> Self {
        Self {
            status: PhaseStatus::Pending,
            attempts: 0,
            started_at: None,
            completed_at: None,
            last_error: None,
            evidence: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResetLedger {
    pub ledger_version: u32,
    pub plan_hash: String,
    pub environment: String,
    pub scope_id: String,
    pub code_sha: String,
    pub config_sha: String,
    pub credential_version: String,
    pub created_at: String,
    pub updated_at: String,
    pub phases: BTreeMap<ResetPhase, PhaseRecord>,
    pub resources: BTreeMap<String, ResourceRecord>,
}

impl ResetLedger {
    fn new(manifest: &ResetManifest, plan_hash: &str) -> Self {
        let now = now();
        Self {
            ledger_version: LEDGER_VERSION,
            plan_hash: plan_hash.to_owned(),
            environment: manifest.environment.clone(),
            scope_id: manifest.scope_id.clone(),
            code_sha: manifest.code_sha.clone(),
            config_sha: manifest.config_sha.clone(),
            credential_version: manifest.credential_version.clone(),
            created_at: now.clone(),
            updated_at: now,
            phases: ResetPhase::ORDERED
                .into_iter()
                .map(|phase| (phase, PhaseRecord::default()))
                .collect(),
            resources: BTreeMap::new(),
        }
    }

    pub fn phase(&self, phase: ResetPhase) -> &PhaseRecord {
        self.phases
            .get(&phase)
            .expect("所有 reset phase 必须在账本创建时登记")
    }

    pub fn phase_complete(&self, phase: ResetPhase) -> bool {
        self.phase(phase).status == PhaseStatus::Complete
    }

    pub fn mark_running(&mut self, phase: ResetPhase) {
        let record = self
            .phases
            .get_mut(&phase)
            .expect("所有 reset phase 必须存在");
        record.status = PhaseStatus::Running;
        record.attempts = record.attempts.saturating_add(1);
        record.started_at = Some(now());
        record.completed_at = None;
        record.last_error = None;
        record.evidence.clear();
        self.updated_at = now();
    }

    pub fn mark_complete(&mut self, phase: ResetPhase, evidence: BTreeMap<String, String>) {
        let record = self
            .phases
            .get_mut(&phase)
            .expect("所有 reset phase 必须存在");
        record.status = PhaseStatus::Complete;
        record.completed_at = Some(now());
        record.last_error = None;
        record.evidence = evidence;
        self.updated_at = now();
    }

    pub fn mark_failed(&mut self, phase: ResetPhase, error: &ResetError) {
        let record = self
            .phases
            .get_mut(&phase)
            .expect("所有 reset phase 必须存在");
        record.status = PhaseStatus::Failed;
        record.completed_at = None;
        record.last_error = Some(error.to_string());
        self.updated_at = now();
    }

    pub fn resource_started(&self, key: &str) -> bool {
        self.resources.contains_key(key)
    }

    pub fn resource_complete(&self, key: &str) -> bool {
        self.resources
            .get(key)
            .is_some_and(|record| record.status == ResourceStatus::Complete)
    }

    pub fn resource_identity(&self, key: &str) -> Option<&str> {
        self.resources
            .get(key)
            .and_then(|record| record.physical_identity_sha256.as_deref())
    }

    pub fn mark_resource_running(
        &mut self,
        key: &str,
        physical_identity_sha256: Option<&str>,
    ) -> ResetResult<()> {
        let updated_at = now();
        let record = self
            .resources
            .entry(key.to_owned())
            .or_insert(ResourceRecord {
                status: ResourceStatus::Running,
                attempts: 0,
                physical_identity_sha256: physical_identity_sha256.map(str::to_owned),
                updated_at: updated_at.clone(),
            });
        if let Some(identity) = physical_identity_sha256 {
            match record.physical_identity_sha256.as_deref() {
                Some(existing) if existing != identity => {
                    return Err(ResetError::new(
                        "外部资源物理身份与耐久 reset 进度不一致，拒绝续跑",
                    ));
                }
                None => record.physical_identity_sha256 = Some(identity.to_owned()),
                Some(_) => {}
            }
        }
        record.status = ResourceStatus::Running;
        record.attempts = record.attempts.saturating_add(1);
        record.updated_at = updated_at.clone();
        self.updated_at = updated_at;
        Ok(())
    }

    pub fn mark_resource_complete(&mut self, key: &str) {
        let updated_at = now();
        let record = self
            .resources
            .get_mut(key)
            .expect("资源必须在完成前持久化 running 状态");
        record.status = ResourceStatus::Complete;
        record.updated_at = updated_at.clone();
        self.updated_at = updated_at;
    }

    pub fn rewind_interrupted_baselines(&mut self, manifest: &ResetManifest) -> ResetResult<bool> {
        let interrupted = [ResetPhase::ControlBaseline, ResetPhase::TenantBaselines]
            .into_iter()
            .any(|phase| {
                matches!(
                    self.phase(phase).status,
                    PhaseStatus::Running | PhaseStatus::Failed
                )
            });
        if !interrupted {
            return Ok(false);
        }
        if !self.phase_complete(ResetPhase::Databases) {
            return Err(ResetError::new(
                "baseline 中断时数据库重建 phase 未完成，拒绝不一致续跑",
            ));
        }
        let updated_at = now();
        for database in &manifest.databases {
            let key = crate::model::database_resource_key(database);
            let record = self
                .resources
                .get_mut(&key)
                .ok_or_else(|| ResetError::new("baseline 中断但缺少逐数据库耐久进度，拒绝续跑"))?;
            record.status = ResourceStatus::Running;
            record.updated_at = updated_at.clone();
        }
        for phase in [
            ResetPhase::Databases,
            ResetPhase::ControlBaseline,
            ResetPhase::TenantBaselines,
            ResetPhase::Verification,
        ] {
            self.phases.insert(phase, PhaseRecord::default());
        }
        self.updated_at = updated_at;
        Ok(true)
    }

    fn validate_identity(&self, manifest: &ResetManifest, plan_hash: &str) -> ResetResult<()> {
        let allowed_resources = crate::model::resource_keys(manifest);
        if self.ledger_version != LEDGER_VERSION
            || self.plan_hash != plan_hash
            || self.environment != manifest.environment
            || self.scope_id != manifest.scope_id
            || self.code_sha != manifest.code_sha
            || self.config_sha != manifest.config_sha
            || self.credential_version != manifest.credential_version
            || self.phases.len() != ResetPhase::ORDERED.len()
            || ResetPhase::ORDERED
                .iter()
                .any(|phase| !self.phases.contains_key(phase))
            || self
                .resources
                .keys()
                .any(|key| !allowed_resources.contains(key))
        {
            return Err(ResetError::new(
                "本地 reset phase ledger 与当前不可变清单不匹配，拒绝续跑",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct LedgerStore {
    ledger_path: PathBuf,
    report_path: PathBuf,
}

impl LedgerStore {
    pub fn from_environment(manifest: &ResetManifest, plan_hash: &str) -> ResetResult<Self> {
        let base = match std::env::var(STATE_DIR_ENV) {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            Ok(_) => return Err(ResetError::new("RYFRAME_RESET_STATE_DIR 不能是空路径")),
            Err(std::env::VarError::NotPresent) => {
                return Err(ResetError::new(
                    "execute 必须显式提供绝对 RYFRAME_RESET_STATE_DIR",
                ));
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(ResetError::new(
                    "RYFRAME_RESET_STATE_DIR 必须使用有效 Unicode 编码",
                ));
            }
        };
        if !base.is_absolute()
            || base
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(ResetError::new(
                "RYFRAME_RESET_STATE_DIR 必须是无点号跳转的绝对路径",
            ));
        }
        Self::new(&base, manifest, plan_hash)
    }

    pub fn new(base: &Path, manifest: &ResetManifest, plan_hash: &str) -> ResetResult<Self> {
        if plan_hash.len() != 64 || !plan_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ResetError::new("phase ledger 收到非法 plan hash"));
        }
        let stem = format!(
            "{}-{}-{}",
            manifest.environment, manifest.scope_id, plan_hash
        );
        Ok(Self {
            ledger_path: base.join(format!("{stem}.ledger.json")),
            report_path: base.join(format!("{stem}.report.json")),
        })
    }

    pub fn load_or_create(
        &self,
        manifest: &ResetManifest,
        plan_hash: &str,
    ) -> ResetResult<ResetLedger> {
        let backup = backup_path(&self.ledger_path)?;
        let source = if self.ledger_path.exists() {
            Some((&self.ledger_path, false))
        } else if backup.exists() {
            Some((&backup, true))
        } else {
            None
        };
        if let Some((source, recovered)) = source {
            let bytes =
                fs::read(source).map_err(|_| ResetError::new("无法读取本地 reset phase ledger"))?;
            let ledger: ResetLedger = serde_json::from_slice(&bytes)
                .map_err(|_| ResetError::new("本地 reset phase ledger 格式无效"))?;
            ledger.validate_identity(manifest, plan_hash)?;
            if recovered {
                self.save(&ledger)?;
            }
            return Ok(ledger);
        }
        let ledger = ResetLedger::new(manifest, plan_hash);
        self.save(&ledger)?;
        Ok(ledger)
    }

    pub fn save(&self, ledger: &ResetLedger) -> ResetResult<()> {
        write_json_atomically(&self.ledger_path, ledger)
    }

    pub fn write_report<T: Serialize>(&self, report: &T) -> ResetResult<()> {
        write_json_atomically(&self.report_path, report)
    }

    pub fn report_path(&self) -> &Path {
        &self.report_path
    }
}

fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> ResetResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| ResetError::new("reset 状态文件缺少父目录"))?;
    fs::create_dir_all(parent).map_err(|_| ResetError::new("无法创建 reset 状态目录"))?;
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| ResetError::new("无法编码 reset 状态文件"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ResetError::new("reset 状态文件名无效"))?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|_| ResetError::new("无法创建 reset 临时状态文件"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ResetError::new("无法持久化 reset 临时状态文件"))?;
    drop(file);
    let backup = backup_path(path)?;
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(|_| ResetError::new("无法更新 reset 备用状态文件"))?;
        }
        fs::rename(path, &backup).map_err(|_| ResetError::new("无法保留 reset 上一版状态文件"))?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if !path.exists() && backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(ResetError::new(format!("无法提交 reset 状态文件：{error}")));
    }
    Ok(())
}

fn backup_path(path: &Path) -> ResetResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| ResetError::new("reset 状态文件缺少父目录"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ResetError::new("reset 状态文件名无效"))?;
    Ok(parent.join(format!(".{file_name}.previous")))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        DatabaseConnectionIdentity, LegacyOwnershipPolicy, ObjectStorageResource, PhysicalDatabase,
        database_resource_key,
    };

    fn manifest() -> ResetManifest {
        ResetManifest {
            manifest_version: 3,
            environment: "test".into(),
            scope_id: "test-a".into(),
            code_sha: "a".repeat(40),
            config_sha: "b".repeat(64),
            credential_version: "test-v1".into(),
            confirmation_phrase: "RESET-RYFRAME-test-test-a".into(),
            legacy_ownership: LegacyOwnershipPolicy {
                mysql_exclusive: false,
                redis_exclusive: false,
                object_storage_exclusive: false,
            },
            redis: None,
            object_storage: ObjectStorageResource {
                backend: "local".into(),
                endpoint: "unused".into(),
                use_ssl: false,
                region: String::new(),
                access_key_env: None,
                secret_key_env: None,
                prefixes: Vec::new(),
            },
            databases: Vec::new(),
        }
    }

    #[test]
    fn same_hash_resumes_and_other_identity_is_rejected() {
        let manifest = manifest();
        let base = std::env::temp_dir().join(format!(
            "ryframe-reset-ledger-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let hash = "c".repeat(64);
        let store = LedgerStore::new(&base, &manifest, &hash).expect("创建账本路径");
        let mut ledger = store.load_or_create(&manifest, &hash).expect("创建账本");
        ledger.mark_running(ResetPhase::ObjectStorage);
        ledger.mark_complete(ResetPhase::ObjectStorage, BTreeMap::new());
        store.save(&ledger).expect("保存账本");
        let resumed = store
            .load_or_create(&manifest, &hash)
            .expect("相同清单续跑");
        assert!(resumed.phase_complete(ResetPhase::ObjectStorage));

        let mut changed = manifest;
        changed.config_sha = "d".repeat(64);
        assert!(store.load_or_create(&changed, &hash).is_err());
        fs::remove_dir_all(base).expect("清理测试状态目录");
    }

    #[test]
    fn recovers_ledger_from_previous_copy_after_interrupted_replace() {
        let manifest = manifest();
        let base = std::env::temp_dir().join(format!(
            "ryframe-reset-ledger-recovery-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let hash = "e".repeat(64);
        let store = LedgerStore::new(&base, &manifest, &hash).expect("创建账本路径");
        let mut ledger = store.load_or_create(&manifest, &hash).expect("创建账本");
        ledger.mark_running(ResetPhase::ObjectStorage);
        store.save(&ledger).expect("生成上一版账本");

        let backup = backup_path(&store.ledger_path).expect("备用路径");
        if backup.exists() {
            fs::remove_file(&backup).expect("清理更早的备用账本");
        }
        fs::rename(&store.ledger_path, &backup).expect("模拟主账本替换中断");
        let recovered = store
            .load_or_create(&manifest, &hash)
            .expect("从上一版账本恢复");

        assert_eq!(
            recovered.phase(ResetPhase::ObjectStorage).status,
            PhaseStatus::Running
        );
        assert!(store.ledger_path.exists());
        assert!(backup.exists());
        fs::remove_dir_all(base).expect("清理测试状态目录");
    }

    #[test]
    fn physical_identity_mismatch_is_rejected_before_resume() {
        let manifest = manifest();
        let mut ledger = ResetLedger::new(&manifest, &"f".repeat(64));
        ledger
            .mark_resource_running("database:test", Some("identity-a"))
            .expect("首次物理身份可以持久化");
        assert!(
            ledger
                .mark_resource_running("database:test", Some("identity-b"))
                .is_err()
        );
        assert_eq!(
            ledger.resource_identity("database:test"),
            Some("identity-a")
        );
    }

    #[test]
    fn interrupted_baseline_rewinds_database_phases_and_keeps_identity() {
        let mut manifest = manifest();
        manifest.databases.push(PhysicalDatabase {
            host: "localhost".into(),
            port: 3306,
            database: "ryframe_test".into(),
            connection: DatabaseConnectionIdentity {
                username: "reset".into(),
                password_env: "RYFRAME_TEST_PASSWORD".into(),
                tls_mode: "required".into(),
                tls_ca_sha256: None,
                tls_client_cert_sha256: None,
                tls_client_key_ref_sha256: None,
            },
            target_keys: vec!["shared-control".into()],
            control_baseline: true,
            tenant_baseline: true,
            ownership_markers: BTreeMap::new(),
        });
        let mut ledger = ResetLedger::new(&manifest, &"a".repeat(64));
        let resource_key = database_resource_key(&manifest.databases[0]);
        ledger
            .mark_resource_running(&resource_key, Some("physical-a"))
            .expect("记录数据库物理身份");
        ledger.mark_resource_complete(&resource_key);
        ledger.mark_running(ResetPhase::Databases);
        ledger.mark_complete(ResetPhase::Databases, BTreeMap::new());
        ledger.mark_running(ResetPhase::ControlBaseline);
        ledger.mark_failed(
            ResetPhase::ControlBaseline,
            &ResetError::new("模拟 baseline 中断"),
        );

        assert!(
            ledger
                .rewind_interrupted_baselines(&manifest)
                .expect("中断 baseline 可以安全回退")
        );
        assert_eq!(
            ledger.phase(ResetPhase::Databases).status,
            PhaseStatus::Pending
        );
        assert_eq!(
            ledger.phase(ResetPhase::ControlBaseline).status,
            PhaseStatus::Pending
        );
        assert!(!ledger.resource_complete(&resource_key));
        assert_eq!(ledger.resource_identity(&resource_key), Some("physical-a"));
    }
}
