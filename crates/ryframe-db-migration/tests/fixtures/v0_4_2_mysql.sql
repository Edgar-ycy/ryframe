-- FIXTURE PROVENANCE: v0.4.2 MySQL DDL with comments removed; fixed legacy data follows.
-- Source tag: v0.4.2:sql/ryframe_config.sql. Keep immutable as the v0.4 boundary.
CREATE TABLE IF NOT EXISTS `sys_tenant` (
    `id`                     BIGINT       NOT NULL,
    `tenant_id`              VARCHAR(64)  NOT NULL,
    `name`                   VARCHAR(128) NOT NULL,
    `domain`                 VARCHAR(255)          DEFAULT NULL,
    `status`                 CHAR(1)      NOT NULL DEFAULT '1',
    `expire_at`              DATETIME              DEFAULT NULL,
    `max_users`              INT          NOT NULL DEFAULT 100,
    `max_roles`              INT          NOT NULL DEFAULT 20,
    `max_storage_mb`         BIGINT       NOT NULL DEFAULT 1024,
    `max_requests_per_min`   INT          NOT NULL DEFAULT 1000,
    `session_version`        INT          NOT NULL DEFAULT 1,
    `created_at`             DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`             DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_id` (`tenant_id`),
    UNIQUE KEY `uk_tenant_domain` (`domain`),
    KEY `idx_tenant_status` (`status`, `expire_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_dept` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(64)  NOT NULL,
    `parent_id`   BIGINT                DEFAULT NULL,
    `ancestors`   VARCHAR(512) NOT NULL DEFAULT '',
    `sort`        INT          NOT NULL DEFAULT 0,
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_parent_id` (`parent_id`),
    CONSTRAINT `fk_sys_dept_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_dept_parent`
        FOREIGN KEY (`parent_id`) REFERENCES `sys_dept` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_user` (
    `id`             BIGINT       NOT NULL,
    `tenant_id`      VARCHAR(64)  NOT NULL DEFAULT 'system',
    `username`       VARCHAR(64)  NOT NULL,
    `password_hash`  VARCHAR(255) NOT NULL,
    `nickname`       VARCHAR(64)  NOT NULL,
    `email`          VARCHAR(128) NOT NULL DEFAULT '',
    `phone`          VARCHAR(32)  NOT NULL DEFAULT '',
    `avatar`         VARCHAR(255)          DEFAULT NULL,
    `status`         VARCHAR(32)  NOT NULL DEFAULT '1',
    `auth_version`   INT          NOT NULL DEFAULT 1,
    `dept_id`        BIGINT                DEFAULT NULL,
    `remark`         VARCHAR(512)          DEFAULT NULL,
    `login_ip`       VARCHAR(128)          DEFAULT NULL,
    `login_date`     DATETIME              DEFAULT NULL,
    `del_flag`       CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_username` (`tenant_id`, `username`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_dept_id` (`dept_id`),
    CONSTRAINT `fk_sys_user_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `password_reset_requests` (
    `id`             BIGINT       NOT NULL,
    `tenant_id`      VARCHAR(64)  NOT NULL DEFAULT 'system',
    `target_user_id` BIGINT       NOT NULL,
    `requested_by`   BIGINT       NOT NULL,
    `reason`         VARCHAR(512) NOT NULL,
    `token_hash`     VARCHAR(255) NOT NULL,
    `expires_at`     DATETIME     NOT NULL,
    `completed_at`   DATETIME              DEFAULT NULL,
    `request_ip`     VARCHAR(128)          DEFAULT NULL,
    `status`         VARCHAR(32)  NOT NULL DEFAULT 'pending',
    `created_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    KEY `idx_password_reset_tenant` (`tenant_id`),
    KEY `idx_target_user_id` (`target_user_id`),
    KEY `idx_requested_by` (`requested_by`),
    KEY `idx_status` (`status`),
    KEY `idx_expires_at` (`expires_at`),
    CONSTRAINT `fk_password_reset_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_password_reset_target_user`
        FOREIGN KEY (`target_user_id`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT `fk_password_reset_requested_by`
        FOREIGN KEY (`requested_by`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_role` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(64)  NOT NULL,
    `code`        VARCHAR(64)  NOT NULL,
    `is_super`    TINYINT(1)   NOT NULL DEFAULT 0,
    `data_scope`  CHAR(1)      NOT NULL DEFAULT '1',
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `sort`        INT          NOT NULL DEFAULT 0,
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_role_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_permission` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(64)  NOT NULL,
    `code`        VARCHAR(128) NOT NULL,
    `parent_id`   BIGINT                DEFAULT NULL,
    `perm_type`   VARCHAR(16)  NOT NULL,
    `icon`        VARCHAR(64)           DEFAULT NULL,
    `sort`        INT          NOT NULL DEFAULT 0,
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_parent_id` (`parent_id`),
    KEY `idx_perm_code` (`code`),
    CONSTRAINT `fk_sys_permission_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_permission_parent`
        FOREIGN KEY (`parent_id`) REFERENCES `sys_permission` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_menu` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(64)  NOT NULL,
    `parent_id`   BIGINT                DEFAULT NULL,
    `menu_type`   CHAR(1)      NOT NULL DEFAULT '',
    `perm_id`     BIGINT                DEFAULT NULL,
    `route_key`   VARCHAR(100)          DEFAULT NULL,
    `icon`        VARCHAR(128)          DEFAULT NULL,
    `sort`        INT          NOT NULL DEFAULT 0,
    `visible`     TINYINT(1)   NOT NULL DEFAULT 1,
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_parent_id` (`parent_id`),
    KEY `idx_perm_id` (`perm_id`),
    KEY `idx_menu_tenant_perm` (`tenant_id`, `perm_id`),
    KEY `idx_menu_tenant_route` (`tenant_id`, `route_key`),
    CONSTRAINT `fk_sys_menu_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_menu_permission`
        FOREIGN KEY (`perm_id`) REFERENCES `sys_permission` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_post` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(64)  NOT NULL,
    `code`        VARCHAR(64)  NOT NULL,
    `sort`        INT          NOT NULL DEFAULT 0,
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_post_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_config` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(128) NOT NULL,
    `key`         VARCHAR(128) NOT NULL,
    `value`       VARCHAR(512) NOT NULL,
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_key` (`tenant_id`, `key`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_config_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_dict_type` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `name`        VARCHAR(64)  NOT NULL,
    `code`        VARCHAR(64)  NOT NULL,
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_dict_type_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_dict_data` (
    `id`          BIGINT       NOT NULL,
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system',
    `type_code`   VARCHAR(64)  NOT NULL,
    `label`       VARCHAR(64)  NOT NULL,
    `value`       VARCHAR(64)  NOT NULL,
    `sort`        INT          NOT NULL DEFAULT 0,
    `status`      CHAR(1)      NOT NULL DEFAULT '1',
    `css_class`   VARCHAR(64)           DEFAULT NULL,
    `remark`      VARCHAR(512)          DEFAULT NULL,
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_type_code` (`type_code`),
    KEY `idx_dict_data_tenant_type` (`tenant_id`, `type_code`),
    CONSTRAINT `fk_sys_dict_data_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_dict_data_type`
        FOREIGN KEY (`tenant_id`, `type_code`) REFERENCES `sys_dict_type` (`tenant_id`, `code`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_notice` (
    `id`           BIGINT       NOT NULL,
    `tenant_id`    VARCHAR(64)  NOT NULL DEFAULT 'system',
    `title`        VARCHAR(128) NOT NULL,
    `content`      TEXT         NOT NULL,
    `type`         VARCHAR(16)           DEFAULT NULL,
    `status`       CHAR(1)      NOT NULL DEFAULT '1',
    `created_by`   BIGINT                DEFAULT NULL,
    `del_flag`     CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_created_by` (`created_by`),
    CONSTRAINT `fk_sys_notice_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_notice_created_by`
        FOREIGN KEY (`created_by`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_oper_log` (
    `id`              BIGINT       NOT NULL,
    `tenant_id`       VARCHAR(64)  NOT NULL DEFAULT 'system',
    `title`           VARCHAR(64)  NOT NULL,
    `business_type`   VARCHAR(32)  NOT NULL,
    `method`          VARCHAR(255) NOT NULL,
    `request_method`  VARCHAR(16)  NOT NULL,
    `oper_name`       VARCHAR(64)  NOT NULL,
    `oper_url`        VARCHAR(255) NOT NULL,
    `oper_ip`         VARCHAR(128) NOT NULL,
    `oper_location`   VARCHAR(128)          DEFAULT NULL,
    `oper_param`      TEXT                  DEFAULT NULL,
    `json_result`     TEXT                  DEFAULT NULL,
    `status`          CHAR(1)      NOT NULL DEFAULT '1',
    `error_msg`       TEXT                  DEFAULT NULL,
    `oper_time`       DATETIME     NOT NULL,
    `cost_time`       BIGINT       NOT NULL DEFAULT 0,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_oper_time` (`oper_time`),
    KEY `idx_business_type` (`business_type`),
    CONSTRAINT `fk_sys_oper_log_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_login_info` (
    `id`              BIGINT       NOT NULL,
    `tenant_id`       VARCHAR(64)  NOT NULL DEFAULT 'system',
    `user_name`       VARCHAR(64)  NOT NULL,
    `ipaddr`          VARCHAR(128) NOT NULL,
    `login_location`  VARCHAR(128)          DEFAULT NULL,
    `browser`         VARCHAR(64)           DEFAULT NULL,
    `os`              VARCHAR(64)           DEFAULT NULL,
    `status`          CHAR(1)      NOT NULL DEFAULT '1',
    `msg`             VARCHAR(255)          DEFAULT NULL,
    `login_time`      DATETIME     NOT NULL,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_login_time` (`login_time`),
    KEY `idx_user_name` (`user_name`),
    CONSTRAINT `fk_sys_login_info_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_user_role` (
    `tenant_id` VARCHAR(64) NOT NULL DEFAULT 'system',
    `user_id`  BIGINT NOT NULL,
    `role_id`  BIGINT NOT NULL,
    PRIMARY KEY (`tenant_id`, `user_id`, `role_id`),
    KEY `idx_user_id` (`user_id`),
    KEY `idx_role_id` (`role_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_role_permission` (
    `tenant_id` VARCHAR(64) NOT NULL DEFAULT 'system',
    `role_id`  BIGINT NOT NULL,
    `perm_id`  BIGINT NOT NULL,
    PRIMARY KEY (`tenant_id`, `role_id`, `perm_id`),
    KEY `idx_role_id` (`role_id`),
    KEY `idx_perm_id` (`perm_id`),
    CONSTRAINT `fk_sys_role_permission_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_role_permission_role`
        FOREIGN KEY (`role_id`) REFERENCES `sys_role` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT `fk_sys_role_permission_permission`
        FOREIGN KEY (`perm_id`) REFERENCES `sys_permission` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_role_dept` (
    `tenant_id` VARCHAR(64) NOT NULL DEFAULT 'system',
    `role_id`  BIGINT NOT NULL,
    `dept_id`  BIGINT NOT NULL,
    PRIMARY KEY (`tenant_id`, `role_id`, `dept_id`),
    KEY `idx_role_id` (`role_id`),
    KEY `idx_dept_id` (`dept_id`),
    CONSTRAINT `fk_sys_role_dept_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_role_dept_role`
        FOREIGN KEY (`role_id`) REFERENCES `sys_role` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT `fk_sys_role_dept_dept`
        FOREIGN KEY (`dept_id`) REFERENCES `sys_dept` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE IF NOT EXISTS `sys_file` (
    `id`            BIGINT       NOT NULL,
    `tenant_id`     VARCHAR(64)  NOT NULL DEFAULT 'system',
    `original_name` VARCHAR(255) NOT NULL,
    `storage_name`  VARCHAR(255) NOT NULL,
    `storage_path`  VARCHAR(500) NOT NULL,
    `bucket`        VARCHAR(100) NOT NULL DEFAULT 'uploads',
    `file_url`      VARCHAR(1000)NOT NULL,
    `file_size`     BIGINT       NOT NULL DEFAULT 0,
    `content_type`  VARCHAR(100) NOT NULL,
    `file_md5`      CHAR(32)              DEFAULT NULL,
    `upload_by`     VARCHAR(64)           DEFAULT NULL,
    `del_flag`      CHAR(1)      NOT NULL DEFAULT '0',
    `created_at`    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    `updated_at`    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_bucket` (`bucket`),
    KEY `idx_upload_by` (`upload_by`),
    KEY `idx_del_flag` (`del_flag`),
    CONSTRAINT `fk_sys_file_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;


INSERT INTO `sys_tenant` (`id`, `tenant_id`, `name`, `status`)
VALUES (9000, 'legacy-fixture', 'Legacy fixture tenant', '1');

INSERT INTO `sys_config` (`id`, `tenant_id`, `name`, `key`, `value`, `remark`)
VALUES (9001, 'legacy-fixture', 'Legacy custom config', 'legacy.custom', 'keep-me', 'v0.4 fixture');
