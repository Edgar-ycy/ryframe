use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TryGetable};

use crate::migration::m20260820_000000_control_baseline::{ddl_statements, seed_statements};

/// 插入规范的引导记录，而不覆盖运行中的变更。
///
/// 重复键使用显式的无操作 upsert，绝不压制无关 SQL 错误。每个规范语义标识和关系
/// 都会在写入后校验，因此冲突的主键或唯一键无法伪装成成功的引导操作。
pub async fn seed<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if db.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("RyFrame v0.5 only supports MySQL".into()));
    }
    for statement in seed_statements() {
        db.execute_unprepared(&idempotent_upsert(statement)?)
            .await?;
    }
    seed_access_catalog(db).await?;
    seed_product_baseline(db).await?;
    seed_tenant_data_placements(db).await?;
    seed_retention_schedule(db).await?;
    verify_seed_identities(db).await?;
    verify_seed_relationships(db).await
}

const ACCESS_CATALOG: &str = include_str!("../../../../catalog/access.toml");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessMenu<'a> {
    pub route_key: &'a str,
    pub name: &'a str,
    pub menu_type: &'a str,
    pub permission: Option<&'a str>,
}

impl AccessMenu<'_> {
    pub fn parent_route_key(&self) -> Option<&str> {
        (self.menu_type == "C")
            .then(|| self.route_key.split_once('.').map(|(parent, _)| parent))
            .flatten()
    }
}

async fn seed_access_catalog<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let permissions = access_permission_codes()?;
    for (index, code) in permissions.iter().enumerate() {
        let index = i32::try_from(index)
            .map_err(|_| DbErr::Custom("访问目录权限数量超出基线可表示范围".into()))?;
        let id = 10_000_i64 + i64::from(index);
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO `sys_permission` \
             (`id`, `tenant_id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`, `created_at`, `updated_at`) \
             VALUES (?, 'system', ?, ?, NULL, 'api', NULL, ?, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
             ON DUPLICATE KEY UPDATE `code` = `code`",
            [id.into(), (*code).into(), (*code).into(), index.into()],
        ))
        .await?;
    }

    for (index, menu) in access_menus()?.iter().enumerate() {
        let index = i32::try_from(index)
            .map_err(|_| DbErr::Custom("访问目录菜单数量超出基线可表示范围".into()))?;
        let permission_id = match menu.permission {
            Some(code) => permission_id(db, code).await?,
            None => None,
        };
        let parent_id = match menu.parent_route_key() {
            Some(route_key) => menu_id(db, route_key).await?,
            None => None,
        };
        if let Some(id) = menu_id(db, menu.route_key).await? {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE `sys_menu` SET `name` = IF(`name` = `route_key`, ?, `name`), \
                 `parent_id` = ?, `menu_type` = ?, `perm_id` = ?, `status` = '1', \
                 `del_flag` = '0', `updated_at` = UTC_TIMESTAMP(6) \
                 WHERE `id` = ? AND `tenant_id` = 'system'",
                [
                    menu.name.into(),
                    parent_id.into(),
                    menu.menu_type.into(),
                    permission_id.into(),
                    id.into(),
                ],
            ))
            .await?;
            continue;
        }
        let id = 20_000_i64 + i64::from(index);
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO `sys_menu` \
             (`id`, `tenant_id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`, `remark`, `del_flag`, `created_at`, `updated_at`) \
             VALUES (?, 'system', ?, ?, ?, ?, ?, NULL, ?, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
            [
                id.into(),
                menu.name.into(),
                parent_id.into(),
                menu.menu_type.into(),
                permission_id.into(),
                menu.route_key.into(),
                index.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

pub fn access_permission_codes() -> Result<Vec<&'static str>, DbErr> {
    let mut values = Vec::new();
    let mut inside = false;
    for line in ACCESS_CATALOG.lines().map(str::trim) {
        if line == "permissions = [" && !inside {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line == "]" {
            break;
        }
        let value = line.trim_end_matches(',').trim_matches('"');
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":-*._".contains(&byte))
        {
            return Err(DbErr::Custom("访问目录包含非法权限编码，拒绝初始化".into()));
        }
        values.push(value);
    }
    if values.is_empty() {
        return Err(DbErr::Custom("访问目录没有权限定义".into()));
    }
    Ok(values)
}

pub fn access_menus() -> Result<Vec<AccessMenu<'static>>, DbErr> {
    let mut menus = Vec::new();
    let mut current = None;
    for line in ACCESS_CATALOG.lines().map(str::trim) {
        if line == "[[menus]]" {
            if let Some(menu) = current.take() {
                menus.push(menu);
            }
            current = Some(AccessMenu {
                route_key: "",
                name: "",
                menu_type: "",
                permission: None,
            });
            continue;
        }
        if line.starts_with("[[") {
            if let Some(menu) = current.take() {
                menus.push(menu);
            }
            continue;
        }
        let Some(menu) = current.as_mut() else {
            continue;
        };
        if let Some(value) = catalog_string_value(line, "route_key") {
            menu.route_key = value;
        } else if let Some(value) = catalog_string_value(line, "name") {
            menu.name = value;
        } else if let Some(value) = catalog_string_value(line, "menu_type") {
            menu.menu_type = value;
        } else if let Some(value) = catalog_string_value(line, "permission") {
            menu.permission = Some(value);
        }
    }
    if let Some(menu) = current {
        menus.push(menu);
    }
    if menus.iter().any(|menu| {
        menu.route_key.is_empty()
            || menu.name.is_empty()
            || menu.name.chars().count() > 64
            || !matches!(menu.menu_type, "M" | "C")
            || !menu
                .route_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
    }) {
        return Err(DbErr::Custom("访问目录包含非法菜单定义".into()));
    }
    Ok(menus)
}

