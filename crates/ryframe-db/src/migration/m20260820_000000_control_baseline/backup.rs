pub(crate) const BACKUP_TABLE_STATEMENTS: [&str; 3] = [
    r#"CREATE TABLE IF NOT EXISTS `sys_backup_set` (
        `id` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `scope_id` varchar(48) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `manifest_hash` char(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `captured_at` datetime(6) NOT NULL,
        `retention_until` datetime(6) NOT NULL,
        `checked_at` datetime(6) NOT NULL,
        `valid` tinyint(1) NOT NULL,
        `payload` json NOT NULL,
        PRIMARY KEY (`id`),
        KEY `idx_backup_set_scope_capture` (`scope_id`, `captured_at`),
        CONSTRAINT `ck_backup_set_time` CHECK (`retention_until` > `captured_at`),
        CONSTRAINT `ck_backup_set_valid` CHECK (`valid` IN (0, 1))
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
    r#"CREATE TABLE IF NOT EXISTS `sys_backup_resource` (
        `backup_id` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `resource_key` varchar(140) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        PRIMARY KEY (`backup_id`, `resource_key`),
        KEY `idx_backup_resource_key` (`resource_key`, `backup_id`),
        CONSTRAINT `fk_backup_resource_set` FOREIGN KEY (`backup_id`)
            REFERENCES `sys_backup_set` (`id`) ON DELETE RESTRICT ON UPDATE RESTRICT
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
    r#"CREATE TABLE IF NOT EXISTS `sys_restore_run` (
        `id` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `backup_id` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `scope_id` varchar(48) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `status` varchar(24) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
        `started_at` datetime(6) NOT NULL,
        `completed_at` datetime(6) DEFAULT NULL,
        `recovered_at` datetime(6) NOT NULL,
        `payload` json NOT NULL,
        PRIMARY KEY (`id`),
        KEY `idx_restore_run_backup` (`backup_id`, `started_at`),
        KEY `idx_restore_run_completed` (`completed_at`),
        CONSTRAINT `fk_restore_run_backup` FOREIGN KEY (`backup_id`)
            REFERENCES `sys_backup_set` (`id`) ON DELETE RESTRICT ON UPDATE RESTRICT,
        CONSTRAINT `ck_restore_run_status`
            CHECK (`status` IN ('running', 'data_verified', 'succeeded', 'failed')),
        CONSTRAINT `ck_restore_run_completed`
            CHECK ((`status` IN ('running', 'data_verified') AND `completed_at` IS NULL)
                OR (`status` IN ('succeeded', 'failed')
                    AND `completed_at` IS NOT NULL
                    AND `completed_at` >= `started_at`))
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
];
