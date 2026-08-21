// 控制库扩展表的当前结构；不得在此保留历史 ALTER 或兼容转换。
pub(crate) fn lifecycle_table_statements() -> [&'static str; 3] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_data_retention_run` (
            `id` BIGINT NOT NULL,
            `background_job_id` BIGINT NOT NULL,
            `trigger_kind` VARCHAR(16) NOT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'pending',
            `policy_snapshot` JSON NOT NULL,
            `eligible_counts` JSON NOT NULL,
            `deleted_counts` JSON NOT NULL,
            `remaining_counts` JSON NOT NULL,
            `requested_by` BIGINT DEFAULT NULL,
            `error_summary` TEXT DEFAULT NULL,
            `started_at` DATETIME(6) DEFAULT NULL,
            `completed_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_retention_run_background_job` (`background_job_id`),
            KEY `idx_retention_run_created` (`created_at`, `id`),
            KEY `idx_retention_run_history` (`status`, `completed_at`, `id`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='数据保留运行记录'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_user_import_job` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `requester_user_id` BIGINT NOT NULL,
            `background_job_id` BIGINT NOT NULL,
            `idempotency_key_hash` CHAR(64) NOT NULL,
            `source_file_id` BIGINT NOT NULL,
            `source_name_snapshot` VARCHAR(255) NOT NULL,
            `source_sha256` CHAR(64) NOT NULL,
            `duplicate_policy` VARCHAR(24) NOT NULL DEFAULT 'skip_existing',
            `status` VARCHAR(16) NOT NULL DEFAULT 'pending',
            `total_rows` INT NOT NULL DEFAULT 0,
            `processed_rows` INT NOT NULL DEFAULT 0,
            `success_count` INT NOT NULL DEFAULT 0,
            `skipped_count` INT NOT NULL DEFAULT 0,
            `failure_count` INT NOT NULL DEFAULT 0,
            `cancel_requested` TINYINT(1) NOT NULL DEFAULT 0,
            `error_report_file_id` BIGINT DEFAULT NULL,
            `last_error` TEXT DEFAULT NULL,
            `started_at` DATETIME(6) DEFAULT NULL,
            `completed_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_user_import_idempotency` (`tenant_id`, `idempotency_key_hash`),
            UNIQUE KEY `uq_user_import_background_job` (`background_job_id`),
            KEY `idx_user_import_tenant_status` (`tenant_id`, `status`, `created_at`),
            KEY `idx_user_import_history` (`completed_at`, `id`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='异步用户导入任务'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_user_import_row_result` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `import_job_id` BIGINT NOT NULL,
            `row_number` INT NOT NULL,
            `username_snapshot` VARCHAR(64) NOT NULL,
            `outcome` VARCHAR(16) NOT NULL,
            `code` VARCHAR(64) NOT NULL,
            `message` VARCHAR(500) NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_user_import_row` (`import_job_id`, `row_number`),
            KEY `idx_user_import_row_tenant` (`tenant_id`, `import_job_id`, `row_number`),
            CONSTRAINT `fk_user_import_row_job`
                FOREIGN KEY (`import_job_id`) REFERENCES `sys_user_import_job` (`id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户导入异常行结果'"#,
    ]
}