fn catalog_string_value(line: &'static str, key: &str) -> Option<&'static str> {
    let value = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    value.strip_prefix('"')?.strip_suffix('"')
}

async fn permission_id<C>(db: &C, code: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `id` FROM `sys_permission` \
             WHERE `tenant_id` = 'system' AND `code` = ? LIMIT 1",
            [code.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}

async fn menu_id<C>(db: &C, route_key: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `id` FROM `sys_menu` \
             WHERE `tenant_id` = 'system' AND `route_key` = ? LIMIT 1",
            [route_key.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}

async fn seed_product_baseline<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    for statement in PRODUCT_SEED_STATEMENTS {
        db.execute_unprepared(statement).await?;
    }
    Ok(())
}

const PRODUCT_SEED_STATEMENTS: &[&str] = &[
    "INSERT INTO `sys_product_plan` \
         (`id`, `plan_key`, `name`, `description`, `status`, `created_by`, `created_at`, `updated_at`) VALUES \
         (1, 'standard', '标准版', '普通租户的默认产品套餐', '1', 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (2, 'platform', '平台版', '系统租户的平台控制面套餐', '1', 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE `id` = `id`",
    "INSERT INTO `sys_product_plan_version` \
         (`id`, `plan_id`, `version`, `name`, `description`, `status`, `created_by`, `published_by`, `published_at`, `created_at`, `updated_at`) VALUES \
         (1, 1, 1, '标准版 v1', '标准版初始能力集合', 'published', 1, 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (2, 2, 1, '平台版 v1', '平台控制面初始能力集合', 'published', 1, 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE `id` = `id`",
    "INSERT INTO `sys_product_plan_capability` \
         (`plan_version_id`, `capability_code`, `variant_code`, `schema_version`, `config`, `created_at`, `updated_at`) VALUES \
         (2, 'system.service_accounts', 'default', 1, JSON_OBJECT(), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE `plan_version_id` = `plan_version_id`",
    "INSERT INTO `sys_tenant_product_plan` \
         (`tenant_id`, `plan_version_id`, `changed_by`, `change_reason`, `created_at`, `updated_at`) \
         SELECT `tenant_id`, IF(`tenant_id` = 'system', 2, 1), NULL, 'fresh_baseline', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6) \
         FROM `sys_tenant` ON DUPLICATE KEY UPDATE \
         `tenant_id` = `sys_tenant_product_plan`.`tenant_id`",
];

const TENANT_PLACEMENT_SEED_STATEMENT: &str = "INSERT INTO `sys_tenant_data_placement` \
    (`tenant_id`, `current_target_key`, `placement_generation`, `state`, `switch_token`, `created_at`, `updated_at`) \
    SELECT `tenant_id`, 'shared-control', 1, 'active', \
           SHA2(CONCAT('ryframe:tenant-data:shared-control:v1:', `tenant_id`), 256), \
           UTC_TIMESTAMP(6), UTC_TIMESTAMP(6) FROM `sys_tenant` \
    ON DUPLICATE KEY UPDATE \
    `tenant_id` = `sys_tenant_data_placement`.`tenant_id`";

const RETENTION_SCHEDULE_SEED_STATEMENT: &str = "INSERT INTO `sys_job_schedule` \
    (`id`, `tenant_id`, `name`, `handler_key`, `cron_expression`, `timezone`, `enabled`, `misfire_policy`, `concurrency_policy`, `max_runtime_seconds`, `next_run_at`, `last_run_at`, `version`, `del_flag`, `created_at`, `updated_at`) VALUES \
    (3, 'system', '数据保留清理', 'system.data_retention_cleanup', '0 30 3 * * * *', 'UTC', 1, 'fire_once', 'forbid', 900, UTC_TIMESTAMP(6), NULL, 1, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
    ON DUPLICATE KEY UPDATE `id` = `id`";

async fn seed_tenant_data_placements<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    db.execute_unprepared(TENANT_PLACEMENT_SEED_STATEMENT)
        .await?;
    Ok(())
}

async fn seed_retention_schedule<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    db.execute_unprepared(RETENTION_SCHEDULE_SEED_STATEMENT)
        .await?;
    Ok(())
}

