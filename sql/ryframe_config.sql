-- ============================================================
-- RyFrame 完整数据库初始化脚本
-- 目标数据库: ryframe_config (MySQL 8.0+)
-- 创建时间: 2026-05-22
-- 说明: 包含所有表结构创建 + 默认初始化数据
--       主键采用 BIGINT（雪花算法ID），状态字段采用 CHAR(1)
-- ============================================================

SET NAMES utf8mb4;
SET FOREIGN_KEY_CHECKS = 0;

-- ============================================================
-- 清理所有已有表 (先删关联表再删主表，避免外键依赖问题)
-- ============================================================
DROP TABLE IF EXISTS `sys_user_role`;
DROP TABLE IF EXISTS `sys_role_permission`;
DROP TABLE IF EXISTS `sys_role_dept`;
DROP TABLE IF EXISTS `sys_tenant`;
DROP TABLE IF EXISTS `password_reset_requests`;
DROP TABLE IF EXISTS `sys_dept`;
DROP TABLE IF EXISTS `sys_user`;
DROP TABLE IF EXISTS `sys_role`;
DROP TABLE IF EXISTS `sys_permission`;
DROP TABLE IF EXISTS `sys_menu`;
DROP TABLE IF EXISTS `sys_post`;
DROP TABLE IF EXISTS `sys_config`;
DROP TABLE IF EXISTS `sys_dict_type`;
DROP TABLE IF EXISTS `sys_dict_data`;
DROP TABLE IF EXISTS `sys_notice`;
DROP TABLE IF EXISTS `sys_oper_log`;
DROP TABLE IF EXISTS `sys_login_info`;
DROP TABLE IF EXISTS `sys_file`;

