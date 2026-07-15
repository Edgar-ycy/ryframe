-- Add super-admin role marker and super-only button permissions.

ALTER TABLE `sys_role`
    ADD COLUMN `is_super` TINYINT(1) NOT NULL DEFAULT 0 COMMENT '是否超级管理员角色: 0否 1是' AFTER `code`;

UPDATE `sys_role`
SET `is_super` = CASE WHEN `code` = 'admin' THEN 1 ELSE 0 END;

INSERT INTO `sys_permission` (`id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`)
SELECT 82, '编辑自身角色', 'sys:user:editSelf', p.id, 'api', NULL, 7, '1'
FROM `sys_permission` p
WHERE p.`code` = 'system:user'
  AND NOT EXISTS (
      SELECT 1 FROM `sys_permission` e
      WHERE e.`tenant_id` = p.`tenant_id` AND e.`code` = 'sys:user:editSelf'
  );

INSERT INTO `sys_permission` (`id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`)
SELECT 83, '编辑超级管理员角色', 'sys:role:editSuper', p.id, 'api', NULL, 6, '1'
FROM `sys_permission` p
WHERE p.`code` = 'system:role'
  AND NOT EXISTS (
      SELECT 1 FROM `sys_permission` e
      WHERE e.`tenant_id` = p.`tenant_id` AND e.`code` = 'sys:role:editSuper'
  );

INSERT INTO `sys_menu` (`id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`)
SELECT 1082, p.`name`, m.`id`, 'F', p.`id`, NULL, NULL, 7, 1, '1'
FROM `sys_permission` p
JOIN `sys_menu` m ON m.`tenant_id` = p.`tenant_id` AND m.`route_key` = 'system.user' AND m.`menu_type` = 'C'
WHERE p.`code` = 'sys:user:editSelf'
  AND NOT EXISTS (
      SELECT 1 FROM `sys_menu` e
      WHERE e.`tenant_id` = p.`tenant_id` AND e.`perm_id` = p.`id` AND e.`menu_type` = 'F'
  );

INSERT INTO `sys_menu` (`id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`)
SELECT 1083, p.`name`, m.`id`, 'F', p.`id`, NULL, NULL, 6, 1, '1'
FROM `sys_permission` p
JOIN `sys_menu` m ON m.`tenant_id` = p.`tenant_id` AND m.`route_key` = 'system.role' AND m.`menu_type` = 'C'
WHERE p.`code` = 'sys:role:editSuper'
  AND NOT EXISTS (
      SELECT 1 FROM `sys_menu` e
      WHERE e.`tenant_id` = p.`tenant_id` AND e.`perm_id` = p.`id` AND e.`menu_type` = 'F'
  );

ALTER TABLE `sys_user_role` DROP FOREIGN KEY `fk_sys_user_role_tenant`;
ALTER TABLE `sys_user_role` DROP FOREIGN KEY `fk_sys_user_role_user`;
ALTER TABLE `sys_user_role` DROP FOREIGN KEY `fk_sys_user_role_role`;
