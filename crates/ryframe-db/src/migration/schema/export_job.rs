pub(super) const EXPORT_JOB_DDL: &str = r####"CREATE TABLE IF NOT EXISTS `sys_export_job` (
    `id` BIGINT NOT NULL,
    `tenant_id` VARCHAR(64) NOT NULL,
    `requester_id` BIGINT NOT NULL,
    `resource` VARCHAR(64) NOT NULL,
    `background_job_id` BIGINT NOT NULL,
    `request_params` JSON NOT NULL,
    `permission_code` VARCHAR(128) NOT NULL,
    `snapshot_at` DATETIME NOT NULL,
    `upper_id` BIGINT NOT NULL,
    `matched_rows` BIGINT NOT NULL,
    `status` VARCHAR(16) NOT NULL DEFAULT 'queued',
    `result_file_id` BIGINT DEFAULT NULL,
    `result_file_name` VARCHAR(255) DEFAULT NULL,
    `content_type` VARCHAR(128) DEFAULT NULL,
    `file_size` BIGINT DEFAULT NULL,
    `expires_at` DATETIME DEFAULT NULL,
    `error_message` TEXT DEFAULT NULL,
    `created_at` DATETIME NOT NULL,
    `updated_at` DATETIME NOT NULL,
    `completed_at` DATETIME DEFAULT NULL,
    `notification_read_at` DATETIME DEFAULT NULL,
    `delete_pending_at` DATETIME DEFAULT NULL,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uq_export_job_background` (`background_job_id`),
    UNIQUE KEY `uq_export_job_result_file` (`result_file_id`),
    KEY `idx_export_job_requester` (`tenant_id`, `requester_id`, `delete_pending_at`, `created_at`, `id`),
    KEY `idx_export_job_expiry` (`status`, `expires_at`),
    KEY `idx_export_job_history` (`status`, `completed_at`, `id`),
    KEY `idx_export_job_notification` (`tenant_id`, `requester_id`, `delete_pending_at`, `notification_read_at`, `status`, `completed_at`, `id`),
    KEY `idx_export_job_delete_pending` (`delete_pending_at`, `id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"####;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_file_is_exclusive_to_one_export() {
        assert!(
            EXPORT_JOB_DDL.contains("UNIQUE KEY `uq_export_job_result_file` (`result_file_id`)")
        );
    }
}
