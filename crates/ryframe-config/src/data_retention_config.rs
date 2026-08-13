use serde::Deserialize;

/// 后台持久化数据的分级保留策略。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRetentionConfig {
    #[serde(default = "default_cleanup_batch_size")]
    pub cleanup_batch_size: usize,
    #[serde(default = "default_max_rows_per_resource_per_run")]
    pub max_rows_per_resource_per_run: usize,
    #[serde(default = "default_background_job_succeeded_days")]
    pub background_job_succeeded_days: u32,
    #[serde(default = "default_outbox_published_days")]
    pub outbox_published_days: u32,
    #[serde(default = "default_schedule_execution_days")]
    pub schedule_execution_days: u32,
    #[serde(default = "default_export_job_history_days")]
    pub export_job_history_days: u32,
    #[serde(default = "default_operation_log_days")]
    pub operation_log_days: u32,
    #[serde(default = "default_login_log_days")]
    pub login_log_days: u32,
    #[serde(default = "default_user_import_history_days")]
    pub user_import_history_days: u32,
    #[serde(default = "default_user_import_artifact_hours")]
    pub user_import_artifact_hours: u32,
    #[serde(default = "default_retention_run_days")]
    pub retention_run_days: u32,
    #[serde(default = "default_service_access_audit_days")]
    pub service_access_audit_days: u32,
}

impl Default for DataRetentionConfig {
    fn default() -> Self {
        Self {
            cleanup_batch_size: default_cleanup_batch_size(),
            max_rows_per_resource_per_run: default_max_rows_per_resource_per_run(),
            background_job_succeeded_days: default_background_job_succeeded_days(),
            outbox_published_days: default_outbox_published_days(),
            schedule_execution_days: default_schedule_execution_days(),
            export_job_history_days: default_export_job_history_days(),
            operation_log_days: default_operation_log_days(),
            login_log_days: default_login_log_days(),
            user_import_history_days: default_user_import_history_days(),
            user_import_artifact_hours: default_user_import_artifact_hours(),
            retention_run_days: default_retention_run_days(),
            service_access_audit_days: default_service_access_audit_days(),
        }
    }
}

impl DataRetentionConfig {
    /// 校验清理批次和各类数据的保留窗口。
    pub fn validate(&self) -> Result<(), String> {
        if !(100..=5_000).contains(&self.cleanup_batch_size) {
            return Err("data_retention.cleanup_batch_size 必须在 100 到 5000 之间".into());
        }
        if self.max_rows_per_resource_per_run < self.cleanup_batch_size
            || self.max_rows_per_resource_per_run > 5_000_000
        {
            return Err(
                "data_retention.max_rows_per_resource_per_run 必须不小于批大小且不超过 5000000"
                    .into(),
            );
        }
        for (name, days) in [
            (
                "background_job_succeeded_days",
                self.background_job_succeeded_days,
            ),
            ("outbox_published_days", self.outbox_published_days),
            ("schedule_execution_days", self.schedule_execution_days),
            ("export_job_history_days", self.export_job_history_days),
            ("operation_log_days", self.operation_log_days),
            ("login_log_days", self.login_log_days),
            ("user_import_history_days", self.user_import_history_days),
            ("service_access_audit_days", self.service_access_audit_days),
        ] {
            if !(1..=3_650).contains(&days) {
                return Err(format!("data_retention.{name} 必须在 1 到 3650 天之间"));
            }
        }
        if !(1..=8_760).contains(&self.user_import_artifact_hours) {
            return Err(
                "data_retention.user_import_artifact_hours 必须在 1 到 8760 小时之间".into(),
            );
        }
        if !(30..=3_650).contains(&self.retention_run_days) {
            return Err("data_retention.retention_run_days 必须在 30 到 3650 天之间".into());
        }
        Ok(())
    }
}

const fn default_cleanup_batch_size() -> usize {
    500
}
const fn default_max_rows_per_resource_per_run() -> usize {
    50_000
}
const fn default_background_job_succeeded_days() -> u32 {
    30
}
const fn default_outbox_published_days() -> u32 {
    30
}
const fn default_schedule_execution_days() -> u32 {
    180
}
const fn default_export_job_history_days() -> u32 {
    180
}
const fn default_operation_log_days() -> u32 {
    180
}
const fn default_login_log_days() -> u32 {
    180
}
const fn default_user_import_history_days() -> u32 {
    180
}
const fn default_user_import_artifact_hours() -> u32 {
    168
}
const fn default_retention_run_days() -> u32 {
    730
}
const fn default_service_access_audit_days() -> u32 {
    180
}