-- ============================================================
-- 1. 租户表 (sys_tenant)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_tenant` (
    `id`                     BIGINT       NOT NULL COMMENT '租户ID',
    `tenant_id`              VARCHAR(64)  NOT NULL COMMENT '租户标识',
    `name`                   VARCHAR(128) NOT NULL COMMENT '租户名称',
    `domain`                 VARCHAR(255)          DEFAULT NULL COMMENT '绑定域名',
    `status`                 CHAR(1)      NOT NULL DEFAULT '1' COMMENT '状态: 0停用 1正常',
    `expire_at`              DATETIME              DEFAULT NULL COMMENT '到期时间',
    `max_users`              INT          NOT NULL DEFAULT 100 COMMENT '最大用户数',
    `max_roles`              INT          NOT NULL DEFAULT 20 COMMENT '最大角色数',
    `max_storage_mb`         BIGINT       NOT NULL DEFAULT 1024 COMMENT '最大存储容量(MB)',
    `max_requests_per_min`   INT          NOT NULL DEFAULT 1000 COMMENT '每分钟最大请求数',
    `session_version`        INT          NOT NULL DEFAULT 1 COMMENT '租户会话版本',
    `created_at`             DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    `updated_at`             DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_id` (`tenant_id`),
    UNIQUE KEY `uk_tenant_domain` (`domain`),
    KEY `idx_tenant_status` (`status`, `expire_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户表';

INSERT INTO `sys_tenant` (`id`, `tenant_id`, `name`, `status`)
VALUES (1, 'system', '系统租户', '1');

-- ============================================================
-- 2. 部门表 (sys_dept)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_dept` (
    `id`          BIGINT       NOT NULL                    COMMENT '部门ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '部门名称',
    `parent_id`   BIGINT                DEFAULT NULL       COMMENT '父部门ID',
    `ancestors`   VARCHAR(512) NOT NULL DEFAULT ''         COMMENT '祖级列表(如: 0,1,2)',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_parent_id` (`parent_id`),
    CONSTRAINT `fk_sys_dept_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_dept_parent`
        FOREIGN KEY (`parent_id`) REFERENCES `sys_dept` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='部门表';

-- ============================================================
-- 2. 用户表 (sys_user)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_user` (
    `id`             BIGINT       NOT NULL                  COMMENT '用户ID',
    `tenant_id`      VARCHAR(64)  NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `username`       VARCHAR(64)  NOT NULL                  COMMENT '用户名',
    `password_hash`  VARCHAR(255) NOT NULL                  COMMENT '密码哈希(argon2)',
    `nickname`       VARCHAR(64)  NOT NULL                  COMMENT '昵称',
    `email`          VARCHAR(128) NOT NULL DEFAULT ''       COMMENT '邮箱',
    `phone`          VARCHAR(32)  NOT NULL DEFAULT ''       COMMENT '手机号',
    `avatar`         VARCHAR(255)          DEFAULT NULL     COMMENT '头像URL',
    `status`         VARCHAR(32)  NOT NULL DEFAULT '1'      COMMENT '状态: 0停用 1正常 2锁定 pending_activation待激活 must_reset_password需改密',
    `auth_version`   INT          NOT NULL DEFAULT 1        COMMENT '用户认证版本，权限变更时递增',
    `dept_id`        BIGINT                DEFAULT NULL     COMMENT '部门ID(软删除场景由代码校验合法性)',
    `remark`         VARCHAR(512)          DEFAULT NULL     COMMENT '备注',
    `login_ip`       VARCHAR(128)          DEFAULT NULL     COMMENT '最后登录IP',
    `login_date`     DATETIME              DEFAULT NULL     COMMENT '最后登录时间',
    `del_flag`       CHAR(1)      NOT NULL DEFAULT '0'      COMMENT '删除标志: 0正常 2删除',
    `created_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_username` (`tenant_id`, `username`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_dept_id` (`dept_id`),
    CONSTRAINT `fk_sys_user_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户表';

-- ============================================================
-- 2.1. 密码重置请求表 (password_reset_requests)
-- ============================================================
CREATE TABLE IF NOT EXISTS `password_reset_requests` (
    `id`             BIGINT       NOT NULL                  COMMENT '重置请求ID',
    `tenant_id`      VARCHAR(64)  NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `target_user_id` BIGINT       NOT NULL                  COMMENT '目标用户ID',
    `requested_by`   BIGINT       NOT NULL                  COMMENT '发起管理员ID',
    `reason`         VARCHAR(512) NOT NULL                  COMMENT '发起原因',
    `token_hash`     VARCHAR(255) NOT NULL                  COMMENT '重置令牌哈希',
    `expires_at`     DATETIME     NOT NULL                  COMMENT '过期时间',
    `completed_at`   DATETIME              DEFAULT NULL     COMMENT '完成时间',
    `request_ip`     VARCHAR(128)          DEFAULT NULL     COMMENT '发起IP',
    `status`         VARCHAR(32)  NOT NULL DEFAULT 'pending' COMMENT '状态: pending/completed/expired',
    `created_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    `updated_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='密码重置请求表';

-- ============================================================
-- 3. 角色表 (sys_role)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_role` (
    `id`          BIGINT       NOT NULL                    COMMENT '角色ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '角色名称',
    `code`        VARCHAR(64)  NOT NULL                    COMMENT '角色编码',
    `data_scope`  CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '数据范围: 1全部 2自定义 3本部门 4本部门及以下 5仅本人',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_role_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色表';

-- ============================================================
-- 4. 权限表 (sys_permission)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_permission` (
    `id`          BIGINT       NOT NULL                    COMMENT '权限ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '权限名称',
    `code`        VARCHAR(128) NOT NULL                    COMMENT '权限编码',
    `parent_id`   BIGINT                DEFAULT NULL       COMMENT '父权限ID(树形)',
    `perm_type`   VARCHAR(16)  NOT NULL                    COMMENT '权限类型: menu/api',
    `icon`        VARCHAR(64)           DEFAULT NULL       COMMENT '图标',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='权限表';

-- ============================================================
-- 5. 菜单表 (sys_menu)
-- 统一管理目录(M)、菜单(C)、按钮(F)，前端通过 menu_type 区分
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_menu` (
    `id`          BIGINT       NOT NULL                    COMMENT '菜单ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '菜单名称',
    `parent_id`   BIGINT                DEFAULT NULL       COMMENT '父菜单ID(由代码校验父子关系合法性)',
    `menu_type`   CHAR(1)      NOT NULL DEFAULT ''         COMMENT '菜单类型: M目录 C菜单 F按钮',
    `perm_id`     BIGINT                DEFAULT NULL       COMMENT '关联sys_permission权限ID',
    `route_key`   VARCHAR(100)          DEFAULT NULL       COMMENT '前端稳定页面标识',
    `icon`        VARCHAR(128)          DEFAULT NULL       COMMENT '图标',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `visible`     TINYINT(1)   NOT NULL DEFAULT 1          COMMENT '是否可见: 0隐藏 1显示',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='菜单表(含目录/菜单/按钮)';

-- ============================================================
-- 6. 岗位表 (sys_post)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_post` (
    `id`          BIGINT       NOT NULL                    COMMENT '岗位ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '岗位名称',
    `code`        VARCHAR(64)  NOT NULL                    COMMENT '岗位编码',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_post_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='岗位表';

-- ============================================================
-- 7. 参数配置表 (sys_config)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_config` (
    `id`          BIGINT       NOT NULL                    COMMENT '配置ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(128) NOT NULL                    COMMENT '配置名称',
    `key`         VARCHAR(128) NOT NULL                    COMMENT '配置键',
    `value`       VARCHAR(512) NOT NULL                    COMMENT '配置值',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_key` (`tenant_id`, `key`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_config_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='参数配置表';

-- ============================================================
-- 8. 字典类型表 (sys_dict_type)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_dict_type` (
    `id`          BIGINT       NOT NULL                    COMMENT '字典类型ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '字典名称',
    `code`        VARCHAR(64)  NOT NULL                    COMMENT '字典类型编码',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_tenant_code` (`tenant_id`, `code`),
    KEY `idx_tenant_id` (`tenant_id`),
    CONSTRAINT `fk_sys_dict_type_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='字典类型表';

-- ============================================================
-- 9. 字典数据表 (sys_dict_data)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_dict_data` (
    `id`          BIGINT       NOT NULL                    COMMENT '字典数据ID',
    `tenant_id`   VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `type_code`   VARCHAR(64)  NOT NULL                    COMMENT '所属字典类型编码',
    `label`       VARCHAR(64)  NOT NULL                    COMMENT '字典标签',
    `value`       VARCHAR(64)  NOT NULL                    COMMENT '字典值',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `css_class`   VARCHAR(64)           DEFAULT NULL       COMMENT 'CSS样式',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='字典数据表';

-- ============================================================
-- 10. 通知公告表 (sys_notice)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_notice` (
    `id`           BIGINT       NOT NULL                   COMMENT '公告ID',
    `tenant_id`    VARCHAR(64)  NOT NULL DEFAULT 'system'  COMMENT '租户ID',
    `title`        VARCHAR(128) NOT NULL                   COMMENT '公告标题',
    `content`      TEXT         NOT NULL                   COMMENT '公告内容',
    `type`         VARCHAR(16)           DEFAULT NULL      COMMENT '公告类型: notice/announcement',
    `status`       CHAR(1)      NOT NULL DEFAULT '1'       COMMENT '状态: 0草稿 1已发布 2已关闭',
    `created_by`   BIGINT                DEFAULT NULL      COMMENT '创建人ID',
    `del_flag`     CHAR(1)      NOT NULL DEFAULT '0'       COMMENT '删除标志: 0正常 2删除',
    `created_at`   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP   COMMENT '创建时间',
    `updated_at`   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_created_by` (`created_by`),
    CONSTRAINT `fk_sys_notice_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_notice_created_by`
        FOREIGN KEY (`created_by`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='通知公告表';

-- ============================================================
-- 11. 操作日志表 (sys_oper_log)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_oper_log` (
    `id`              BIGINT       NOT NULL                COMMENT '日志ID',
    `tenant_id`       VARCHAR(64)  NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `title`           VARCHAR(64)  NOT NULL                COMMENT '模块标题',
    `business_type`   VARCHAR(32)  NOT NULL                COMMENT '业务类型(INSERT/UPDATE/DELETE等)',
    `method`          VARCHAR(255) NOT NULL                COMMENT '操作方法(类名.方法名)',
    `request_method`  VARCHAR(16)  NOT NULL                COMMENT '请求方式(GET/POST/PUT/DELETE)',
    `oper_name`       VARCHAR(64)  NOT NULL                COMMENT '操作人员',
    `oper_url`        VARCHAR(255) NOT NULL                COMMENT '请求URL',
    `oper_ip`         VARCHAR(128) NOT NULL                COMMENT '操作IP',
    `oper_location`   VARCHAR(128)          DEFAULT NULL   COMMENT '操作地点',
    `oper_param`      TEXT                  DEFAULT NULL   COMMENT '请求参数(JSON)',
    `json_result`     TEXT                  DEFAULT NULL   COMMENT '返回结果(JSON)',
    `status`          CHAR(1)      NOT NULL DEFAULT '1'    COMMENT '操作状态: 0失败 1成功',
    `error_msg`       TEXT                  DEFAULT NULL   COMMENT '错误信息',
    `oper_time`       DATETIME     NOT NULL                COMMENT '操作时间',
    `cost_time`       BIGINT       NOT NULL DEFAULT 0      COMMENT '耗时(毫秒)',
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_oper_time` (`oper_time`),
    KEY `idx_business_type` (`business_type`),
    CONSTRAINT `fk_sys_oper_log_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='操作日志表';

-- ============================================================
-- 12. 登录信息表 (sys_login_info)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_login_info` (
    `id`              BIGINT       NOT NULL                COMMENT '日志ID',
    `tenant_id`       VARCHAR(64)  NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `user_name`       VARCHAR(64)  NOT NULL                COMMENT '用户名',
    `ipaddr`          VARCHAR(128) NOT NULL                COMMENT '登录IP',
    `login_location`  VARCHAR(128)          DEFAULT NULL   COMMENT '登录地点',
    `browser`         VARCHAR(64)           DEFAULT NULL   COMMENT '浏览器',
    `os`              VARCHAR(64)           DEFAULT NULL   COMMENT '操作系统',
    `status`          CHAR(1)      NOT NULL DEFAULT '1'    COMMENT '登录状态: 0失败 1成功',
    `msg`             VARCHAR(255)          DEFAULT NULL   COMMENT '提示信息',
    `login_time`      DATETIME     NOT NULL                COMMENT '登录时间',
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_login_time` (`login_time`),
    KEY `idx_user_name` (`user_name`),
    CONSTRAINT `fk_sys_login_info_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='登录信息表';

-- ============================================================
-- 13. 用户-角色关联表 (user_role)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_user_role` (
    `tenant_id` VARCHAR(64) NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `user_id`  BIGINT NOT NULL  COMMENT '用户ID',
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    PRIMARY KEY (`tenant_id`, `user_id`, `role_id`),
    KEY `idx_user_id` (`user_id`),
    KEY `idx_role_id` (`role_id`),
    CONSTRAINT `fk_sys_user_role_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    CONSTRAINT `fk_sys_user_role_user`
        FOREIGN KEY (`user_id`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    CONSTRAINT `fk_sys_user_role_role`
        FOREIGN KEY (`role_id`) REFERENCES `sys_role` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户-角色关联表';

-- ============================================================
-- 14. 角色-权限关联表 (role_permission)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_role_permission` (
    `tenant_id` VARCHAR(64) NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    `perm_id`  BIGINT NOT NULL  COMMENT '权限ID',
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色-权限关联表';

-- 16. 角色-部门关联表 (sys_role_dept) - 自定义数据权限
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_role_dept` (
    `tenant_id` VARCHAR(64) NOT NULL DEFAULT 'system' COMMENT '租户ID',
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    `dept_id`  BIGINT NOT NULL  COMMENT '部门ID',
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色-部门关联表(自定义数据权限)';

-- ============================================================
-- 17. 文件元数据表 (sys_file)
-- 存储所有上传文件的元信息，仅存在于主数据库
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_file` (
    `id`            BIGINT       NOT NULL                    COMMENT '文件ID',
    `tenant_id`     VARCHAR(64)  NOT NULL DEFAULT 'system'   COMMENT '租户ID',
    `original_name` VARCHAR(255) NOT NULL                    COMMENT '原始文件名',
    `storage_name`  VARCHAR(255) NOT NULL                    COMMENT '存储文件名(UUID)',
    `storage_path`  VARCHAR(500) NOT NULL                    COMMENT '对象存储 key',
    `bucket`        VARCHAR(100) NOT NULL DEFAULT 'uploads'  COMMENT '存储桶',
    `file_url`      VARCHAR(1000)NOT NULL                    COMMENT '相对路径(bucket/date/uuid.ext)',
    `file_size`     BIGINT       NOT NULL DEFAULT 0          COMMENT '字节数',
    `content_type`  VARCHAR(100) NOT NULL                    COMMENT 'MIME类型',
    `file_md5`      CHAR(32)              DEFAULT NULL       COMMENT 'MD5去重校验',
    `upload_by`     VARCHAR(64)           DEFAULT NULL       COMMENT '上传者',
    `del_flag`      CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    KEY `idx_tenant_id` (`tenant_id`),
    KEY `idx_bucket` (`bucket`),
    KEY `idx_upload_by` (`upload_by`),
    KEY `idx_del_flag` (`del_flag`),
    CONSTRAINT `fk_sys_file_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='文件元数据表';

-- ============================================================
-- =================== 默认初始化数据 =========================
-- ============================================================
-- ID 规划:
--   sys_dept:       1~100
--   sys_role:       1~100
--   sys_user:       1~100
--   sys_permission: 1~100
--   sys_menu:       1~1100
--   sys_post:       1~100
--   sys_config:     1~100
--   sys_dict_type:  1~100
--   sys_dict_data:  1~100

-- -----------------------------------------------------------
-- 默认部门 (sys_dept)
-- -----------------------------------------------------------
INSERT INTO `sys_dept` (`id`, `name`, `parent_id`, `ancestors`, `sort`, `status`) VALUES
    (1, 'RyFrame 科技',  NULL, '0',        1, '1'),
    (2, '研发部',         1,    '0,1',     1, '1'),
    (3, '产品部',         1,    '0,1',     2, '1'),
    (4, '运维部',         1,    '0,1',     3, '1'),
    (5, '后端组',         2,    '0,1,2',   1, '1'),
    (6, '前端组',         2,    '0,1,2',   2, '1');

-- -----------------------------------------------------------
-- 默认角色 (sys_role)
-- -----------------------------------------------------------
INSERT INTO `sys_role` (`id`, `name`, `code`, `data_scope`, `status`, `sort`, `remark`) VALUES
    (1, '超级管理员', 'admin',  '1', '1', 1, '超级管理员，拥有所有权限'),
    (2, '普通用户',   'common', '5', '1', 2, '普通用户，拥有基础权限');

-- -----------------------------------------------------------
-- 默认用户 (sys_user)
-- 默认密码: 123456 (argon2id 哈希)
-- -----------------------------------------------------------
INSERT INTO `sys_user` (`id`, `username`, `password_hash`, `nickname`, `email`, `phone`, `status`, `dept_id`) VALUES
    (1, 'admin',
        '$argon2id$v=19$m=65536,t=3,p=4$1tkQju6ICTl7Wtqcp0LZ5g$tMXEHFJkwViNukdGQ+QEa7EklHS7M+dN33e5G5g9gqY',
        '超级管理员', 'admin@ryframe.com', '13800000000', '1', 1),
    (2, 'user',
        '$argon2id$v=19$m=65536,t=3,p=4$1tkQju6ICTl7Wtqcp0LZ5g$tMXEHFJkwViNukdGQ+QEa7EklHS7M+dN33e5G5g9gqY',
        '普通用户', 'user@ryframe.com', '13800000001', '1', 5);

-- -----------------------------------------------------------
-- 默认权限 (sys_permission)
-- -----------------------------------------------------------
INSERT INTO `sys_permission` (`id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`) VALUES
    -- 系统管理
    (1, '系统管理', 'system', NULL, 'menu', 'Setting', 1, '1'),
    (2, '用户管理', 'system:user', 1, 'menu', 'User', 1, '1'),
    (3, '角色管理', 'system:role', 1, 'menu', 'UserFilled', 2, '1'),
    (4, '菜单管理', 'system:menu', 1, 'menu', 'Menu', 3, '1'),
    (5, '部门管理', 'system:dept', 1, 'menu', 'Grid', 4, '1'),
    (6, '岗位管理', 'system:post', 1, 'menu', 'Management', 5, '1'),
    (76, '租户管理', 'tenant:manage', NULL, 'menu', 'OfficeBuilding', 12, '1'),
    (77, '租户查询', 'tenant:list', 76, 'api', NULL, 1, '1'),
    (78, '租户新增', 'tenant:add', 76, 'api', NULL, 2, '1'),
    (79, '租户修改', 'tenant:edit', 76, 'api', NULL, 3, '1'),
    (80, '租户状态', 'tenant:status', 76, 'api', NULL, 4, '1'),
    -- 用户管理接口权限
    (7, '用户查询', 'system:user:list', 2, 'api', NULL, 1, '1'),
    (8, '用户新增', 'system:user:add', 2, 'api', NULL, 2, '1'),
    (9, '用户修改', 'system:user:edit', 2, 'api', NULL, 3, '1'),
    (10, '用户删除', 'system:user:remove', 2, 'api', NULL, 4, '1'),
    (11, '用户导出', 'system:user:export', 2, 'api', NULL, 5, '1'),
    (12, '全部权限', '*:*:*', NULL, 'api', NULL, 0, '1'),
    -- 系统监控
    (13, '系统监控', 'monitor', NULL, 'menu', 'Monitor', 2, '1'),
    (14, '在线用户', 'monitor:online', 13, 'menu', 'Connection', 1, '1'),
    (15, '服务器监控', 'monitor:server', 13, 'menu', 'DataAnalysis', 2, '1'),
    (16, '缓存监控', 'monitor:cache', 13, 'menu', 'Coin', 3, '1'),
    (17, '连接池监控', 'monitor:db-pool', 13, 'menu', 'Connection', 4, '1'),
    -- 日志管理
    (18, '操作日志', 'system:operlog', 1, 'menu', 'Document', 6, '1'),
    (19, '登录日志', 'system:logininfor', 1, 'menu', 'Notebook', 7, '1'),
    -- 字典管理
    (20, '字典管理', 'system:dict', 1, 'menu', 'Collection', 9, '1'),
    -- 参数设置
    (21, '参数设置', 'system:config', 1, 'menu', 'EditPen', 10, '1'),
    -- 通知公告
    (22, '通知公告', 'system:notice', 1, 'menu', 'Bell', 11, '1'),
    -- 代码生成
    (23, '代码生成', 'tools:gen', NULL, 'menu', 'MagicStick', 3, '1'),
    -- 角色管理接口权限
    (24, '角色查询', 'system:role:list', 3, 'api', NULL, 1, '1'),
    (25, '角色新增', 'system:role:add', 3, 'api', NULL, 2, '1'),
    (26, '角色修改', 'system:role:edit', 3, 'api', NULL, 3, '1'),
    (27, '角色删除', 'system:role:remove', 3, 'api', NULL, 4, '1'),
    (28, '角色导出', 'system:role:export', 3, 'api', NULL, 5, '1'),
    -- 菜单管理接口权限
    (29, '菜单查询', 'system:menu:list', 4, 'api', NULL, 1, '1'),
    (30, '菜单新增', 'system:menu:add', 4, 'api', NULL, 2, '1'),
    (31, '菜单修改', 'system:menu:edit', 4, 'api', NULL, 3, '1'),
    (32, '菜单删除', 'system:menu:remove', 4, 'api', NULL, 4, '1'),
    -- 权限管理接口权限
    (33, '权限查询', 'system:perm:list', 4, 'api', NULL, 5, '1'),
    (72, '权限新增', 'system:perm:add', 4, 'api', NULL, 6, '1'),
    (73, '权限修改', 'system:perm:edit', 4, 'api', NULL, 7, '1'),
    (74, '权限删除', 'system:perm:remove', 4, 'api', NULL, 8, '1'),
    (75, '权限同步', 'system:perm:sync', 4, 'api', NULL, 9, '1'),
    -- 部门管理接口权限
    (34, '部门查询', 'system:dept:list', 5, 'api', NULL, 1, '1'),
    (35, '部门新增', 'system:dept:add', 5, 'api', NULL, 2, '1'),
    (36, '部门修改', 'system:dept:edit', 5, 'api', NULL, 3, '1'),
    (37, '部门删除', 'system:dept:remove', 5, 'api', NULL, 4, '1'),
    -- 岗位管理接口权限
    (38, '岗位查询', 'system:post:list', 6, 'api', NULL, 1, '1'),
    (39, '岗位新增', 'system:post:add', 6, 'api', NULL, 2, '1'),
    (40, '岗位修改', 'system:post:edit', 6, 'api', NULL, 3, '1'),
    (41, '岗位删除', 'system:post:remove', 6, 'api', NULL, 4, '1'),
    (42, '岗位导出', 'system:post:export', 6, 'api', NULL, 5, '1'),
    -- 参数配置接口权限
    (43, '参数查询', 'system:config:list', 21, 'api', NULL, 1, '1'),
    (44, '参数新增', 'system:config:add', 21, 'api', NULL, 2, '1'),
    (45, '参数修改', 'system:config:edit', 21, 'api', NULL, 3, '1'),
    (46, '参数删除', 'system:config:remove', 21, 'api', NULL, 4, '1'),
    (47, '参数导出', 'system:config:export', 21, 'api', NULL, 5, '1'),
    -- 字典管理接口权限
    (48, '字典查询', 'system:dict:list', 20, 'api', NULL, 1, '1'),
    (49, '字典新增', 'system:dict:add', 20, 'api', NULL, 2, '1'),
    (50, '字典修改', 'system:dict:edit', 20, 'api', NULL, 3, '1'),
    (51, '字典删除', 'system:dict:remove', 20, 'api', NULL, 4, '1'),
    (52, '字典导出', 'system:dict:export', 20, 'api', NULL, 5, '1'),
    -- 通知公告接口权限
    (53, '通知查询', 'system:notice:list', 22, 'api', NULL, 1, '1'),
    (54, '通知新增', 'system:notice:add', 22, 'api', NULL, 2, '1'),
    (55, '通知修改', 'system:notice:edit', 22, 'api', NULL, 3, '1'),
    (56, '通知删除', 'system:notice:remove', 22, 'api', NULL, 4, '1'),
    -- 日志接口权限
    (57, '操作日志查询', 'system:operlog:list', 18, 'api', NULL, 1, '1'),
    (58, '操作日志导出', 'system:operlog:export', 18, 'api', NULL, 2, '1'),
    (60, '登录日志查询', 'system:logininfor:list', 19, 'api', NULL, 1, '1'),
    (61, '登录日志导出', 'system:logininfor:export', 19, 'api', NULL, 2, '1'),
    -- 在线用户与监控接口权限
    (63, '在线用户查询', 'monitor:online:list', 14, 'api', NULL, 1, '1'),
    (64, '在线用户强退', 'monitor:online:force-logout', 14, 'api', NULL, 2, '1'),
    (65, '服务器监控查询', 'monitor:server:list', 15, 'api', NULL, 1, '1'),
    (66, '缓存监控查询', 'monitor:cache:list', 16, 'api', NULL, 1, '1'),
    (67, '连接池监控查询', 'monitor:db-pool:list', 17, 'api', NULL, 1, '1'),
    -- 代码生成接口权限
    (68, '代码生成查询', 'tools:gen:list', 23, 'api', NULL, 1, '1'),
    (69, '代码生成操作', 'tools:gen:add', 23, 'api', NULL, 2, '1'),
    -- 运行时监控
    (70, '运行时监控', 'monitor:runtime', 13, 'menu', 'Operation', 5, '1'),
    (71, '运行时监控查询', 'monitor:runtime:list', 70, 'api', NULL, 1, '1');

-- -----------------------------------------------------------
-- 默认菜单 (sys_menu)
-- -----------------------------------------------------------
INSERT INTO `sys_menu` (`id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`) VALUES
    -- 主页
    (0,  '首页',   NULL, 'C', NULL, 'home', 'HomeFilled', 0, 1, '1'),
    -- 一级目录
    (1,  '系统管理', NULL, 'M', NULL, 'system', 'Setting', 1, 1, '1'),
    (2,  '系统监控', NULL, 'M', NULL, 'monitor', 'Monitor', 2, 1, '1'),
    (3,  '系统工具', NULL, 'M', NULL, 'tools', 'Tools', 3, 1, '1'),
    -- 系统管理子菜单
    (4,  '用户管理', 1, 'C', 7,  'system.user', 'User', 1, 1, '1'),
    (5,  '角色管理', 1, 'C', 24, 'system.role', 'UserFilled', 2, 1, '1'),
    (6,  '菜单管理', 1, 'C', 29, 'system.menu', 'Grid', 3, 1, '1'),
    (7,  '部门管理', 1, 'C', 34, 'system.dept', 'Menu', 4, 1, '1'),
    (8,  '岗位管理', 1, 'C', 38, 'system.post', 'Management', 5, 1, '1'),
    (9,  '字典管理', 1, 'C', 48, 'system.dict', 'Collection', 6, 1, '1'),
    (10, '参数设置', 1, 'C', 43, 'system.config', 'EditPen', 7, 1, '1'),
    (11, '通知公告', 1, 'C', 53, 'system.notice', 'Bell', 8, 1, '1'),
    (25, '权限管理', 1, 'C', 33, 'system.perm', 'Lock', 9, 1, '1'),
    (12, '操作日志', 1, 'C', 57, 'system.operlog', 'Document', 10, 1, '1'),
    (13, '登录日志', 1, 'C', 60, 'system.logininfor', 'Notebook', 11, 1, '1'),
    (26, '租户管理', NULL, 'C', 76, 'platform.tenant', 'OfficeBuilding', 12, 1, '1'),
    -- 系统监控子菜单
    (15, '在线用户', 2, 'C', 63, 'monitor.online', 'Connection', 1, 1, '1'),
    (16, '服务监控', 2, 'C', 65, 'monitor.server', 'DataAnalysis', 2, 1, '1'),
    (14, '运行时监控', 2, 'C', 71, 'monitor.runtime', 'Operation', 3, 1, '1'),
    (23, '缓存监控', 2, 'C', 66, 'monitor.cache', 'Coin', 4, 1, '1'),
    (24, '连接池监控', 2, 'C', 67, 'monitor.db-pool', 'Connection', 5, 1, '1'),
    -- 系统工具子菜单
    (17, '代码生成', 3, 'C', 68, 'tools.gen', 'MagicStick', 1, 1, '1'),
    -- 用户管理按钮
    (18, '用户查询', 4, 'F', 7,  NULL, NULL, 1, 1, '1'),
    (19, '用户新增', 4, 'F', 8,  NULL, NULL, 2, 1, '1'),
    (20, '用户修改', 4, 'F', 9,  NULL, NULL, 3, 1, '1'),
    (21, '用户删除', 4, 'F', 10, NULL, NULL, 4, 1, '1'),
    (22, '用户导出', 4, 'F', 11, NULL, NULL, 5, 1, '1'),
    -- 角色管理按钮
    (1024, '角色查询', 5, 'F', 24, NULL, NULL, 1, 1, '1'),
    (1025, '角色新增', 5, 'F', 25, NULL, NULL, 2, 1, '1'),
    (1026, '角色修改', 5, 'F', 26, NULL, NULL, 3, 1, '1'),
    (1027, '角色删除', 5, 'F', 27, NULL, NULL, 4, 1, '1'),
    (1028, '角色导出', 5, 'F', 28, NULL, NULL, 5, 1, '1'),
    -- 菜单管理按钮
    (1029, '菜单查询', 6, 'F', 29, NULL, NULL, 1, 1, '1'),
    (1030, '菜单新增', 6, 'F', 30, NULL, NULL, 2, 1, '1'),
    (1031, '菜单修改', 6, 'F', 31, NULL, NULL, 3, 1, '1'),
    (1032, '菜单删除', 6, 'F', 32, NULL, NULL, 4, 1, '1'),
    -- 权限管理按钮
    (1033, '权限查询', 25, 'F', 33, NULL, NULL, 1, 1, '1'),
    (1072, '权限新增', 25, 'F', 72, NULL, NULL, 2, 1, '1'),
    (1073, '权限修改', 25, 'F', 73, NULL, NULL, 3, 1, '1'),
    (1074, '权限删除', 25, 'F', 74, NULL, NULL, 4, 1, '1'),
    (1075, '权限同步', 25, 'F', 75, NULL, NULL, 5, 1, '1'),
    -- 部门管理按钮
    (1034, '部门查询', 7, 'F', 34, NULL, NULL, 1, 1, '1'),
    (1035, '部门新增', 7, 'F', 35, NULL, NULL, 2, 1, '1'),
    (1036, '部门修改', 7, 'F', 36, NULL, NULL, 3, 1, '1'),
    (1037, '部门删除', 7, 'F', 37, NULL, NULL, 4, 1, '1'),
    -- 岗位管理按钮
    (1038, '岗位查询', 8, 'F', 38, NULL, NULL, 1, 1, '1'),
    (1039, '岗位新增', 8, 'F', 39, NULL, NULL, 2, 1, '1'),
    (1040, '岗位修改', 8, 'F', 40, NULL, NULL, 3, 1, '1'),
    (1041, '岗位删除', 8, 'F', 41, NULL, NULL, 4, 1, '1'),
    (1042, '岗位导出', 8, 'F', 42, NULL, NULL, 5, 1, '1'),
    -- 字典管理按钮
    (1048, '字典查询', 9, 'F', 48, NULL, NULL, 1, 1, '1'),
    (1049, '字典新增', 9, 'F', 49, NULL, NULL, 2, 1, '1'),
    (1050, '字典修改', 9, 'F', 50, NULL, NULL, 3, 1, '1'),
    (1051, '字典删除', 9, 'F', 51, NULL, NULL, 4, 1, '1'),
    (1052, '字典导出', 9, 'F', 52, NULL, NULL, 5, 1, '1'),
    -- 参数设置按钮
    (1043, '参数查询', 10, 'F', 43, NULL, NULL, 1, 1, '1'),
    (1044, '参数新增', 10, 'F', 44, NULL, NULL, 2, 1, '1'),
    (1045, '参数修改', 10, 'F', 45, NULL, NULL, 3, 1, '1'),
    (1046, '参数删除', 10, 'F', 46, NULL, NULL, 4, 1, '1'),
    (1047, '参数导出', 10, 'F', 47, NULL, NULL, 5, 1, '1'),
    -- 通知公告按钮
    (1053, '通知查询', 11, 'F', 53, NULL, NULL, 1, 1, '1'),
    (1054, '通知新增', 11, 'F', 54, NULL, NULL, 2, 1, '1'),
    (1055, '通知修改', 11, 'F', 55, NULL, NULL, 3, 1, '1'),
    (1056, '通知删除', 11, 'F', 56, NULL, NULL, 4, 1, '1'),
    -- 日志管理按钮
    (1057, '操作日志查询', 12, 'F', 57, NULL, NULL, 1, 1, '1'),
    (1058, '操作日志导出', 12, 'F', 58, NULL, NULL, 2, 1, '1'),
    (1060, '登录日志查询', 13, 'F', 60, NULL, NULL, 1, 1, '1'),
    (1061, '登录日志导出', 13, 'F', 61, NULL, NULL, 2, 1, '1'),
    -- 系统监控按钮
    (1071, '运行时监控查询', 14, 'F', 71, NULL, NULL, 1, 1, '1'),
    (1063, '在线用户查询', 15, 'F', 63, NULL, NULL, 1, 1, '1'),
    (1064, '在线用户强退', 15, 'F', 64, NULL, NULL, 2, 1, '1'),
    (1065, '服务器监控查询', 16, 'F', 65, NULL, NULL, 1, 1, '1'),
    (1066, '缓存监控查询', 23, 'F', 66, NULL, NULL, 1, 1, '1'),
    (1067, '连接池监控查询', 24, 'F', 67, NULL, NULL, 1, 1, '1'),
    -- 系统工具按钮
    (1068, '代码生成查询', 17, 'F', 68, NULL, NULL, 1, 1, '1'),
    (1069, '代码生成操作', 17, 'F', 69, NULL, NULL, 2, 1, '1'),
    -- 租户管理按钮
    (1077, '租户查询', 26, 'F', 77, NULL, NULL, 1, 1, '1'),
    (1078, '租户新增', 26, 'F', 78, NULL, NULL, 2, 1, '1'),
    (1079, '租户修改', 26, 'F', 79, NULL, NULL, 3, 1, '1'),
    (1080, '租户状态', 26, 'F', 80, NULL, NULL, 4, 1, '1');

-- -----------------------------------------------------------
-- 默认岗位 (sys_post)
-- -----------------------------------------------------------
INSERT INTO `sys_post` (`id`, `name`, `code`, `sort`, `status`, `remark`) VALUES
    (1, '董事长',   'ceo',    1, '1', '公司最高管理者'),
    (2, '技术总监', 'cto',    2, '1', '技术部门负责人'),
    (3, '项目经理', 'pm',     3, '1', '项目经理'),
    (4, '普通员工', 'user',   4, '1', '普通员工');

-- -----------------------------------------------------------
-- 默认参数配置 (sys_config)
-- -----------------------------------------------------------
INSERT INTO `sys_config` (`id`, `name`, `key`, `value`, `remark`) VALUES
    (1, '主框架页-默认皮肤样式', 'sys.index.skinName',     'skin-blue',    '蓝色 skin-blue、绿色 skin-green、紫色 skin-purple、红色 skin-red、黄色 skin-yellow'),
    (3, '主框架页-侧边栏主题',  'sys.index.sideTheme',    'theme-light',  'dark主题theme-dark，light主题theme-light'),
    (4, '账号自助-验证码开关',  'sys.account.captchaEnabled', 'false',      '是否开启验证码功能（true开启，false关闭）'),
    (5, '账号自助-是否开启注册', 'sys.account.registerUser', 'false',      '是否开启注册功能（true开启，false关闭）');

-- -----------------------------------------------------------
-- 默认字典类型 (sys_dict_type)
-- -----------------------------------------------------------
INSERT INTO `sys_dict_type` (`id`, `name`, `code`, `status`, `remark`) VALUES
    (2, '菜单状态',   'sys_show_hide',   '1', '菜单状态列表'),
    (3, '系统开关',   'sys_normal_disable', '1', '系统正常停用状态'),
    (5, '系统是否',   'sys_yes_no',      '1', '系统是否列表'),
    (6, '通知类型',   'sys_notice_type', '1', '通知类型列表'),
    (7, '通知状态',   'sys_notice_status', '1', '通知状态列表'),
    (8, '操作类型',   'sys_oper_type',   '1', '操作日志类型'),
    (9, '登录状态',   'sys_common_status', '1', '登录状态列表');

-- -----------------------------------------------------------
-- 默认字典数据 (sys_dict_data)
-- -----------------------------------------------------------
INSERT INTO `sys_dict_data` (`id`, `type_code`, `label`, `value`, `sort`, `status`, `css_class`) VALUES
    -- 菜单状态
    (4,  'sys_show_hide', '显示', '1', 1, '1', 'primary'),
    (5,  'sys_show_hide', '隐藏', '0', 2, '1', 'danger'),
    -- 系统开关
    (6,  'sys_normal_disable', '正常', '1', 1, '1', 'primary'),
    (7,  'sys_normal_disable', '停用', '0', 2, '1', 'danger'),
    -- 系统是否
    (10, 'sys_yes_no', '是', 'Y', 1, '1', 'primary'),
    (11, 'sys_yes_no', '否', 'N', 2, '1', 'danger'),
    -- 通知类型
    (12, 'sys_notice_type', '通知', '1', 1, '1', 'primary'),
    (13, 'sys_notice_type', '公告', '2', 2, '1', 'success'),
    -- 通知状态
    (14, 'sys_notice_status', '正常', '1', 1, '1', 'primary'),
    (15, 'sys_notice_status', '关闭', '0', 2, '1', 'danger'),
    -- 操作类型
    (16, 'sys_oper_type', '其它',     '0',  1, '1', ''),
    (17, 'sys_oper_type', '新增',     '1',  2, '1', 'primary'),
    (18, 'sys_oper_type', '修改',     '2',  3, '1', 'primary'),
    (19, 'sys_oper_type', '删除',     '3',  4, '1', 'danger'),
    (20, 'sys_oper_type', '授权',     '4',  5, '1', 'primary'),
    (21, 'sys_oper_type', '导出',     '5',  6, '1', 'warning'),
    (22, 'sys_oper_type', '导入',     '6',  7, '1', 'warning'),
    -- 登录状态
    (23, 'sys_common_status', '成功', '1', 1, '1', 'primary'),
    (24, 'sys_common_status', '失败', '0', 2, '1', 'danger');

-- -----------------------------------------------------------
-- 用户-角色关联 (user_role)
-- -----------------------------------------------------------
INSERT INTO `sys_user_role` (`user_id`, `role_id`) VALUES
    (1, 1),  -- admin -> 超级管理员
    (2, 2);  -- user  -> 普通用户

-- -----------------------------------------------------------
-- 角色-权限关联 (role_permission)
-- 超级管理员拥有全部权限通配符
-- -----------------------------------------------------------
INSERT INTO `sys_role_permission` (`role_id`, `perm_id`) VALUES
    (1, 12),  -- admin -> *:*:*
    -- 普通用户拥有系统监控只读权限，菜单与 API 保持一致
    (2, 13),  -- common -> monitor
    (2, 14),  -- common -> monitor:online
    (2, 63),  -- common -> monitor:online:list
    (2, 15),  -- common -> monitor:server
    (2, 65),  -- common -> monitor:server:list
    (2, 70),  -- common -> monitor:runtime
    (2, 71),  -- common -> monitor:runtime:list
    (2, 16),  -- common -> monitor:cache
    (2, 66),  -- common -> monitor:cache:list
    (2, 17),  -- common -> monitor:db-pool
    (2, 67),  -- common -> monitor:db-pool:list
    (2, 43);  -- common -> system:config:list

SET FOREIGN_KEY_CHECKS = 1;
