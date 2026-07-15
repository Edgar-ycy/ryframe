-- Remove foreign keys from databases that applied an earlier draft of
-- 20260702_add_foreign_keys.sql. Both operations are idempotent.
-- sys_user uses soft deletion, so dept_id is validated by application code.
SET @drop_user_dept_fk = (
    SELECT IF(
        EXISTS(
            SELECT 1 FROM information_schema.TABLE_CONSTRAINTS
            WHERE CONSTRAINT_SCHEMA = DATABASE()
              AND TABLE_NAME = 'sys_user'
              AND CONSTRAINT_NAME = 'fk_sys_user_dept'
        ),
        'ALTER TABLE `sys_user` DROP FOREIGN KEY `fk_sys_user_dept`',
        'SELECT 1'
    )
);
PREPARE stmt FROM @drop_user_dept_fk;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Menu parent_id is a logical tree relation and is validated by MenuService.
SET @drop_menu_parent_fk = (
    SELECT IF(
        EXISTS(
            SELECT 1 FROM information_schema.TABLE_CONSTRAINTS
            WHERE CONSTRAINT_SCHEMA = DATABASE()
              AND TABLE_NAME = 'sys_menu'
              AND CONSTRAINT_NAME = 'fk_sys_menu_parent'
        ),
        'ALTER TABLE `sys_menu` DROP FOREIGN KEY `fk_sys_menu_parent`',
        'SELECT 1'
    )
);
PREPARE stmt FROM @drop_menu_parent_fk;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
