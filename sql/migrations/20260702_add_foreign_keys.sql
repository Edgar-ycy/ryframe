-- Add database-level foreign keys for tenant, permission, menu, role, user, and dept relations.
-- Run after 20260624_add_sys_tenant.sql.

ALTER TABLE `sys_dept`
    ADD KEY `idx_parent_id` (`parent_id`),
    ADD CONSTRAINT `fk_sys_dept_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_dept_parent`
        FOREIGN KEY (`parent_id`) REFERENCES `sys_dept` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL;

ALTER TABLE `sys_user`
    ADD CONSTRAINT `fk_sys_user_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `password_reset_requests`
    ADD CONSTRAINT `fk_password_reset_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_password_reset_target_user`
        FOREIGN KEY (`target_user_id`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    ADD CONSTRAINT `fk_password_reset_requested_by`
        FOREIGN KEY (`requested_by`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE `sys_role`
    ADD CONSTRAINT `fk_sys_role_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_permission`
    ADD CONSTRAINT `fk_sys_permission_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_permission_parent`
        FOREIGN KEY (`parent_id`) REFERENCES `sys_permission` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL;

ALTER TABLE `sys_menu`
    ADD KEY `idx_perm_id` (`perm_id`),
    ADD CONSTRAINT `fk_sys_menu_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_menu_permission`
        FOREIGN KEY (`perm_id`) REFERENCES `sys_permission` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL;

ALTER TABLE `sys_post`
    ADD CONSTRAINT `fk_sys_post_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_config`
    ADD CONSTRAINT `fk_sys_config_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_dict_type`
    ADD CONSTRAINT `fk_sys_dict_type_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_dict_data`
    ADD KEY `idx_dict_data_tenant_type` (`tenant_id`, `type_code`),
    ADD CONSTRAINT `fk_sys_dict_data_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_dict_data_type`
        FOREIGN KEY (`tenant_id`, `type_code`) REFERENCES `sys_dict_type` (`tenant_id`, `code`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_notice`
    ADD KEY `idx_created_by` (`created_by`),
    ADD CONSTRAINT `fk_sys_notice_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_notice_created_by`
        FOREIGN KEY (`created_by`) REFERENCES `sys_user` (`id`)
        ON UPDATE CASCADE ON DELETE SET NULL;

ALTER TABLE `sys_oper_log`
    ADD CONSTRAINT `fk_sys_oper_log_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_login_info`
    ADD CONSTRAINT `fk_sys_login_info_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;

ALTER TABLE `sys_user_role`
    ADD KEY `idx_user_id` (`user_id`);

ALTER TABLE `sys_role_permission`
    ADD KEY `idx_role_id` (`role_id`),
    ADD CONSTRAINT `fk_sys_role_permission_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_role_permission_role`
        FOREIGN KEY (`role_id`) REFERENCES `sys_role` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    ADD CONSTRAINT `fk_sys_role_permission_permission`
        FOREIGN KEY (`perm_id`) REFERENCES `sys_permission` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE `sys_role_dept`
    ADD KEY `idx_role_id` (`role_id`),
    ADD CONSTRAINT `fk_sys_role_dept_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT,
    ADD CONSTRAINT `fk_sys_role_dept_role`
        FOREIGN KEY (`role_id`) REFERENCES `sys_role` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE,
    ADD CONSTRAINT `fk_sys_role_dept_dept`
        FOREIGN KEY (`dept_id`) REFERENCES `sys_dept` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE `sys_file`
    ADD CONSTRAINT `fk_sys_file_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE RESTRICT;
