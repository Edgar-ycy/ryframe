use super::*;

/// 生成空控制库的完整初始化脚本。调用方必须使用遇错即停的导入模式。
pub fn mysql_initialization_sql(hashes: &crate::migration::BootstrapHashes) -> String {
    mysql_initialization_sql_with_business(hashes, &[], &[])
}

/// 业务 SQL 已完成完整性校验，先执行结构与业务账本，再登记控制库完成标记。
pub fn mysql_initialization_sql_with_business(
    hashes: &crate::migration::BootstrapHashes,
    business: &[String],
    finish: &[String],
) -> String {
    let snapshot = hashes.render(&mysql_snapshot_sql());
    let start = snapshot
        .find("CREATE TABLE IF NOT EXISTS")
        .expect("控制库包含表");
    let mut sql = String::from(
        "-- RyFrame 完整控制库初始化 SQL：只允许导入明确选择的空库。\n\
         -- 必须使用遇错即停模式；错误后保留现场，不自动重放或删除对象。\n\
         -- 包含管理员密码哈希，请作为部署凭据保管，不提交版本库。\n\
         SET NAMES utf8mb4;\n\
         CREATE TEMPORARY TABLE `ryframe_initialization_guard` (`valid` TINYINT NOT NULL CHECK (`valid` = 1));\n\
         INSERT INTO `ryframe_initialization_guard` SELECT IF(\
         (SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE()) = 0 \
         AND VERSION() NOT LIKE '%MariaDB%' \
         AND (CAST(SUBSTRING_INDEX(VERSION(), '.', 1) AS UNSIGNED) > 8 \
         OR (CAST(SUBSTRING_INDEX(VERSION(), '.', 1) AS UNSIGNED) = 8 \
         AND (CAST(SUBSTRING_INDEX(SUBSTRING_INDEX(VERSION(), '.', 2), '.', -1) AS UNSIGNED) > 0 \
         OR CAST(SUBSTRING_INDEX(SUBSTRING_INDEX(VERSION(), '.', 3), '.', -1) AS UNSIGNED) >= 16))), 1, 0);\n\n",
    );
    sql.push_str(&snapshot[start..].replace("CREATE TABLE IF NOT EXISTS", "CREATE TABLE"));
    sql.push_str("CREATE TABLE `seaql_migrations` (`version` VARCHAR(255) NOT NULL PRIMARY KEY, `applied_at` BIGINT NOT NULL);\n");
    for statement in business {
        sql.push('\n');
        sql.push_str(statement.trim_end_matches([';', '\n', '\r', ' ']));
        sql.push_str(";\n");
    }
    sql.push_str("\n-- 所有结构和初始化数据成功后登记迁移；导入完成后执行 control verify。\n");
    for name in crate::migration::expected_migration_names() {
        sql.push_str(&format!("INSERT INTO `seaql_migrations` (`version`, `applied_at`) SELECT '{name}', UNIX_TIMESTAMP() FROM `ryframe_initialization_guard` WHERE `valid` = 1;\n"));
    }
    for statement in finish {
        append_snapshot_statement(&mut sql, statement);
    }
    sql.push_str("DROP TEMPORARY TABLE `ryframe_initialization_guard`;\n");
    sql
}

pub fn mysql_snapshot_sql() -> String {
    let mut snapshot = format!(
        "-- 自动生成文件：RyFrame 控制库新基线快照。\n\
         -- schema fingerprint: {}\n\
         -- 唯一事实来源：ryframe-db::migration Migrator 与 Seeder。\n\
         -- 仅供审阅：部署和重置工具不得执行此文件。\n\
         -- 重新生成命令：cargo run --locked -p ryframe-db --features migration --bin export_mysql_snapshot -- sql/ryframe_config.sql\n\n",
        crate::migration::m20260820_000000_control_baseline::schema_fingerprint()
    );
    for statement in ddl_statements() {
        snapshot.push_str(statement.trim());
        snapshot.push_str(";\n\n");
    }
    for statement in post_table_statements() {
        append_snapshot_statement(&mut snapshot, statement);
    }
    snapshot.push_str("-- 幂等初始化数据；此审阅快照不用于部署初始化。\n\n");
    for statement in seed_statements() {
        snapshot.push_str(
            &idempotent_upsert(statement)
                .expect("canonical seed statements must be valid INSERT statements"),
        );
        snapshot.push_str(";\n\n");
    }
    snapshot.push_str("-- 访问目录、产品套餐和租户数据放置种子。\n\n");
    for statement in access_catalog_snapshot_statements().expect("访问目录必须在构建时通过严格解析")
    {
        append_snapshot_statement(&mut snapshot, &statement);
    }
    for statement in PRODUCT_SEED_STATEMENTS {
        append_snapshot_statement(&mut snapshot, statement);
    }
    append_snapshot_statement(&mut snapshot, TENANT_PLACEMENT_SEED_STATEMENT);
    snapshot.push_str(&data_scope_projection_event_statement());
    snapshot.push_str(";\n\n");
    append_snapshot_statement(&mut snapshot, RETENTION_SCHEDULE_SEED_STATEMENT);
    snapshot.truncate(snapshot.trim_end().len());
    snapshot.push('\n');
    snapshot
}

