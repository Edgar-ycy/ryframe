-- Align relation constraints and button menu records with the permission architecture.
-- Run after 20260702_add_foreign_keys.sql.

-- sys_user and sys_dept both use soft deletion, so dept_id is validated by application code.
ALTER TABLE `sys_user`
    DROP FOREIGN KEY `fk_sys_user_dept`;

-- Menu trees are validated by application code to avoid self-referencing FK maintenance issues.
ALTER TABLE `sys_menu`
    DROP FOREIGN KEY `fk_sys_menu_parent`;

-- Every non-wildcard API permission has a corresponding button record in sys_menu.
INSERT IGNORE INTO `sys_menu`
    (`id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`)
VALUES
    (18, '用户查询', 4, 'F', 7, NULL, NULL, 1, 1, '1'),
    (19, '用户新增', 4, 'F', 8, NULL, NULL, 2, 1, '1'),
    (20, '用户修改', 4, 'F', 9, NULL, NULL, 3, 1, '1'),
    (21, '用户删除', 4, 'F', 10, NULL, NULL, 4, 1, '1'),
    (22, '用户导出', 4, 'F', 11, NULL, NULL, 5, 1, '1'),
    (1024, '角色查询', 5, 'F', 24, NULL, NULL, 1, 1, '1'),
    (1025, '角色新增', 5, 'F', 25, NULL, NULL, 2, 1, '1'),
    (1026, '角色修改', 5, 'F', 26, NULL, NULL, 3, 1, '1'),
    (1027, '角色删除', 5, 'F', 27, NULL, NULL, 4, 1, '1'),
    (1028, '角色导出', 5, 'F', 28, NULL, NULL, 5, 1, '1'),
    (1029, '菜单查询', 6, 'F', 29, NULL, NULL, 1, 1, '1'),
    (1030, '菜单新增', 6, 'F', 30, NULL, NULL, 2, 1, '1'),
    (1031, '菜单修改', 6, 'F', 31, NULL, NULL, 3, 1, '1'),
    (1032, '菜单删除', 6, 'F', 32, NULL, NULL, 4, 1, '1'),
    (1033, '权限查询', 25, 'F', 33, NULL, NULL, 1, 1, '1'),
    (1072, '权限新增', 25, 'F', 72, NULL, NULL, 2, 1, '1'),
    (1073, '权限修改', 25, 'F', 73, NULL, NULL, 3, 1, '1'),
    (1074, '权限删除', 25, 'F', 74, NULL, NULL, 4, 1, '1'),
    (1075, '权限同步', 25, 'F', 75, NULL, NULL, 5, 1, '1'),
    (1034, '部门查询', 7, 'F', 34, NULL, NULL, 1, 1, '1'),
    (1035, '部门新增', 7, 'F', 35, NULL, NULL, 2, 1, '1'),
    (1036, '部门修改', 7, 'F', 36, NULL, NULL, 3, 1, '1'),
    (1037, '部门删除', 7, 'F', 37, NULL, NULL, 4, 1, '1'),
    (1038, '岗位查询', 8, 'F', 38, NULL, NULL, 1, 1, '1'),
    (1039, '岗位新增', 8, 'F', 39, NULL, NULL, 2, 1, '1'),
    (1040, '岗位修改', 8, 'F', 40, NULL, NULL, 3, 1, '1'),
    (1041, '岗位删除', 8, 'F', 41, NULL, NULL, 4, 1, '1'),
    (1042, '岗位导出', 8, 'F', 42, NULL, NULL, 5, 1, '1'),
    (1048, '字典查询', 9, 'F', 48, NULL, NULL, 1, 1, '1'),
    (1049, '字典新增', 9, 'F', 49, NULL, NULL, 2, 1, '1'),
    (1050, '字典修改', 9, 'F', 50, NULL, NULL, 3, 1, '1'),
    (1051, '字典删除', 9, 'F', 51, NULL, NULL, 4, 1, '1'),
    (1052, '字典导出', 9, 'F', 52, NULL, NULL, 5, 1, '1'),
    (1043, '参数查询', 10, 'F', 43, NULL, NULL, 1, 1, '1'),
    (1044, '参数新增', 10, 'F', 44, NULL, NULL, 2, 1, '1'),
    (1045, '参数修改', 10, 'F', 45, NULL, NULL, 3, 1, '1'),
    (1046, '参数删除', 10, 'F', 46, NULL, NULL, 4, 1, '1'),
    (1047, '参数导出', 10, 'F', 47, NULL, NULL, 5, 1, '1'),
    (1053, '通知查询', 11, 'F', 53, NULL, NULL, 1, 1, '1'),
    (1054, '通知新增', 11, 'F', 54, NULL, NULL, 2, 1, '1'),
    (1055, '通知修改', 11, 'F', 55, NULL, NULL, 3, 1, '1'),
    (1056, '通知删除', 11, 'F', 56, NULL, NULL, 4, 1, '1'),
    (1057, '操作日志查询', 12, 'F', 57, NULL, NULL, 1, 1, '1'),
    (1058, '操作日志导出', 12, 'F', 58, NULL, NULL, 2, 1, '1'),
    (1060, '登录日志查询', 13, 'F', 60, NULL, NULL, 1, 1, '1'),
    (1061, '登录日志导出', 13, 'F', 61, NULL, NULL, 2, 1, '1'),
    (1071, '运行时监控查询', 14, 'F', 71, NULL, NULL, 1, 1, '1'),
    (1063, '在线用户查询', 15, 'F', 63, NULL, NULL, 1, 1, '1'),
    (1064, '在线用户强退', 15, 'F', 64, NULL, NULL, 2, 1, '1'),
    (1065, '服务器监控查询', 16, 'F', 65, NULL, NULL, 1, 1, '1'),
    (1066, '缓存监控查询', 23, 'F', 66, NULL, NULL, 1, 1, '1'),
    (1067, '连接池监控查询', 24, 'F', 67, NULL, NULL, 1, 1, '1'),
    (1068, '代码生成查询', 17, 'F', 68, NULL, NULL, 1, 1, '1'),
    (1069, '代码生成操作', 17, 'F', 69, NULL, NULL, 2, 1, '1'),
    (1077, '租户查询', 26, 'F', 77, NULL, NULL, 1, 1, '1'),
    (1078, '租户新增', 26, 'F', 78, NULL, NULL, 2, 1, '1'),
    (1079, '租户修改', 26, 'F', 79, NULL, NULL, 3, 1, '1'),
    (1080, '租户状态', 26, 'F', 80, NULL, NULL, 4, 1, '1');
