//! seed 关闭只读取已登记共享控制库的一致排空快照。
use std::time::Duration;

use crate::{
    Result,
    recovery::clone::native::{
        export::transport::{CommandOutput, CommandSpec, SystemTransport, Transport},
        post_process::{files, read_target_control},
        s3,
        target_verify::storage::StorageInputs,
    },
};
use serde_json::Value;

use super::super::protocol::guards::parse_snapshot;

pub(super) fn observe(
    target: &StorageInputs<'_>,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    match read_target_control(
        target,
        ("seed-drain-snapshot", DRAIN_SQL),
        &DrainTransport,
        checkpoint,
        parse_snapshot,
    ) {
        Ok(value) => Ok(value),
        Err(error) => files::combine(Err(error), checkpoint()),
    }
}

struct DrainTransport;

impl Transport for DrainTransport {
    fn run(&self, mut command: CommandSpec) -> Result<CommandOutput> {
        // 每次 MySQL 读取保留原排空命令的 30 秒上限，先于外层请求超时完成回收。
        command.timeout = command.timeout.min(Duration::from_secs(30));
        SystemTransport.run(command)
    }

    fn s3(&self, _: s3::Request) -> Result<Value> {
        Err("排空窗口只允许读取共享控制库".into())
    }
}

// 在一个 SELECT 中读取数据库时间、活跃项和终态计数，保持同一数据库快照。
const DRAIN_SQL: &str = concat!(
    "SELECT JSON_OBJECT('database_time',",
    "DATE_FORMAT(UTC_TIMESTAMP(6),'%Y-%m-%dT%H:%i:%s.%fZ'),",
    "'active',JSON_OBJECT(",
    "'background_job_status',(SELECT COUNT(*) FROM sys_background_job WHERE status NOT IN ('succeeded','dead'))",
    ",'background_job_lease',(SELECT COUNT(*) FROM sys_background_job WHERE lease_owner IS NOT NULL OR lease_until IS NOT NULL)",
    ",'background_job_completion',(SELECT COUNT(*) FROM sys_background_job WHERE completed_at IS NULL)",
    ",'background_job_attempt',(SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome NOT IN ('succeeded','failed','dead','deferred','lease_expired') OR closed_at IS NULL OR NOT ((outcome='lease_expired' AND finished_at IS NULL) OR (outcome IN ('succeeded','failed','dead','deferred') AND finished_at IS NOT NULL)))",
    ",'outbox_event_status',(SELECT COUNT(*) FROM sys_outbox_event WHERE status NOT IN ('published','dead'))",
    ",'outbox_event_lease',(SELECT COUNT(*) FROM sys_outbox_event WHERE lease_owner IS NOT NULL OR lease_until IS NOT NULL)",
    ",'export_job_status',(SELECT COUNT(*) FROM sys_export_job WHERE status NOT IN ('succeeded','failed','cancelled','expired'))",
    ",'export_job_request',(SELECT COUNT(*) FROM sys_export_job WHERE active_request_fingerprint IS NOT NULL)",
    ",'export_job_delete',(SELECT COUNT(*) FROM sys_export_job WHERE delete_pending_at IS NOT NULL)",
    ",'user_import_job',(SELECT COUNT(*) FROM sys_user_import_job WHERE status NOT IN ('succeeded','partial','failed','cancelled'))",
    ",'enabled_schedule',(SELECT COUNT(*) FROM sys_job_schedule WHERE del_flag<>'2' AND enabled<>0)",
    ",'schedule_execution',(SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome NOT IN ('enqueued','skipped_misfire','skipped_concurrency','target_unavailable','invalid_configuration') OR (outcome='enqueued' AND background_job_id IS NULL) OR (outcome<>'enqueued' AND background_job_id IS NOT NULL))",
    ",'config_bundle',(SELECT COUNT(*) FROM sys_tenant_config_bundle WHERE status NOT IN ('succeeded','failed','expired'))",
    ",'config_transfer',(SELECT COUNT(*) FROM sys_tenant_config_transfer WHERE status NOT IN ('preview_ready','previewed','applied','rolled_back','failed'))",
    ",'retention_run',(SELECT COUNT(*) FROM sys_data_retention_run WHERE status NOT IN ('succeeded','failed'))",
    ",'password_reset',(SELECT COUNT(*) FROM password_reset_requests WHERE status NOT IN ('completed','expired'))",
    ",'tenant_operation_lease',(SELECT COUNT(*) FROM sys_tenant_operation_lease)",
    ",'tenant_data_migration',(SELECT COUNT(*) FROM sys_tenant_data_migration)",
    ",'tenant_data_migration_item',(SELECT COUNT(*) FROM sys_tenant_data_migration_item)",
    ",'file_upload',(SELECT COUNT(*) FROM sys_file WHERE upload_status<>'ready')",
    ",'tenant_status',(SELECT COUNT(*) FROM sys_tenant WHERE status NOT IN ('enabled','disabled'))",
    "),",
    "'terminal_non_success',JSON_OBJECT(",
    "'background_job_dead',(SELECT COUNT(*) FROM sys_background_job WHERE status='dead')",
    ",'background_job_attempt_failed',(SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='failed')",
    ",'background_job_attempt_dead',(SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='dead')",
    ",'background_job_attempt_deferred',(SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='deferred')",
    ",'background_job_attempt_lease_expired',(SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='lease_expired')",
    ",'outbox_event_dead',(SELECT COUNT(*) FROM sys_outbox_event WHERE status='dead')",
    ",'export_job_failed',(SELECT COUNT(*) FROM sys_export_job WHERE status='failed')",
    ",'export_job_cancelled',(SELECT COUNT(*) FROM sys_export_job WHERE status='cancelled')",
    ",'export_job_expired',(SELECT COUNT(*) FROM sys_export_job WHERE status='expired')",
    ",'user_import_partial',(SELECT COUNT(*) FROM sys_user_import_job WHERE status='partial')",
    ",'user_import_failed',(SELECT COUNT(*) FROM sys_user_import_job WHERE status='failed')",
    ",'user_import_cancelled',(SELECT COUNT(*) FROM sys_user_import_job WHERE status='cancelled')",
    ",'schedule_skipped_misfire',(SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='skipped_misfire')",
    ",'schedule_skipped_concurrency',(SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='skipped_concurrency')",
    ",'schedule_target_unavailable',(SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='target_unavailable')",
    ",'schedule_invalid_configuration',(SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='invalid_configuration')",
    ",'config_bundle_failed',(SELECT COUNT(*) FROM sys_tenant_config_bundle WHERE status='failed')",
    ",'config_bundle_expired',(SELECT COUNT(*) FROM sys_tenant_config_bundle WHERE status='expired')",
    ",'config_transfer_failed',(SELECT COUNT(*) FROM sys_tenant_config_transfer WHERE status='failed')",
    ",'retention_run_failed',(SELECT COUNT(*) FROM sys_data_retention_run WHERE status='failed')",
    ",'password_reset_expired',(SELECT COUNT(*) FROM password_reset_requests WHERE status='expired')",
    "));",
);

#[cfg(test)]
mod tests {
    use super::DRAIN_SQL;
    use sha2::{Digest, Sha256};

    #[test]
    fn fixed_query_preserves_the_complete_original_read_only_snapshot() {
        assert_eq!(
            hex::encode(Sha256::digest(DRAIN_SQL.as_bytes())),
            "4bdd9b9e2e5d8af8df0d7cb5dbf6507eb626c2f6c555d4a8b2e6b9ce666a6349"
        );
    }
}
