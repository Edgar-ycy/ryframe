use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 恢复演练只绑定显式登记的隔离目标。连接凭据由进程外的配置提供。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestorePlan {
    pub id: String,
    pub backup_id: String,
    pub scope_id: String,
    pub fault_at: DateTime<Utc>,
    pub databases: Vec<RestoreDatabase>,
    pub object_endpoint: String,
    pub object_prefix: String,
    pub api_ready_url: String,
    pub worker_ready_url: String,
    pub frontend_sha: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreDatabase {
    pub source_key: String,
    pub target_key: String,
    pub server_uuid: String,
    pub database: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreStatus {
    Running,
    DataVerified,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreRecord {
    pub plan: RestorePlan,
    pub plan_hash: String,
    pub status: RestoreStatus,
    pub started_at: DateTime<Utc>,
    pub data_verified_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub recovered_at: DateTime<Utc>,
    pub failure: Option<String>,
}

/// 由真实服务浏览器验收入口写出的结果；必须绑定当前演练与精确源码。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBusinessProof {
    pub restore_id: String,
    pub plan_hash: String,
    pub backend_sha: String,
    pub frontend_sha: String,
    pub scope_id: String,
    pub runtime_receipt_sha256: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub scenarios: Vec<RestoreScenarioResult>,
    pub unexpected_console_messages: u64,
    pub unexpected_network_failures: u64,
    pub axe_serious_or_critical: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreScenarioResult {
    pub name: String,
    pub succeeded: bool,
}
