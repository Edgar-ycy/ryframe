-- Introduce tenant lifecycle data without rebuilding existing business tables.
CREATE TABLE IF NOT EXISTS `sys_tenant` (
    `id` BIGINT NOT NULL,
    `tenant_id` VARCHAR(64) NOT NULL,
    `name` VARCHAR(128) NOT NULL,
    `domain` VARCHAR(255) DEFAULT NULL,
    `status` CHAR(1) NOT NULL DEFAULT '1',
    `expire_at` DATETIME DEFAULT NULL,
    `max_users` INT NOT NULL DEFAULT 100,
    `max_roles` INT NOT NULL DEFAULT 20,
    `max_storage_mb` BIGINT NOT NULL DEFAULT 1024,
    `max_requests_per_min` INT NOT NULL DEFAULT 1000,
    `session_version` INT NOT NULL DEFAULT 1,
    `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_id` (`tenant_id`),
    UNIQUE KEY `uk_tenant_domain` (`domain`),
    KEY `idx_tenant_status` (`status`, `expire_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

INSERT IGNORE INTO `sys_tenant` (`id`, `tenant_id`, `name`, `status`)
VALUES (1, 'system', '系统租户', '1');