pub fn mysql_snapshot_sql() -> String {
    let mut snapshot = format!(
        "-- 自动生成文件：RyFrame 控制库新基线快照。\n\
         -- schema fingerprint: {}\n\
         -- 唯一事实来源：ryframe-db::migration Migrator 与 Seeder。\n\
         -- 仅供审阅：部署和重置工具不得执行此文件。\n\
         -- 重新生成命令：cargo run -p ryframe-db --bin export_mysql_snapshot -- sql/ryframe_config.sql\n\n",
        crate::migration::m20260820_000000_control_baseline::schema_fingerprint()
    );
    for statement in ddl_statements() {
        snapshot.push_str(statement.trim());
        snapshot.push_str(";\n\n");
    }
    snapshot.push_str("-- 幂等初始化数据（生产环境用户默认锁定）。\n\n");
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
    let permission_rows = permissions
        .iter()
        .enumerate()
        .map(|(index, code)| {
            format!(
                "({}, 'system', '{}', '{}', NULL, 'api', NULL, {}, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
                10_000 + index,
                code,
                code,
                index
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let mut statements = vec![format!(
        "INSERT INTO `sys_permission` \
         (`id`, `tenant_id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`, `created_at`, `updated_at`) VALUES\n{permission_rows}\n\
         ON DUPLICATE KEY UPDATE `code` = `code`"
    )];

    for (index, menu) in access_menus()?.iter().enumerate() {
        let permission_id = menu.permission.map_or_else(
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
        let menu_name = menu.name.replace('\'', "''");
        statements.push(format!(
            "INSERT INTO `sys_menu` \
             (`id`, `tenant_id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`, `remark`, `del_flag`, `created_at`, `updated_at`) \
             SELECT {}, 'system', '{}', {}, '{}', {}, '{}', NULL, {}, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6) \
             WHERE NOT EXISTS (SELECT 1 FROM `sys_menu` WHERE `tenant_id` = 'system' AND `route_key` = '{}')",
            20_000 + index,
            menu_name,
            parent_id,
            menu.menu_type,
            permission_id,
            menu.route_key,
            index,
            menu.route_key,
        ));
        statements.push(format!(
            "UPDATE `sys_menu` SET `name` = IF(`name` = `route_key`, '{}', `name`), `parent_id` = {}, `menu_type` = '{}', `perm_id` = {}, `status` = '1', `del_flag` = '0' \
             WHERE `tenant_id` = 'system' AND `route_key` = '{}'",
            menu_name, parent_id, menu.menu_type, permission_id, menu.route_key,
        ));
    }
    Ok(statements)
}

#[derive(Debug)]
struct SeedInsert {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn idempotent_upsert(statement: &str) -> Result<String, DbErr> {
    let parsed = parse_seed_insert(statement)?;
    let no_op_column = parsed.columns.first().ok_or_else(|| {
        DbErr::Custom(format!(
            "canonical seed for {} has no columns",
            parsed.table
        ))
    })?;
    Ok(format!(
        "{} ON DUPLICATE KEY UPDATE `{no_op_column}` = `{no_op_column}`",
        statement.trim()
    ))
}

async fn verify_seed_identities<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    for statement in seed_statements() {
        let insert = parse_seed_insert(statement)?;
        let identity_columns = seed_identity_columns(&insert.table).ok_or_else(|| {
            DbErr::Custom(format!(
                "canonical seed table {} has no identity verification specification",
                insert.table
            ))
        })?;

        for row in &insert.rows {
            let mut predicates = Vec::with_capacity(identity_columns.len() + 1);
            for identity_column in identity_columns {
                let position = insert
                    .columns
                    .iter()
                    .position(|column| column == identity_column)
                    .ok_or_else(|| {
                        DbErr::Custom(format!(
                            "canonical seed for {} omits identity column {}",
                            insert.table, identity_column
                        ))
                    })?;
                predicates.push(format!(
                    "`{identity_column}` <=> {}",
                    row.get(position).ok_or_else(|| {
                        DbErr::Custom(format!(
                            "canonical seed for {} has a malformed value tuple",
                            insert.table
                        ))
                    })?
                ));
            }
            if insert.table != "sys_tenant" && !identity_columns.contains(&"tenant_id") {
                predicates.push("`tenant_id` <=> 'system'".into());
            }
            let sql = format!(
                "SELECT COUNT(*) FROM `{}` WHERE {}",
                insert.table,
                predicates.join(" AND ")
            );
            let row = db
                .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
                .await?
                .ok_or_else(|| {
                    DbErr::Custom(format!(
                        "seed identity verification returned no row for {}",
                        insert.table
                    ))
                })?;
            let count = i64::try_get_by_index(&row, 0)?;
            if count != 1 {
                return Err(DbErr::Custom(format!(
                    "canonical seed identity is missing or conflicting in {}: expected one row matching {}; refusing startup",
                    insert.table,
                    predicates.join(" AND ")
                )));
            }
        }
    }
    Ok(())
}

async fn verify_seed_relationships<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    const RELATIONSHIPS: &[(&str, &str)] = &[
        (
            "SELECT COUNT(*) FROM sys_user_role ur \
             LEFT JOIN sys_user u ON u.id = ur.user_id AND u.tenant_id = ur.tenant_id \
             LEFT JOIN sys_role r ON r.id = ur.role_id AND r.tenant_id = ur.tenant_id \
             WHERE ur.tenant_id = 'system' AND (u.id IS NULL OR r.id IS NULL)",
            "system user-role bindings",
        ),
        (
            "SELECT COUNT(*) FROM sys_role_permission rp \
             LEFT JOIN sys_role r ON r.id = rp.role_id AND r.tenant_id = rp.tenant_id \
             LEFT JOIN sys_permission p ON p.id = rp.perm_id AND p.tenant_id = rp.tenant_id \
             WHERE rp.tenant_id = 'system' AND (r.id IS NULL OR p.id IS NULL)",
            "system role-permission bindings",
        ),
        (
            "SELECT COUNT(*) FROM sys_menu m \
             LEFT JOIN sys_menu parent ON parent.id = m.parent_id AND parent.tenant_id = m.tenant_id \
             LEFT JOIN sys_permission p ON p.id = m.perm_id AND p.tenant_id = m.tenant_id \
             WHERE m.tenant_id = 'system' \
               AND ((m.parent_id IS NOT NULL AND parent.id IS NULL) \
                 OR (m.perm_id IS NOT NULL AND p.id IS NULL))",
            "system menu hierarchy and permissions",
        ),
        (
            "SELECT COUNT(*) FROM sys_permission p \
             LEFT JOIN sys_permission parent \
               ON parent.id = p.parent_id AND parent.tenant_id = p.tenant_id \
             WHERE p.tenant_id = 'system' \
               AND p.parent_id IS NOT NULL AND parent.id IS NULL",
            "system permission hierarchy",
        ),
        (
            "SELECT COUNT(*) FROM sys_dept d \
             LEFT JOIN sys_dept parent ON parent.id = d.parent_id AND parent.tenant_id = d.tenant_id \
             WHERE d.tenant_id = 'system' \
               AND d.parent_id IS NOT NULL AND parent.id IS NULL",
            "system department hierarchy",
        ),
    ];

    for (sql, label) in RELATIONSHIPS {
        let row = db
            .query_one_raw(Statement::from_string(DbBackend::MySql, (*sql).to_owned()))
            .await?
            .ok_or_else(|| {
                DbErr::Custom(format!(
                    "seed relationship verification returned no row for {label}"
                ))
            })?;
        let violations = i64::try_get_by_index(&row, 0)?;
        if violations != 0 {
            return Err(DbErr::Custom(format!(
                "seed relationship verification failed for {label}: found {violations} invalid rows; refusing startup"
            )));
        }
    }
    Ok(())
}

fn seed_identity_columns(table: &str) -> Option<&'static [&'static str]> {
    match table {
        "sys_tenant" => Some(&["id", "tenant_id"]),
        "sys_cache_namespace_version" => Some(&["tenant_id", "namespace"]),
        "sys_dept" => Some(&["id", "parent_id"]),
        "sys_role" => Some(&["id", "code"]),
        "sys_user" => Some(&["id", "username"]),
        "sys_permission" => Some(&["id", "code", "parent_id"]),
        "sys_menu" => Some(&["id", "parent_id", "menu_type", "perm_id", "route_key"]),
        "sys_post" => Some(&["id", "code"]),
        "sys_config" => Some(&["id", "key"]),
        "sys_dict_type" => Some(&["id", "code"]),
        "sys_dict_data" => Some(&["id", "type_code", "value"]),
        "sys_user_role" => Some(&["user_id", "role_id"]),
        "sys_role_permission" => Some(&["role_id", "perm_id"]),
        "sys_job_schedule" => Some(&["id"]),
        _ => None,
    }
}

fn parse_seed_insert(statement: &str) -> Result<SeedInsert, DbErr> {
    let statement = statement.trim();
    if !statement.to_ascii_uppercase().starts_with("INSERT INTO") {
        return Err(DbErr::Custom(
            "canonical seed contains a non-INSERT statement".into(),
        ));
    }
    let identifiers = backtick_identifiers(statement);
    let table = identifiers
        .first()
        .cloned()
        .ok_or_else(|| DbErr::Custom("canonical seed INSERT is missing a table name".into()))?;

    let open = statement.find('(').ok_or_else(|| {
        DbErr::Custom(format!(
            "canonical seed for {table} is missing a column list"
        ))
    })?;
    let close = statement[open + 1..]
        .find(')')
        .map(|index| open + index + 1)
        .ok_or_else(|| {
            DbErr::Custom(format!(
                "canonical seed for {table} has an unterminated column list"
            ))
        })?;
    let columns = backtick_identifiers(&statement[open..=close]);
    let after_columns = &statement[close + 1..];
    let values_at = after_columns
        .to_ascii_uppercase()
        .find("VALUES")
        .ok_or_else(|| DbErr::Custom(format!("canonical seed for {table} is missing VALUES")))?;
    let rows = split_value_rows(&after_columns[values_at + "VALUES".len()..])?;
    if columns.is_empty() || rows.is_empty() {
        return Err(DbErr::Custom(format!(
            "canonical seed for {table} has no columns or rows"
        )));
    }
    if rows.iter().any(|row| row.len() != columns.len()) {
        return Err(DbErr::Custom(format!(
            "canonical seed for {table} has a value count that does not match its columns"
        )));
    }
    Ok(SeedInsert {
        table,
        columns,
        rows,
    })
}

fn split_value_rows(value: &str) -> Result<Vec<Vec<String>>, DbErr> {
    let mut rows = Vec::new();
    let mut row_start = None;
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut characters = value.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
                continue;
            }
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match character {
            '(' => {
                if depth == 0 {
                    row_start = Some(index + 1);
                }
                depth += 1;
            }
            ')' => {
                if depth == 0 {
                    return Err(DbErr::Custom(
                        "canonical seed has an unmatched closing parenthesis".into(),
                    ));
                }
                depth -= 1;
                if depth == 0 {
                    let start = row_start
                        .take()
                        .ok_or_else(|| DbErr::Custom("canonical seed row has no start".into()))?;
                    rows.push(split_row_values(&value[start..index])?);
                }
            }
            _ => {}
        }
    }
    if quoted || depth != 0 {
        return Err(DbErr::Custom(
            "canonical seed has an unterminated string or value tuple".into(),
        ));
    }
    Ok(rows)
}

fn split_row_values(row: &str) -> Result<Vec<String>, DbErr> {
    let mut values = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut characters = row.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
                continue;
            }
            quoted = !quoted;
        } else if character == ',' && !quoted {
            values.push(row[start..index].trim().to_owned());
            start = index + 1;
        }
    }
    if quoted {
        return Err(DbErr::Custom(
            "canonical seed row has an unterminated string".into(),
        ));
    }
    values.push(row[start..].trim().to_owned());
    Ok(values)
}

fn backtick_identifiers(value: &str) -> Vec<String> {
    value
        .split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, identifier)| identifier.to_owned())
        .collect()
}

pub fn validate_seed_statements() -> Result<(), DbErr> {
    for statement in seed_statements() {
        let parsed = parse_seed_insert(statement)?;
        if parsed.table.is_empty()
            || parsed.rows.is_empty()
            || parsed
                .rows
                .iter()
                .any(|row| row.len() != parsed.columns.len())
        {
            return Err(DbErr::Custom("控制库基线种子结构不完整".into()));
        }
    }
    Ok(())
}