fn append_snapshot_statement(snapshot: &mut String, statement: &str) {
    snapshot.push_str(statement.trim());
    snapshot.push_str(";\n\n");
}

fn access_catalog_snapshot_statements() -> Result<Vec<String>, DbErr> {
    let permissions = access_permission_codes()?;
    let permission_names = access_permission_names()?;
    let permission_rows = permissions
        .iter()
        .enumerate()
        .map(|(index, code)| {
            let name = permission_names
                .get(code)
                .map(String::as_str)
                .unwrap_or(code.as_str());
            format!(
                "({}, 'system', {}, '{}', NULL, 'api', NULL, {}, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
                10_000 + index,
                seed_text(name),
                code,
                index
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let mut statements = vec![format!(
        "INSERT INTO `sys_permission` \
         (`id`, `tenant_id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`, `created_at`, `updated_at`) VALUES\n{permission_rows}\n\
         ON DUPLICATE KEY UPDATE `name` = IF(`name` = `code`, VALUES(`name`), `name`)"
    )];

    for (index, menu) in access_menus()?.iter().enumerate() {
        let permission_id = menu.permission.as_deref().map_or_else(
            || "NULL".to_owned(),
            |code| {
                format!(
                    "(SELECT `id` FROM `sys_permission` WHERE `tenant_id` = 'system' AND `code` = '{code}' LIMIT 1)"
                )
            },
        );
        let parent_id = menu.parent_route_key().map_or_else(
            || "NULL".to_owned(),
            |route_key| {
                format!(
                    "(SELECT `id` FROM `sys_menu` WHERE `tenant_id` = 'system' AND `route_key` = '{route_key}' LIMIT 1)"
                )
            },
        );
        let menu_name = seed_text(&menu.name);
        let icon = menu
            .icon()
            .map_or_else(|| "NULL".to_owned(), |icon| format!("'{icon}'"));
        statements.push(format!(
            "INSERT INTO `sys_menu` \
             (`id`, `tenant_id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`, `remark`, `del_flag`, `created_at`, `updated_at`) \
             SELECT {}, 'system', {}, {}, '{}', {}, '{}', {}, {}, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6) \
             WHERE NOT EXISTS (SELECT 1 FROM `sys_menu` WHERE `tenant_id` = 'system' AND `route_key` = '{}')",
            20_000 + index,
            menu_name,
            parent_id,
            menu.menu_type,
            permission_id,
            menu.route_key,
            icon,
            menu.sort(),
            menu.route_key,
        ));
        let parent_route = menu.parent_route_key().map_or_else(
            || "NULL".to_owned(),
            |route| format!("'{}'", route.replace('\'', "''")),
        );
        statements.push(format!(
            "UPDATE `sys_menu` AS `menu` \
             LEFT JOIN `sys_menu` AS `parent` ON `parent`.`tenant_id` = 'system' AND `parent`.`route_key` = {parent_route} \
             SET `menu`.`name` = IF(`menu`.`name` = `menu`.`route_key`, {}, `menu`.`name`), \
             `menu`.`parent_id` = `parent`.`id`, `menu`.`menu_type` = '{}', `menu`.`perm_id` = {}, \
             `menu`.`icon` = COALESCE({}, `menu`.`icon`), `menu`.`sort` = {}, `menu`.`status` = '1', `menu`.`del_flag` = '0' \
             WHERE `menu`.`tenant_id` = 'system' AND `menu`.`route_key` = '{}'",
            menu_name,
            menu.menu_type,
            permission_id,
            icon,
            menu.sort(),
            menu.route_key,
        ));
    }
    Ok(statements)
}

/// DML 名称使用模式无关的表达式；普通名称保持审阅快照的既有排版。
fn seed_text(value: &str) -> String {
    use std::fmt::Write;
    if value.contains('\\') || value.chars().any(char::is_control) {
        let mut hex = String::with_capacity(value.len() * 2);
        for byte in value.as_bytes() {
            write!(hex, "{byte:02x}").expect("字符串写入不会失败");
        }
        format!("CONVERT(0x{hex} USING utf8mb4)")
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}
