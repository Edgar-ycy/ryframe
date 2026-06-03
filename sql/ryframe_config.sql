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
-- 1. 部门表 (sys_dept)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_dept` (
    `id`          BIGINT       NOT NULL                    COMMENT '部门ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '部门名称',
    `parent_id`   BIGINT                DEFAULT NULL       COMMENT '父部门ID',
    `ancestors`   VARCHAR(512) NOT NULL DEFAULT ''         COMMENT '祖级列表(如: 0,1,2)',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='部门表';

-- ============================================================
-- 2. 用户表 (sys_user)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_user` (
    `id`             BIGINT       NOT NULL                  COMMENT '用户ID',
    `username`       VARCHAR(64)  NOT NULL                  COMMENT '用户名',
    `password_hash`  VARCHAR(255) NOT NULL                  COMMENT '密码哈希(argon2)',
    `nickname`       VARCHAR(64)  NOT NULL                  COMMENT '昵称',
    `email`          VARCHAR(128) NOT NULL DEFAULT ''       COMMENT '邮箱',
    `phone`          VARCHAR(32)  NOT NULL DEFAULT ''       COMMENT '手机号',
    `avatar`         VARCHAR(255)          DEFAULT NULL     COMMENT '头像URL',
    `status`         CHAR(1)      NOT NULL DEFAULT '1'      COMMENT '状态: 0停用 1正常 2锁定',
    `dept_id`        BIGINT                DEFAULT NULL     COMMENT '部门ID',
    `remark`         VARCHAR(512)          DEFAULT NULL     COMMENT '备注',
    `login_ip`       VARCHAR(128)          DEFAULT NULL     COMMENT '最后登录IP',
    `login_date`     DATETIME              DEFAULT NULL     COMMENT '最后登录时间',
    `del_flag`       CHAR(1)      NOT NULL DEFAULT '0'      COMMENT '删除标志: 0正常 2删除',
    `created_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_username` (`username`),
    KEY `idx_dept_id` (`dept_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户表';

-- ============================================================
-- 3. 角色表 (sys_role)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_role` (
    `id`          BIGINT       NOT NULL                    COMMENT '角色ID',
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
    UNIQUE KEY `uk_code` (`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色表';

-- ============================================================
-- 4. 权限表 (sys_permission)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_permission` (
    `id`          BIGINT       NOT NULL                    COMMENT '权限ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '权限名称',
    `code`        VARCHAR(128) NOT NULL                    COMMENT '权限编码',
    `parent_id`   BIGINT                DEFAULT NULL       COMMENT '父权限ID(树形)',
    `perm_type`   VARCHAR(16)  NOT NULL                    COMMENT '权限类型: menu/api',
    `path`        VARCHAR(255)          DEFAULT NULL       COMMENT '路由路径',
    `icon`        VARCHAR(64)           DEFAULT NULL       COMMENT '图标',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_code` (`code`),
    KEY `idx_parent_id` (`parent_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='权限表';

-- ============================================================
-- 5. 菜单表 (sys_menu)
-- 统一管理目录(M)、菜单(C)、按钮(F)，前端通过 menu_type 区分
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_menu` (
    `id`          BIGINT       NOT NULL                    COMMENT '菜单ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '菜单名称',
    `parent_id`   BIGINT                DEFAULT NULL       COMMENT '父菜单ID',
    `menu_type`   CHAR(1)      NOT NULL DEFAULT ''         COMMENT '菜单类型: M目录 C菜单 F按钮',
    `path`        VARCHAR(255)          DEFAULT NULL       COMMENT '路由路径(目录/菜单)或接口路径(按钮)',
    `component`   VARCHAR(255)          DEFAULT NULL       COMMENT '组件路径(仅菜单类型)',
    `query`       VARCHAR(255)          DEFAULT NULL       COMMENT '路由参数(如 id=1)',
    `perms`       VARCHAR(128)          DEFAULT NULL       COMMENT '权限标识(如 system:user:list)',
    `icon`        VARCHAR(128)          DEFAULT NULL       COMMENT '图标',
    `is_frame`    TINYINT(1)   NOT NULL DEFAULT 0          COMMENT '是否外链: 0否 1是',
    `is_cache`    TINYINT(1)   NOT NULL DEFAULT 0          COMMENT '是否缓存: 0否 1是',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `visible`     TINYINT(1)   NOT NULL DEFAULT 1          COMMENT '是否可见: 0隐藏 1显示',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    KEY `idx_parent_id` (`parent_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='菜单表(含目录/菜单/按钮)';

-- ============================================================
-- 6. 岗位表 (sys_post)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_post` (
    `id`          BIGINT       NOT NULL                    COMMENT '岗位ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '岗位名称',
    `code`        VARCHAR(64)  NOT NULL                    COMMENT '岗位编码',
    `sort`        INT          NOT NULL DEFAULT 0          COMMENT '显示顺序',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_code` (`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='岗位表';

-- ============================================================
-- 7. 参数配置表 (sys_config)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_config` (
    `id`          BIGINT       NOT NULL                    COMMENT '配置ID',
    `name`        VARCHAR(128) NOT NULL                    COMMENT '配置名称',
    `key`         VARCHAR(128) NOT NULL                    COMMENT '配置键',
    `value`       VARCHAR(512) NOT NULL                    COMMENT '配置值',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_key` (`key`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='参数配置表';

-- ============================================================
-- 8. 字典类型表 (sys_dict_type)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_dict_type` (
    `id`          BIGINT       NOT NULL                    COMMENT '字典类型ID',
    `name`        VARCHAR(64)  NOT NULL                    COMMENT '字典名称',
    `code`        VARCHAR(64)  NOT NULL                    COMMENT '字典类型编码',
    `status`      CHAR(1)      NOT NULL DEFAULT '1'        COMMENT '状态: 0停用 1正常',
    `remark`      VARCHAR(512)          DEFAULT NULL       COMMENT '备注',
    `del_flag`    CHAR(1)      NOT NULL DEFAULT '0'        COMMENT '删除标志: 0正常 2删除',
    `created_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `updated_at`  DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_code` (`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='字典类型表';

-- ============================================================
-- 9. 字典数据表 (sys_dict_data)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_dict_data` (
    `id`          BIGINT       NOT NULL                    COMMENT '字典数据ID',
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
    KEY `idx_type_code` (`type_code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='字典数据表';

-- ============================================================
-- 10. 通知公告表 (sys_notice)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_notice` (
    `id`           BIGINT       NOT NULL                   COMMENT '公告ID',
    `title`        VARCHAR(128) NOT NULL                   COMMENT '公告标题',
    `content`      TEXT         NOT NULL                   COMMENT '公告内容',
    `type`         VARCHAR(16)           DEFAULT NULL      COMMENT '公告类型: notice/announcement',
    `status`       CHAR(1)      NOT NULL DEFAULT '1'       COMMENT '状态: 0草稿 1已发布 2已关闭',
    `created_by`   BIGINT                DEFAULT NULL      COMMENT '创建人ID',
    `del_flag`     CHAR(1)      NOT NULL DEFAULT '0'       COMMENT '删除标志: 0正常 2删除',
    `created_at`   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP   COMMENT '创建时间',
    `updated_at`   DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='通知公告表';

-- ============================================================
-- 11. 操作日志表 (sys_oper_log)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_oper_log` (
    `id`              BIGINT       NOT NULL                COMMENT '日志ID',
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
    KEY `idx_oper_time` (`oper_time`),
    KEY `idx_business_type` (`business_type`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='操作日志表';

-- ============================================================
-- 12. 登录信息表 (sys_login_info)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_login_info` (
    `id`              BIGINT       NOT NULL                COMMENT '日志ID',
    `user_name`       VARCHAR(64)  NOT NULL                COMMENT '用户名',
    `ipaddr`          VARCHAR(128) NOT NULL                COMMENT '登录IP',
    `login_location`  VARCHAR(128)          DEFAULT NULL   COMMENT '登录地点',
    `browser`         VARCHAR(64)           DEFAULT NULL   COMMENT '浏览器',
    `os`              VARCHAR(64)           DEFAULT NULL   COMMENT '操作系统',
    `status`          CHAR(1)      NOT NULL DEFAULT '1'    COMMENT '登录状态: 0失败 1成功',
    `msg`             VARCHAR(255)          DEFAULT NULL   COMMENT '提示信息',
    `login_time`      DATETIME     NOT NULL                COMMENT '登录时间',
    PRIMARY KEY (`id`),
    KEY `idx_login_time` (`login_time`),
    KEY `idx_user_name` (`user_name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='登录信息表';

-- ============================================================
-- 13. 定时任务表 (sys_job)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_job` (
    `id`              BIGINT       NOT NULL                COMMENT '任务ID',
    `name`            VARCHAR(64)  NOT NULL                COMMENT '任务名称（与代码注册名对应）',
    `group_name`      VARCHAR(64)  NOT NULL DEFAULT 'system' COMMENT '任务分组',
    `cron_expr`       VARCHAR(128) NOT NULL                COMMENT 'Cron 表达式',
    `misfire_policy`  CHAR(1)      NOT NULL DEFAULT '1'    COMMENT '失败策略: 1立即执行 2执行一次 3放弃',
    `concurrent`      CHAR(1)      NOT NULL DEFAULT '0'    COMMENT '并发执行: 0禁止 1允许',
    `status`          CHAR(1)      NOT NULL DEFAULT '1'    COMMENT '状态: 0暂停 1正常',
    `remark`          VARCHAR(512)          DEFAULT NULL   COMMENT '备注',
    `create_time`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP    COMMENT '创建时间',
    `update_time`     DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`id`),
    UNIQUE KEY `uk_name` (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='定时任务表';

-- ============================================================
-- 14. 定时任务执行日志表 (sys_job_log)
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_job_log` (
    `id`           BIGINT       NOT NULL                   COMMENT '日志ID',
    `job_name`     VARCHAR(64)  NOT NULL                   COMMENT '任务名称',
    `job_group`    VARCHAR(64)  NOT NULL DEFAULT 'system'  COMMENT '任务分组',
    `message`      TEXT         NOT NULL                   COMMENT '执行消息',
    `status`       CHAR(1)      NOT NULL DEFAULT '0'       COMMENT '状态: 0失败 1成功',
    `error_msg`    TEXT                  DEFAULT NULL      COMMENT '错误信息',
    `cost_ms`      BIGINT       NOT NULL DEFAULT 0         COMMENT '耗时(毫秒)',
    `start_time`   DATETIME     NOT NULL                   COMMENT '开始时间',
    PRIMARY KEY (`id`),
    KEY `idx_job_name` (`job_name`),
    KEY `idx_start_time` (`start_time`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='定时任务执行日志表';

-- ============================================================
-- 15. 用户-角色关联表 (user_role)
-- ============================================================
CREATE TABLE IF NOT EXISTS `user_role` (
    `user_id`  BIGINT NOT NULL  COMMENT '用户ID',
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    PRIMARY KEY (`user_id`, `role_id`),
    KEY `idx_role_id` (`role_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户-角色关联表';

-- ============================================================
-- 16. 角色-权限关联表 (role_permission)
-- ============================================================
CREATE TABLE IF NOT EXISTS `role_permission` (
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    `perm_id`  BIGINT NOT NULL  COMMENT '权限ID',
    PRIMARY KEY (`role_id`, `perm_id`),
    KEY `idx_perm_id` (`perm_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色-权限关联表';

-- ============================================================
-- 17. 角色-菜单关联表 (role_menu)
-- ============================================================
CREATE TABLE IF NOT EXISTS `role_menu` (
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    `menu_id`  BIGINT NOT NULL  COMMENT '菜单ID',
    PRIMARY KEY (`role_id`, `menu_id`),
    KEY `idx_menu_id` (`menu_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色-菜单关联表';

-- ============================================================
-- 18. 角色-部门关联表 (sys_role_dept) — 自定义数据权限
-- ============================================================
CREATE TABLE IF NOT EXISTS `sys_role_dept` (
    `role_id`  BIGINT NOT NULL  COMMENT '角色ID',
    `dept_id`  BIGINT NOT NULL  COMMENT '部门ID',
    PRIMARY KEY (`role_id`, `dept_id`),
    KEY `idx_dept_id` (`dept_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='角色-部门关联表(自定义数据权限)';

-- ============================================================
-- =================== 默认初始化数据 =========================
-- ============================================================
-- ID 规划:
--   sys_dept:       1~100
--   sys_role:       1~100
--   sys_user:       1~100
--   sys_permission: 1~100
--   sys_menu:       1~100
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
INSERT INTO `sys_permission` (`id`, `name`, `code`, `parent_id`, `perm_type`, `path`, `icon`, `sort`, `status`) VALUES
    -- 系统管理
    (1,  '系统管理',   'system',            NULL, 'menu', '/system',  'Setting', 1, '1'),
    (2,  '用户管理',   'system:user',       1,    'menu', '/system/user',  'User',   1, '1'),
    (3,  '角色管理',   'system:role',       1,    'menu', '/system/role',  'UserFilled', 2, '1'),
    (4,  '菜单管理',   'system:menu',       1,    'menu', '/system/menu',  'Menu',   3, '1'),
    (5,  '部门管理',   'system:dept',       1,    'menu', '/system/dept',  'Grid', 4, '1'),
    (6,  '岗位管理',   'system:post',       1,    'menu', '/system/post',  'Management',   5, '1'),
    -- 用户管理按钮权限
    (7,  '用户查询',   'system:user:list',   2,    'api', NULL, NULL, 1, '1'),
    (8,  '用户新增',   'system:user:add',    2,    'api', NULL, NULL, 2, '1'),
    (9,  '用户修改',   'system:user:edit',   2,    'api', NULL, NULL, 3, '1'),
    (10, '用户删除',   'system:user:remove', 2,    'api', NULL, NULL, 4, '1'),
    -- 超级管理员通配符权限
    (11, '全部权限',   '*:*:*',             NULL, 'api', NULL, NULL, 0, '1'),
    -- 系统监控
    (12, '系统监控',   'monitor',           NULL, 'menu', '/monitor', 'Monitor', 2, '1'),
    (13, '在线用户',   'monitor:online',    12,   'menu', '/monitor/online', 'Connection', 1, '1'),
    (14, '服务器监控', 'monitor:server',    12,   'menu', '/monitor/server', 'DataAnalysis', 2, '1'),
    -- 日志管理
    (15, '操作日志',   'system:operlog',    1,    'menu', '/system/operlog', 'Document', 6, '1'),
    (16, '登录日志',   'system:logininfor', 1,    'menu', '/system/logininfor', 'Notebook', 7, '1'),
    -- 定时任务
    (17, '定时任务',   'system:job',        1,    'menu', '/system/job', 'Timer', 8, '1'),
    -- 字典管理
    (18, '字典管理',   'system:dict',       1,    'menu', '/system/dict', 'Collection', 9, '1'),
    -- 参数设置
    (19, '参数设置',   'system:config',     1,    'menu', '/system/config', 'EditPen', 10, '1'),
    -- 通知公告
    (20, '通知公告',   'system:notice',     1,    'menu', '/system/notice', 'Bell', 11, '1'),
    -- 代码生成
    (21, '代码生成',   'tools:gen',         NULL, 'menu', '/tools/gen', 'MagicStick', 3, '1');

-- -----------------------------------------------------------
-- 默认菜单 (sys_menu)
-- -----------------------------------------------------------
INSERT INTO `sys_menu` (`id`, `name`, `parent_id`, `menu_type`, `path`, `component`, `query`, `perms`, `icon`, `is_frame`, `is_cache`, `sort`, `visible`, `status`) VALUES
    -- 主页
    (0,  '首页',   NULL, 'C', '/dashboard',    'dashboard/index',   NULL,  NULL,          'HomeFilled',    0, 0, 0, 1, '1'),
    -- 一级目录
    (1,  '系统管理', NULL, 'M', '/system',  'Layout', NULL,  NULL,          'Setting',    0, 0, 1, 1, '1'),
    (2,  '系统监控', NULL, 'M', '/monitor', 'Layout', NULL,  NULL,          'Monitor',    0, 0, 2, 1, '1'),
    (3,  '系统工具', NULL, 'M', '/tools',   'Layout', NULL,  NULL,          'Tools',      0, 0, 3, 1, '1'),
    -- 系统管理子菜单
    (4,  '用户管理', 1, 'C', '/system/user',      'system/user/index',      NULL,  'system:user:list',   'User',          0, 0, 1, 1, '1'),
    (5,  '角色管理', 1, 'C', '/system/role',      'system/role/index',      NULL,  'system:role:list',   'UserFilled',    0, 0, 2, 1, '1'),
    (6,  '菜单管理', 1, 'C', '/system/menu',      'system/menu/index',      NULL,  'system:menu:list',   'Grid',          0, 0, 3, 1, '1'),
    (7,  '部门管理', 1, 'C', '/system/dept',      'system/dept/index',      NULL,  'system:dept:list',   'Menu',          0, 0, 4, 1, '1'),
    (8,  '岗位管理', 1, 'C', '/system/post',      'system/post/index',      NULL,  'system:post:list',   'Management',    0, 0, 5, 1, '1'),
    (9,  '字典管理', 1, 'C', '/system/dict',      'system/dict/index',      NULL,  'system:dict:list',   'Collection',    0, 0, 6, 1, '1'),
    (10, '参数设置', 1, 'C', '/system/config',    'system/config/index',    NULL,  'system:config:list', 'EditPen',       0, 0, 7, 1, '1'),
    (11, '通知公告', 1, 'C', '/system/notice',    'system/notice/index',    NULL,  'system:notice:list', 'Bell',          0, 0, 8, 1, '1'),
    (12, '操作日志', 1, 'C', '/system/operlog',   'system/operlog/index',   NULL,  'system:operlog:list','Document',      0, 0, 9, 1, '1'),
    (13, '登录日志', 1, 'C', '/system/logininfor','system/logininfor/index',NULL,  'system:logininfor:list','Notebook',  0, 0, 10, 1, '1'),
    (14, '定时任务', 1, 'C', '/system/job',       'system/job/index',       NULL,  'system:job:list',    'Timer',         0, 0, 11, 1, '1'),
    -- 系统监控子菜单
    (15, '在线用户', 2, 'C', '/monitor/online',   'monitor/online/index',   NULL,  'monitor:online:list','Connection',    0, 0, 1, 1, '1'),
    (16, '服务监控', 2, 'C', '/monitor/server',   'monitor/server/index',   NULL,  'monitor:server:list','DataAnalysis',  0, 0, 2, 1, '1'),
    -- 系统工具子菜单
    (17, '代码生成', 3, 'C', '/tools/gen',        'tools/gen/index',        NULL,  'tools:gen:list',     'MagicStick',    0, 0, 1, 1, '1'),
    -- 用户管理按钮
    (18, '用户查询', 4, 'F', NULL,  NULL, NULL, 'system:user:list',   NULL, 0, 0, 1, 1, '1'),
    (19, '用户新增', 4, 'F', NULL,  NULL, NULL, 'system:user:add',    NULL, 0, 0, 2, 1, '1'),
    (20, '用户修改', 4, 'F', NULL,  NULL, NULL, 'system:user:edit',   NULL, 0, 0, 3, 1, '1'),
    (21, '用户删除', 4, 'F', NULL,  NULL, NULL, 'system:user:remove', NULL, 0, 0, 4, 1, '1'),
    (22, '用户导出', 4, 'F', NULL,  NULL, NULL, 'system:user:export', NULL, 0, 0, 5, 1, '1');

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
    (2, '用户管理-账号初始密码', 'sys.user.initPassword',  '123456',   '初始化密码'),
    (3, '主框架页-侧边栏主题',  'sys.index.sideTheme',    'theme-dark',   'dark主题theme-dark，light主题theme-light'),
    (4, '账号自助-验证码开关',  'sys.account.captchaEnabled', 'true',       '是否开启验证码功能（true开启，false关闭）'),
    (5, '账号自助-是否开启注册', 'sys.account.registerUser', 'false',      '是否开启注册功能（true开启，false关闭）');

-- -----------------------------------------------------------
-- 默认字典类型 (sys_dict_type)
-- -----------------------------------------------------------
INSERT INTO `sys_dict_type` (`id`, `name`, `code`, `status`, `remark`) VALUES
    (1, '用户性别',   'sys_user_sex',    '1', '用户性别列表'),
    (2, '菜单状态',   'sys_show_hide',   '1', '菜单状态列表'),
    (3, '系统开关',   'sys_normal_disable', '1', '系统正常停用状态'),
    (4, '任务状态',   'sys_job_status',  '1', '定时任务状态'),
    (5, '系统是否',   'sys_yes_no',      '1', '系统是否列表'),
    (6, '通知类型',   'sys_notice_type', '1', '通知类型列表'),
    (7, '通知状态',   'sys_notice_status', '1', '通知状态列表'),
    (8, '操作类型',   'sys_oper_type',   '1', '操作日志类型'),
    (9, '登录状态',   'sys_common_status', '1', '登录状态列表');

-- -----------------------------------------------------------
-- 默认字典数据 (sys_dict_data)
-- -----------------------------------------------------------
INSERT INTO `sys_dict_data` (`id`, `type_code`, `label`, `value`, `sort`, `status`, `css_class`) VALUES
    -- 用户性别
    (1,  'sys_user_sex', '男',   '0', 1, '1', ''),
    (2,  'sys_user_sex', '女',   '1', 2, '1', ''),
    (3,  'sys_user_sex', '未知', '2', 3, '1', ''),
    -- 菜单状态
    (4,  'sys_show_hide', '显示', '1', 1, '1', 'primary'),
    (5,  'sys_show_hide', '隐藏', '0', 2, '1', 'danger'),
    -- 系统开关
    (6,  'sys_normal_disable', '正常', '1', 1, '1', 'primary'),
    (7,  'sys_normal_disable', '停用', '0', 2, '1', 'danger'),
    -- 任务状态
    (8,  'sys_job_status', '正常', '1', 1, '1', 'primary'),
    (9,  'sys_job_status', '暂停', '0', 2, '1', 'danger'),
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
INSERT INTO `user_role` (`user_id`, `role_id`) VALUES
    (1, 1),  -- admin -> 超级管理员
    (2, 2);  -- user  -> 普通用户

-- -----------------------------------------------------------
-- 角色-权限关联 (role_permission)
-- 超级管理员拥有全部权限通配符
-- -----------------------------------------------------------
INSERT INTO `role_permission` (`role_id`, `perm_id`) VALUES
    (1, 11),  -- admin -> *:*:*
    -- 普通用户拥有基础查看权限
    (2, 7),   -- common -> system:user:list
    (2, 1);   -- common -> system (查看)

-- -----------------------------------------------------------
-- 角色-菜单关联 (role_menu)
-- 超级管理员拥有全部菜单（含按钮）
-- -----------------------------------------------------------
INSERT INTO `role_menu` (`role_id`, `menu_id`) VALUES
    -- 超级管理员 - 全部菜单
    (1, 0),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (1, 8),
    (1, 9),
    (1, 10),
    (1, 11),
    (1, 12),
    (1, 13),
    (1, 14),
    (1, 15),
    (1, 16),
    (1, 17),
    (1, 18),
    (1, 19),
    (1, 20),
    (1, 21),
    (1, 22),
    -- 普通用户 - 首页 + 系统监控菜单
    (2, 0),
    (2, 2),
    (2, 15),
    (2, 16);

SET FOREIGN_KEY_CHECKS = 1;