pub(crate) fn tenant_config_table_statements() -> [&'static str; 3] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_bundle` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `origin` VARCHAR(16) NOT NULL,
            `source_tenant_key` VARCHAR(64) NOT NULL,
            `source_tenant_name_snapshot` VARCHAR(128) NOT NULL,
            `package_schema_version` VARCHAR(64) NOT NULL,
            `source_app_version` VARCHAR(32) NOT NULL,
            `file_id` BIGINT DEFAULT NULL,
            `sha256` CHAR(64) DEFAULT NULL,
            `resource_counts` JSON NOT NULL,
            `item_count` INT NOT NULL DEFAULT 0,
            `status` VARCHAR(24) NOT NULL DEFAULT 'pending',
            `background_job_id` BIGINT DEFAULT NULL,
            `idempotency_key_hash` CHAR(64) DEFAULT NULL,
            `created_by` BIGINT NOT NULL,
            `error_summary` TEXT DEFAULT NULL,
            `expires_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_config_bundle_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_tenant_config_bundle_background_job` (`background_job_id`),
            UNIQUE KEY `uq_tenant_config_bundle_idempotency` (`tenant_id`, `created_by`, `idempotency_key_hash`),
            KEY `idx_tenant_config_bundle_list` (`tenant_id`, `created_at`, `id`),
            KEY `idx_tenant_config_bundle_expiry` (`status`, `expires_at`, `id`),
            KEY `idx_tenant_config_bundle_file` (`tenant_id`, `file_id`),
            CONSTRAINT `fk_tenant_config_bundle_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_bundle_file`
                FOREIGN KEY (`tenant_id`, `file_id`)
                REFERENCES `sys_file` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置包'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_transfer` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `bundle_id` BIGINT NOT NULL,
            `idempotency_key_hash` CHAR(64) NOT NULL,
            `request_kind` VARCHAR(24) NOT NULL,
            `request_fingerprint` CHAR(64) NOT NULL,
            `status` VARCHAR(24) NOT NULL DEFAULT 'preview_ready',
            `target_configuration_version` BIGINT NOT NULL,
            `target_authorization_epoch` INT NOT NULL,
            `plan_hash` CHAR(64) DEFAULT NULL,
            `preview_calculated_at` DATETIME(6) DEFAULT NULL,
            `preview_background_job_id` BIGINT DEFAULT NULL,
            `apply_background_job_id` BIGINT DEFAULT NULL,
            `rollback_background_job_id` BIGINT DEFAULT NULL,
            `snapshot_file_id` BIGINT DEFAULT NULL,
            `applied_configuration_version` BIGINT DEFAULT NULL,
            `applied_authorization_epoch` INT DEFAULT NULL,
            `change_counts` JSON NOT NULL,
            `error_summary` TEXT DEFAULT NULL,
            `requested_by` BIGINT NOT NULL,
            `rollback_expires_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_config_transfer_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_tenant_config_transfer_idempotency` (`tenant_id`, `requested_by`, `idempotency_key_hash`),
            UNIQUE KEY `uq_tenant_config_transfer_preview_job` (`preview_background_job_id`),
            UNIQUE KEY `uq_tenant_config_transfer_apply_job` (`apply_background_job_id`),
            UNIQUE KEY `uq_tenant_config_transfer_rollback_job` (`rollback_background_job_id`),
            KEY `idx_tenant_config_transfer_list` (`tenant_id`, `created_at`, `id`),
            KEY `idx_tenant_config_transfer_status` (`tenant_id`, `status`, `created_at`),
            KEY `idx_tenant_config_transfer_bundle` (`tenant_id`, `bundle_id`),
            KEY `idx_tenant_config_transfer_snapshot` (`tenant_id`, `snapshot_file_id`),
            CONSTRAINT `fk_tenant_config_transfer_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_transfer_bundle`
                FOREIGN KEY (`tenant_id`, `bundle_id`)
                REFERENCES `sys_tenant_config_bundle` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_transfer_snapshot`
                FOREIGN KEY (`tenant_id`, `snapshot_file_id`)
                REFERENCES `sys_file` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置迁移'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_transfer_item` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `transfer_id` BIGINT NOT NULL,
            `resource_type` VARCHAR(32) NOT NULL,
            `stable_key` VARCHAR(384) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
            `display_name` VARCHAR(255) NOT NULL,
            `action` VARCHAR(16) NOT NULL,
            `outcome` VARCHAR(16) NOT NULL DEFAULT 'pending',
            `detail_code` VARCHAR(64) DEFAULT NULL,
            `detail` VARCHAR(500) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_config_transfer_item` (`transfer_id`, `resource_type`, `stable_key`),
            KEY `idx_tenant_config_transfer_item_list` (`tenant_id`, `transfer_id`, `id`),
            KEY `idx_tenant_config_transfer_item_action` (`transfer_id`, `action`, `id`),
            CONSTRAINT `fk_tenant_config_transfer_item_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_transfer_item_transfer`
                FOREIGN KEY (`tenant_id`, `transfer_id`)
                REFERENCES `sys_tenant_config_transfer` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置迁移明细'"#,
    ]
}

pub(crate) fn product_capability_table_statements() -> [&'static str; 7] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_product_plan` (
            `id` BIGINT NOT NULL,
            `plan_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `name` VARCHAR(128) NOT NULL,
            `description` VARCHAR(500) DEFAULT NULL,
            `status` CHAR(1) NOT NULL DEFAULT '1',
            `created_by` BIGINT NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_product_plan_key` (`plan_key`),
            KEY `idx_product_plan_status` (`status`, `plan_key`),
            CONSTRAINT `ck_product_plan_status` CHECK (`status` IN ('0', '1')),
            CONSTRAINT `ck_product_plan_key` CHECK (CHAR_LENGTH(`plan_key`) > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='产品套餐'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_product_plan_version` (
            `id` BIGINT NOT NULL,
            `plan_id` BIGINT NOT NULL,
            `version` INT NOT NULL,
            `name` VARCHAR(128) NOT NULL,
            `description` VARCHAR(500) DEFAULT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'draft',
            `created_by` BIGINT NOT NULL,
            `published_by` BIGINT DEFAULT NULL,
            `published_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_product_plan_version` (`plan_id`, `version`),
            KEY `idx_product_plan_version_status` (`plan_id`, `status`, `version`),
            CONSTRAINT `fk_product_plan_version_plan`
                FOREIGN KEY (`plan_id`) REFERENCES `sys_product_plan` (`id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_product_plan_version_status`
                CHECK (`status` IN ('draft', 'published', 'retired'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='产品套餐版本'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_product_plan_capability` (
            `plan_version_id` BIGINT NOT NULL,
            `capability_code` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `variant_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `schema_version` INT NOT NULL,
            `config` JSON NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`plan_version_id`, `capability_code`),
            KEY `idx_product_plan_capability_code` (`capability_code`, `plan_version_id`),
            CONSTRAINT `fk_product_plan_capability_version`
                FOREIGN KEY (`plan_version_id`) REFERENCES `sys_product_plan_version` (`id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_product_plan_capability_code`
                CHECK (CHAR_LENGTH(`capability_code`) > 0),
            CONSTRAINT `ck_product_plan_capability_variant`
                CHECK (CHAR_LENGTH(`variant_code`) > 0),
            CONSTRAINT `ck_product_plan_capability_schema`
                CHECK (`schema_version` > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='产品套餐版本能力'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_product_plan` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `plan_version_id` BIGINT NOT NULL,
            `changed_by` BIGINT DEFAULT NULL,
            `change_reason` VARCHAR(500) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_product_plan_version` (`plan_version_id`, `tenant_id`),
            CONSTRAINT `fk_tenant_product_plan_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_tenant_product_plan_version`
                FOREIGN KEY (`plan_version_id`) REFERENCES `sys_product_plan_version` (`id`)
                ON UPDATE CASCADE ON DELETE RESTRICT
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户产品套餐分配'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_capability_override` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `capability_code` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `enabled` TINYINT(1) NOT NULL,
            `variant_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `schema_version` INT NOT NULL,
            `config` JSON NOT NULL,
            `reason` VARCHAR(500) DEFAULT NULL,
            `changed_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`, `capability_code`),
            KEY `idx_tenant_capability_override_code` (`capability_code`, `tenant_id`),
            CONSTRAINT `fk_tenant_capability_override_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_capability_override_enabled`
                CHECK (`enabled` IN (0, 1)),
            CONSTRAINT `ck_tenant_capability_override_code`
                CHECK (CHAR_LENGTH(`capability_code`) > 0),
            CONSTRAINT `ck_tenant_capability_override_variant`
                CHECK (CHAR_LENGTH(`variant_code`) > 0),
            CONSTRAINT `ck_tenant_capability_override_schema`
                CHECK (`schema_version` > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户能力覆盖'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_provision_request` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `request_token` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `admin_password_hash` VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            CONSTRAINT `fk_tenant_provision_request_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_provision_request_token`
                CHECK (CHAR_LENGTH(`request_token`) = 64)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户创建 Saga 权威幂等请求'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_operation_lease` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `owner_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `operation` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `resource_type` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `resource_id` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `expires_at` DATETIME(6) NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_operation_lease_expiry` (`expires_at`, `tenant_id`),
            KEY `idx_tenant_operation_lease_resource` (`tenant_id`, `resource_type`, `resource_id`),
            CONSTRAINT `fk_tenant_operation_lease_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户统一操作租约'"#,
    ]
}

pub(crate) fn tenant_data_control_table_statements() -> [&'static str; 4] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_placement` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `current_target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `placement_generation` BIGINT NOT NULL,
            `state` VARCHAR(24) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_data_placement_target` (`current_target_key`, `state`, `tenant_id`),
            KEY `idx_tenant_data_placement_state` (`state`, `updated_at`),
            CONSTRAINT `fk_tenant_data_placement_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_data_placement_generation`
                CHECK (`placement_generation` > 0),
            CONSTRAINT `ck_tenant_data_placement_state`
                CHECK (`state` IN ('provisioning', 'active', 'maintenance', 'failed'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据权威放置'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_migration` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `source_target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `source_target_mode` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `source_target_kind` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_target_mode` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_target_kind` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `source_generation` BIGINT NOT NULL,
            `source_switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_generation` BIGINT NOT NULL,
            `source_schema_fingerprint` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_schema_fingerprint` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `plan_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `create_idempotency_key_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `cancel_idempotency_key_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `finalize_idempotency_key_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `state` VARCHAR(24) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `operator_id` BIGINT NOT NULL,
            `cancelled_by` BIGINT DEFAULT NULL,
            `finalized_by` BIGINT DEFAULT NULL,
            `background_job_id` BIGINT DEFAULT NULL,
            `retention_hours` INT NOT NULL DEFAULT 168,
            `error_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `error_detail` VARCHAR(1000) DEFAULT NULL,
            `prechecked_at` DATETIME(6) DEFAULT NULL,
            `queued_at` DATETIME(6) DEFAULT NULL,
            `quiesced_at` DATETIME(6) DEFAULT NULL,
            `frozen_at` DATETIME(6) DEFAULT NULL,
            `copy_started_at` DATETIME(6) DEFAULT NULL,
            `copy_completed_at` DATETIME(6) DEFAULT NULL,
            `verified_at` DATETIME(6) DEFAULT NULL,
            `cut_over_at` DATETIME(6) DEFAULT NULL,
            `activated_at` DATETIME(6) DEFAULT NULL,
            `succeeded_at` DATETIME(6) DEFAULT NULL,
            `retention_until` DATETIME(6) DEFAULT NULL,
            `cancel_requested_at` DATETIME(6) DEFAULT NULL,
            `finalize_requested_at` DATETIME(6) DEFAULT NULL,
            `cleanup_ready_at` DATETIME(6) DEFAULT NULL,
            `finalized_at` DATETIME(6) DEFAULT NULL,
            `failed_at` DATETIME(6) DEFAULT NULL,
            `cancelled_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_data_migration_switch` (`switch_token`),
            UNIQUE KEY `uq_tenant_data_migration_create_key` (`create_idempotency_key_hash`),
            UNIQUE KEY `uq_tenant_data_migration_cancel_key` (`cancel_idempotency_key_hash`),
            UNIQUE KEY `uq_tenant_data_migration_finalize_key` (`finalize_idempotency_key_hash`),
            KEY `idx_tenant_data_migration_tenant` (`tenant_id`, `state`, `created_at`),
            KEY `idx_tenant_data_migration_target` (`target_key`, `state`, `created_at`),
            KEY `idx_tenant_data_migration_job` (`background_job_id`),
            CONSTRAINT `fk_tenant_data_migration_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_data_migration_generation`
                CHECK (`source_generation` > 0 AND `target_generation` > `source_generation`),
            CONSTRAINT `ck_tenant_data_migration_retention`
                CHECK (`retention_hours` BETWEEN 1 AND 8760),
            CONSTRAINT `ck_tenant_data_migration_target_contract`
                CHECK (`source_target_mode` IN ('shared', 'dedicated')
                    AND `target_target_mode` IN ('shared', 'dedicated')
                    AND `source_target_kind` IN ('control', 'mysql')
                    AND `target_target_kind` IN ('control', 'mysql')),
            CONSTRAINT `ck_tenant_data_migration_state`
                CHECK (`state` IN ('prechecking', 'queued', 'quiescing', 'frozen', 'copying', 'verifying', 'cutting_over', 'activating', 'succeeded', 'retention_pending', 'finalized', 'failed', 'cancelled'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据迁移任务'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_migration_item` (
            `id` BIGINT NOT NULL,
            `migration_id` BIGINT NOT NULL,
            `table_name` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `copy_order` INT NOT NULL,
            `state` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `cursor_json` JSON DEFAULT NULL,
            `source_row_count` BIGINT DEFAULT NULL,
            `target_row_count` BIGINT DEFAULT NULL,
            `source_digest` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `target_digest` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `error_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `error_detail` VARCHAR(1000) DEFAULT NULL,
            `copy_started_at` DATETIME(6) DEFAULT NULL,
            `copied_at` DATETIME(6) DEFAULT NULL,
            `verified_at` DATETIME(6) DEFAULT NULL,
            `cleanup_state` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'pending',
            `cleanup_row_count` BIGINT NOT NULL DEFAULT 0,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_data_migration_item` (`migration_id`, `table_name`),
            KEY `idx_tenant_data_migration_item_state` (`migration_id`, `state`, `copy_order`),
            CONSTRAINT `fk_tenant_data_migration_item_migration`
                FOREIGN KEY (`migration_id`) REFERENCES `sys_tenant_data_migration` (`id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_data_migration_item_state`
                CHECK (`state` IN ('pending', 'copying', 'copied', 'verifying', 'verified', 'failed')),
            CONSTRAINT `ck_tenant_data_migration_item_cleanup_state`
                CHECK (`cleanup_state` IN ('pending', 'cleaning', 'cleaned')),
            CONSTRAINT `ck_tenant_data_migration_item_copy_order`
                CHECK (`copy_order` > 0),
            CONSTRAINT `ck_tenant_data_migration_item_counts`
                CHECK ((`source_row_count` IS NULL OR `source_row_count` >= 0)
                    AND (`target_row_count` IS NULL OR `target_row_count` >= 0)
                    AND `cleanup_row_count` >= 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据迁移表级检查点'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_backup_point` (
            `id` BIGINT NOT NULL,
            `scope` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `tenant_id` VARCHAR(64) DEFAULT NULL,
            `target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `placement_generation` BIGINT DEFAULT NULL,
            `schema_fingerprint` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `provider_ref` VARCHAR(512) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `captured_at` DATETIME(6) NOT NULL,
            `checksum` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `validation_status` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `validation_detail` VARCHAR(1000) DEFAULT NULL,
            `retention_until` DATETIME(6) NOT NULL,
            `expires_at` DATETIME(6) DEFAULT NULL,
            `last_restore_drill_at` DATETIME(6) DEFAULT NULL,
            `created_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_data_backup_provider_ref` (`provider_ref`),
            KEY `idx_tenant_data_backup_tenant` (`tenant_id`, `captured_at`),
            KEY `idx_tenant_data_backup_target` (`target_key`, `scope`, `captured_at`),
            KEY `idx_tenant_data_backup_expiry` (`expires_at`, `validation_status`),
            CONSTRAINT `fk_tenant_data_backup_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON DELETE RESTRICT,
            CONSTRAINT `ck_tenant_data_backup_scope`
                CHECK ((`scope` = 'tenant' AND `tenant_id` IS NOT NULL AND `placement_generation` > 0)
                    OR (`scope` = 'shard' AND `tenant_id` IS NULL)),
            CONSTRAINT `ck_tenant_data_backup_validation`
                CHECK (`validation_status` IN ('pending', 'valid', 'invalid')),
            CONSTRAINT `ck_tenant_data_backup_retention`
                CHECK (`expires_at` IS NULL OR `expires_at` >= `retention_until`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据备份恢复点'"#,
    ]
}

pub(crate) fn service_account_table_statements() -> [&'static str; 6] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_service_account` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `code` VARCHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
            `name` VARCHAR(128) NOT NULL,
            `description` VARCHAR(500) DEFAULT NULL,
            `dept_id` BIGINT DEFAULT NULL,
            `status` CHAR(1) NOT NULL DEFAULT '1',
            `authorization_version` INT NOT NULL DEFAULT 1,
            `max_requests_per_minute` INT NOT NULL DEFAULT 60,
            `created_by` BIGINT NOT NULL,
            `del_flag` CHAR(1) NOT NULL DEFAULT '0',
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_account_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_service_account_code` (`tenant_id`, `code`),
            KEY `idx_service_account_list` (`tenant_id`, `del_flag`, `created_at`, `id`),
            KEY `idx_service_account_dept` (`tenant_id`, `dept_id`),
            KEY `fk_service_account_creator` (`tenant_id`, `created_by`),
            CONSTRAINT `fk_service_account_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_account_dept`
                FOREIGN KEY (`tenant_id`, `dept_id`) REFERENCES `sys_dept` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_account_creator`
                FOREIGN KEY (`tenant_id`, `created_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_service_account_status` CHECK (`status` IN ('0', '1')),
            CONSTRAINT `ck_service_account_del_flag` CHECK (`del_flag` IN ('0', '2')),
            CONSTRAINT `ck_service_account_rate` CHECK (`max_requests_per_minute` > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='服务账号'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_account_role` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `account_id` BIGINT NOT NULL,
            `role_id` BIGINT NOT NULL,
            PRIMARY KEY (`tenant_id`, `account_id`, `role_id`),
            KEY `idx_service_account_role_role` (`tenant_id`, `role_id`, `account_id`),
            CONSTRAINT `fk_service_account_role_account`
                FOREIGN KEY (`tenant_id`, `account_id`)
                REFERENCES `sys_service_account` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_service_account_role_role`
                FOREIGN KEY (`tenant_id`, `role_id`) REFERENCES `sys_role` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='服务账号角色关系'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_credential` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `account_id` BIGINT NOT NULL,
            `key_id` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `secret_mac` BINARY(32) NOT NULL,
            `pepper_version` INT NOT NULL,
            `label` VARCHAR(128) NOT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'active',
            `expires_at` DATETIME(6) NOT NULL,
            `last_used_at` DATETIME(6) DEFAULT NULL,
            `created_by` BIGINT NOT NULL,
            `revoked_at` DATETIME(6) DEFAULT NULL,
            `revoked_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            `idempotency_key_hash` BINARY(32) NOT NULL,
            `request_fingerprint` BINARY(32) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_credential_key_id` (`key_id`),
            UNIQUE KEY `uq_service_credential_idempotency` (`tenant_id`, `account_id`, `idempotency_key_hash`),
            KEY `idx_service_credential_active` (`tenant_id`, `account_id`, `status`, `expires_at`, `id`),
            KEY `idx_service_credential_expiry` (`status`, `expires_at`, `id`),
            KEY `fk_service_credential_creator` (`tenant_id`, `created_by`),
            KEY `fk_service_credential_revoker` (`tenant_id`, `revoked_by`),
            CONSTRAINT `fk_service_credential_account`
                FOREIGN KEY (`tenant_id`, `account_id`)
                REFERENCES `sys_service_account` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_service_credential_creator`
                FOREIGN KEY (`tenant_id`, `created_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_credential_revoker`
                FOREIGN KEY (`tenant_id`, `revoked_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_service_credential_status` CHECK (`status` IN ('active', 'revoked'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='服务账号 API Key 凭据'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_delegation` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `account_id` BIGINT NOT NULL,
            `user_id` BIGINT NOT NULL,
            `token_mac` BINARY(32) NOT NULL,
            `pepper_version` INT NOT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'active',
            `version` INT NOT NULL DEFAULT 1,
            `not_before` DATETIME(6) NOT NULL,
            `expires_at` DATETIME(6) NOT NULL,
            `reason` VARCHAR(500) NOT NULL,
            `created_by_user_id` BIGINT NOT NULL,
            `revoked_at` DATETIME(6) DEFAULT NULL,
            `revoked_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            `idempotency_key_hash` BINARY(32) NOT NULL,
            `request_fingerprint` BINARY(32) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_delegation_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_service_delegation_token_mac` (`token_mac`),
            UNIQUE KEY `uq_service_delegation_idempotency` (`tenant_id`, `user_id`, `idempotency_key_hash`),
            KEY `idx_service_delegation_account` (`tenant_id`, `account_id`, `status`, `expires_at`, `id`),
            KEY `idx_service_delegation_user` (`tenant_id`, `user_id`, `status`, `expires_at`, `id`),
            KEY `idx_service_delegation_expiry` (`status`, `expires_at`, `id`),
            KEY `fk_service_delegation_creator` (`tenant_id`, `created_by_user_id`),
            KEY `fk_service_delegation_revoker` (`tenant_id`, `revoked_by`),
            CONSTRAINT `fk_service_delegation_account`
                FOREIGN KEY (`tenant_id`, `account_id`)
                REFERENCES `sys_service_account` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_service_delegation_user`
                FOREIGN KEY (`tenant_id`, `user_id`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_delegation_creator`
                FOREIGN KEY (`tenant_id`, `created_by_user_id`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_delegation_revoker`
                FOREIGN KEY (`tenant_id`, `revoked_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_service_delegation_status` CHECK (`status` IN ('active', 'revoked')),
            CONSTRAINT `ck_service_delegation_window` CHECK (`not_before` < `expires_at`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户显式查询委托'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_delegation_capability` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `delegation_id` BIGINT NOT NULL,
            `capability_key` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            PRIMARY KEY (`tenant_id`, `delegation_id`, `capability_key`),
            KEY `idx_service_delegation_capability` (`tenant_id`, `capability_key`, `delegation_id`),
            CONSTRAINT `fk_service_delegation_capability_delegation`
                FOREIGN KEY (`tenant_id`, `delegation_id`)
                REFERENCES `sys_service_delegation` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='委托查询能力白名单'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_access_audit` (
            `id` BIGINT NOT NULL,
            `request_id` CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `tenant_id` VARCHAR(64) DEFAULT NULL,
            `account_id` BIGINT DEFAULT NULL,
            `credential_id` BIGINT DEFAULT NULL,
            `delegation_id` BIGINT DEFAULT NULL,
            `represented_user_id` BIGINT DEFAULT NULL,
            `operation_id` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `capability_key` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `required_permission` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `access_mode` VARCHAR(16) NOT NULL,
            `result` VARCHAR(16) NOT NULL,
            `reason_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `http_status` INT NOT NULL,
            `request_ip_digest` BINARY(32) DEFAULT NULL,
            `user_agent_digest` BINARY(32) DEFAULT NULL,
            `row_count` INT DEFAULT NULL,
            `response_bytes` BIGINT DEFAULT NULL,
            `tenant_epoch` INT DEFAULT NULL,
            `account_authorization_version` INT DEFAULT NULL,
            `user_authorization_version` INT DEFAULT NULL,
            `delegation_version` INT DEFAULT NULL,
            `started_at` DATETIME(6) NOT NULL,
            `completed_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_access_audit_request` (`request_id`),
            KEY `idx_service_access_audit_retention` (`completed_at`, `id`),
            KEY `idx_service_access_audit_tenant` (`tenant_id`, `completed_at`, `id`),
            KEY `idx_service_access_audit_account` (`tenant_id`, `account_id`, `completed_at`, `id`),
            KEY `idx_service_access_audit_user` (`tenant_id`, `represented_user_id`, `completed_at`, `id`),
            CONSTRAINT `ck_service_access_audit_mode` CHECK (`access_mode` IN ('direct', 'delegated', 'unknown')),
            CONSTRAINT `ck_service_access_audit_result` CHECK (`result` IN ('success', 'denied', 'error')),
            CONSTRAINT `ck_service_access_audit_counts` CHECK (
                (`row_count` IS NULL OR `row_count` >= 0)
                AND (`response_bytes` IS NULL OR `response_bytes` >= 0)
            )
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='Agent API 访问审计'"#,
    ]
}

pub(crate) const OUTBOX_EVENT_DDL: &str = r####"CREATE TABLE IF NOT EXISTS `sys_outbox_event` (
    `id` BIGINT NOT NULL,
    `tenant_id` VARCHAR(64) DEFAULT NULL,
    `event_type` VARCHAR(96) NOT NULL,
    `aggregate_type` VARCHAR(64) NOT NULL,
    `aggregate_id` VARCHAR(128) NOT NULL,
    `payload` JSON NOT NULL,
    `status` VARCHAR(16) NOT NULL DEFAULT 'pending',
    `available_at` DATETIME NOT NULL,
    `attempts` INT NOT NULL DEFAULT 0,
    `max_attempts` INT NOT NULL DEFAULT 5,
    `lease_owner` VARCHAR(128) DEFAULT NULL,
    `lease_until` DATETIME DEFAULT NULL,
    `dedupe_key` VARCHAR(191) DEFAULT NULL,
    `traceparent` VARCHAR(255) DEFAULT NULL,
    `tracestate` VARCHAR(512) DEFAULT NULL,
    `last_error` TEXT DEFAULT NULL,
    `published_at` DATETIME DEFAULT NULL,
    `created_at` DATETIME NOT NULL,
    `updated_at` DATETIME NOT NULL,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uq_outbox_event_dedupe` (`event_type`, `dedupe_key`),
    KEY `idx_outbox_event_claim` (`status`, `available_at`, `id`),
    KEY `idx_outbox_event_lease` (`status`, `lease_until`),
    KEY `idx_outbox_event_aggregate` (`aggregate_type`, `aggregate_id`, `created_at`),
    KEY `idx_outbox_event_retention` (`status`, `published_at`, `id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"####;

pub(crate) const EXPORT_JOB_DDL: &str = r####"CREATE TABLE IF NOT EXISTS `sys_export_job` (
    `id` BIGINT NOT NULL,
    `tenant_id` VARCHAR(64) NOT NULL,
    `requester_id` BIGINT NOT NULL,
    `resource` VARCHAR(64) NOT NULL,
    `background_job_id` BIGINT NOT NULL,
    `request_params` JSON NOT NULL,
    `request_version` INT NOT NULL,
    `permission_code` VARCHAR(128) NOT NULL,
    `authorization_fingerprint` CHAR(64) NOT NULL,
    `request_fingerprint` CHAR(64) NOT NULL,
    `active_request_fingerprint` CHAR(64) DEFAULT NULL,
    `snapshot_at` DATETIME(6) NOT NULL,
    `upper_id` BIGINT NOT NULL,
    `matched_rows` BIGINT NOT NULL,
    `exported_rows` BIGINT NOT NULL DEFAULT 0,
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
    UNIQUE KEY `uq_export_job_active_request` (`active_request_fingerprint`),
    KEY `idx_export_job_requester` (`tenant_id`, `requester_id`, `delete_pending_at`, `created_at`, `id`),
    KEY `idx_export_job_expiry` (`status`, `expires_at`),
    KEY `idx_export_job_history` (`status`, `completed_at`, `id`),
    KEY `idx_export_job_notification` (`tenant_id`, `requester_id`, `delete_pending_at`, `notification_read_at`, `status`, `completed_at`, `id`),
    KEY `idx_export_job_delete_pending` (`delete_pending_at`, `id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"####;

pub(crate) const RESOURCE_OWNERSHIP_DDL: &str = r####"CREATE TABLE IF NOT EXISTS `ryframe_resource_ownership` (
    `resource_kind` VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `scope_id` VARCHAR(48) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `marker` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (`resource_kind`),
    UNIQUE KEY `uq_resource_ownership_marker` (`marker`),
    UNIQUE KEY `uq_resource_ownership_scope` (`scope_id`, `resource_kind`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='物理数据库资源作用域所有权'"####;
