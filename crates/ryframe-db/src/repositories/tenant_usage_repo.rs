use chrono::{DateTime, Duration, Utc};
use ryframe_core::{PageResult, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, FromQueryResult, QueryResult, Statement, Value,
};

use crate::entities::tenant;

const TENANT_USAGE_JOINS: &str = r#"
LEFT JOIN (
    SELECT `tenant_id`, COUNT(*) AS `used_users`
    FROM `sys_user`
    WHERE `del_flag` = '0'
    GROUP BY `tenant_id`
) AS `users` ON `users`.`tenant_id` = `tenant`.`tenant_id`
LEFT JOIN (
    SELECT `tenant_id`, COUNT(*) AS `used_roles`
    FROM `sys_role`
    WHERE `del_flag` = '0'
    GROUP BY `tenant_id`
) AS `roles` ON `roles`.`tenant_id` = `tenant`.`tenant_id`
LEFT JOIN (
    SELECT
        `tenant_id`,
        CAST(COALESCE(SUM(CASE WHEN `file_size` > 0 THEN `file_size` ELSE 0 END), 0) AS SIGNED) AS `used_storage_bytes`
    FROM `sys_file`
    WHERE `del_flag` = '0'
    GROUP BY `tenant_id`
) AS `files` ON `files`.`tenant_id` = `tenant`.`tenant_id`
"#;

const CAPACITY_STATUS_SQL: &str = r#"
CASE
    WHEN `tenant`.`max_users` = 0
        AND `tenant`.`max_roles` = 0
        AND `tenant`.`max_storage_mb` = 0
    THEN 'unlimited'
    WHEN (`tenant`.`max_users` > 0 AND COALESCE(`users`.`used_users`, 0) >= `tenant`.`max_users`)
        OR (`tenant`.`max_roles` > 0 AND COALESCE(`roles`.`used_roles`, 0) >= `tenant`.`max_roles`)
        OR (
            `tenant`.`max_storage_mb` > 0
            AND COALESCE(`files`.`used_storage_bytes`, 0)
                >= CAST(`tenant`.`max_storage_mb` AS DECIMAL(30, 4)) * 1048576
        )
    THEN 'exceeded'
    WHEN (`tenant`.`max_users` > 0 AND CAST(COALESCE(`users`.`used_users`, 0) AS DECIMAL(38, 0)) * 100 >= CAST(`tenant`.`max_users` AS DECIMAL(38, 0)) * 90)
        OR (`tenant`.`max_roles` > 0 AND CAST(COALESCE(`roles`.`used_roles`, 0) AS DECIMAL(38, 0)) * 100 >= CAST(`tenant`.`max_roles` AS DECIMAL(38, 0)) * 90)
        OR (
            `tenant`.`max_storage_mb` > 0
            AND CAST(COALESCE(`files`.`used_storage_bytes`, 0) AS DECIMAL(38, 0)) * 100
                >= CAST(`tenant`.`max_storage_mb` AS DECIMAL(38, 0)) * 1048576 * 90
        )
    THEN 'critical'
    WHEN (`tenant`.`max_users` > 0 AND CAST(COALESCE(`users`.`used_users`, 0) AS DECIMAL(38, 0)) * 100 >= CAST(`tenant`.`max_users` AS DECIMAL(38, 0)) * 80)
        OR (`tenant`.`max_roles` > 0 AND CAST(COALESCE(`roles`.`used_roles`, 0) AS DECIMAL(38, 0)) * 100 >= CAST(`tenant`.`max_roles` AS DECIMAL(38, 0)) * 80)
        OR (
            `tenant`.`max_storage_mb` > 0
            AND CAST(COALESCE(`files`.`used_storage_bytes`, 0) AS DECIMAL(38, 0)) * 100
                >= CAST(`tenant`.`max_storage_mb` AS DECIMAL(38, 0)) * 1048576 * 80
        )
    THEN 'warning'
    ELSE 'normal'
END
"#;

#[derive(Clone, Debug, Default)]
pub struct TenantUsagePageFilter<'a> {
    pub tenant_id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub expiration_status: Option<&'a str>,
    pub capacity_status: Option<&'a str>,
}

#[derive(Clone, Debug, Default)]
pub struct TenantUsageAggregate {
    pub tenant_id: String,
    pub users: u64,
    pub roles: u64,
    pub storage_bytes: u64,
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub dead_jobs: u64,
    pub enabled_schedules: u64,
    pub active_user_imports: u64,
}

/// 平台租户容量查询仓储。
///
/// 分页查询只确定租户范围，随后使用一条聚合语句读取当前页的资源与辅助用量，
/// 查询次数不会随当前页租户数量增长。
pub struct TenantUsageRepository;

impl TenantUsageRepository {
    pub async fn page(
        &self,
        db: &DatabaseConnection,
        filter: TenantUsagePageFilter<'_>,
        page: &ValidatedPageQuery,
        calculated_at: DateTime<Utc>,
    ) -> AppResult<PageResult<tenant::Model>> {
        let needs_usage_join = filter.capacity_status.is_some();
        let usage_joins = if needs_usage_join {
            TENANT_USAGE_JOINS
        } else {
            ""
        };
        let (where_sql, values) = page_filter_sql(filter, calculated_at);
        let count_sql = format!(
            "SELECT COUNT(*) AS `total` FROM `sys_tenant` AS `tenant` {usage_joins} {where_sql}"
        );
        let total_row = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                count_sql,
                values.clone(),
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("租户容量分页统计没有返回记录".into()))?;
        let total = read_count(&total_row, "total")?;

        let mut record_values = values;
        record_values.push(Value::from(page.page_size()));
        record_values.push(Value::from(page.offset()));
        let record_sql = format!(
            r#"
SELECT `tenant`.*
FROM `sys_tenant` AS `tenant`
{usage_joins}
{where_sql}
ORDER BY `tenant`.`tenant_id` ASC
LIMIT ? OFFSET ?
"#
        );
        let records = db
            .query_all_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                record_sql,
                record_values,
            ))
            .await
            .map_err(database_error)?
            .into_iter()
            .map(|row| tenant::Model::from_query_result(&row, "").map_err(database_error))
            .collect::<AppResult<Vec<_>>>()?;
        Ok(PageResult::new(records, total, page))
    }

    pub async fn aggregate_for_tenants(
        &self,
        db: &DatabaseConnection,
        tenant_ids: &[String],
    ) -> AppResult<Vec<TenantUsageAggregate>> {
        if tenant_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; tenant_ids.len()].join(", ");
        let scoped_usage_joins = scoped_usage_joins(&placeholders);
        let sql = format!(
            r#"
SELECT
    `tenant`.`tenant_id`,
    COALESCE(`users`.`used_users`, 0) AS `used_users`,
    COALESCE(`roles`.`used_roles`, 0) AS `used_roles`,
    COALESCE(`files`.`used_storage_bytes`, 0) AS `used_storage_bytes`,
    COALESCE(`jobs`.`pending_jobs`, 0) AS `pending_jobs`,
    COALESCE(`jobs`.`running_jobs`, 0) AS `running_jobs`,
    COALESCE(`jobs`.`dead_jobs`, 0) AS `dead_jobs`,
    COALESCE(`schedules`.`enabled_schedules`, 0) AS `enabled_schedules`,
    COALESCE(`imports`.`active_user_imports`, 0) AS `active_user_imports`
FROM `sys_tenant` AS `tenant`
{scoped_usage_joins}
LEFT JOIN (
    SELECT
        `tenant_id`,
        CAST(SUM(CASE WHEN `status` = 'pending' THEN 1 ELSE 0 END) AS SIGNED) AS `pending_jobs`,
        CAST(SUM(CASE WHEN `status` = 'running' THEN 1 ELSE 0 END) AS SIGNED) AS `running_jobs`,
        CAST(SUM(CASE WHEN `status` = 'dead' THEN 1 ELSE 0 END) AS SIGNED) AS `dead_jobs`
    FROM `sys_background_job`
    WHERE `tenant_id` IN ({placeholders}) AND `status` IN ('pending', 'running', 'dead')
    GROUP BY `tenant_id`
) AS `jobs` ON `jobs`.`tenant_id` = `tenant`.`tenant_id`
LEFT JOIN (
    SELECT `tenant_id`, COUNT(*) AS `enabled_schedules`
    FROM `sys_job_schedule`
    WHERE `enabled` = 1 AND `del_flag` = '0' AND `tenant_id` IN ({placeholders})
    GROUP BY `tenant_id`
) AS `schedules` ON `schedules`.`tenant_id` = `tenant`.`tenant_id`
LEFT JOIN (
    SELECT `tenant_id`, COUNT(*) AS `active_user_imports`
    FROM `sys_user_import_job`
    WHERE `status` IN ('pending', 'running') AND `tenant_id` IN ({placeholders})
    GROUP BY `tenant_id`
) AS `imports` ON `imports`.`tenant_id` = `tenant`.`tenant_id`
WHERE `tenant`.`tenant_id` IN ({placeholders})
ORDER BY `tenant`.`tenant_id` ASC
"#
        );
        let values: Vec<Value> = (0..7)
            .flat_map(|_| tenant_ids.iter().cloned().map(Value::from))
            .collect();
        db.query_all_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            sql,
            values,
        ))
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(TenantUsageAggregate {
                tenant_id: row.try_get("", "tenant_id").map_err(database_error)?,
                users: read_count(&row, "used_users")?,
                roles: read_count(&row, "used_roles")?,
                storage_bytes: read_count(&row, "used_storage_bytes")?,
                pending_jobs: read_count(&row, "pending_jobs")?,
                running_jobs: read_count(&row, "running_jobs")?,
                dead_jobs: read_count(&row, "dead_jobs")?,
                enabled_schedules: read_count(&row, "enabled_schedules")?,
                active_user_imports: read_count(&row, "active_user_imports")?,
            })
        })
        .collect()
    }
}

fn scoped_usage_joins(placeholders: &str) -> String {
    format!(
        r#"
LEFT JOIN (
    SELECT `tenant_id`, COUNT(*) AS `used_users`
    FROM `sys_user`
    WHERE `del_flag` = '0' AND `tenant_id` IN ({placeholders})
    GROUP BY `tenant_id`
) AS `users` ON `users`.`tenant_id` = `tenant`.`tenant_id`
LEFT JOIN (
    SELECT `tenant_id`, COUNT(*) AS `used_roles`
    FROM `sys_role`
    WHERE `del_flag` = '0' AND `tenant_id` IN ({placeholders})
    GROUP BY `tenant_id`
) AS `roles` ON `roles`.`tenant_id` = `tenant`.`tenant_id`
LEFT JOIN (
    SELECT
        `tenant_id`,
        CAST(COALESCE(SUM(CASE WHEN `file_size` > 0 THEN `file_size` ELSE 0 END), 0) AS SIGNED) AS `used_storage_bytes`
    FROM `sys_file`
    WHERE `del_flag` = '0' AND `tenant_id` IN ({placeholders})
    GROUP BY `tenant_id`
) AS `files` ON `files`.`tenant_id` = `tenant`.`tenant_id`
"#
    )
}

fn page_filter_sql(
    filter: TenantUsagePageFilter<'_>,
    calculated_at: DateTime<Utc>,
) -> (String, Vec<Value>) {
    let mut conditions = Vec::<String>::with_capacity(5);
    let mut values = Vec::with_capacity(5);
    if let Some(tenant_id) = filter.tenant_id.filter(|value| !value.is_empty()) {
        conditions.push("`tenant`.`tenant_id` LIKE ? ESCAPE '!'".to_owned());
        values.push(Value::from(contains_like(tenant_id)));
    }
    if let Some(name) = filter.name.filter(|value| !value.is_empty()) {
        conditions.push("`tenant`.`name` LIKE ? ESCAPE '!'".to_owned());
        values.push(Value::from(contains_like(name)));
    }
    if let Some(status) = filter.status {
        conditions.push("`tenant`.`status` = ?".to_owned());
        values.push(Value::from(status.to_owned()));
    }
    match filter.expiration_status {
        Some("active") => {
            conditions.push("`tenant`.`expire_at` > ?".to_owned());
            values.push(Value::from(calculated_at + Duration::days(30)));
        }
        Some("expiring") => {
            conditions.push("`tenant`.`expire_at` > ? AND `tenant`.`expire_at` <= ?".to_owned());
            values.push(Value::from(calculated_at));
            values.push(Value::from(calculated_at + Duration::days(30)));
        }
        Some("expired") => {
            conditions
                .push("`tenant`.`expire_at` IS NOT NULL AND `tenant`.`expire_at` <= ?".to_owned());
            values.push(Value::from(calculated_at));
        }
        Some("never") => conditions.push("`tenant`.`expire_at` IS NULL".to_owned()),
        _ => {}
    }
    if let Some(capacity_status) = filter.capacity_status {
        conditions.push(format!("({CAPACITY_STATUS_SQL}) = ?"));
        values.push(Value::from(capacity_status.to_owned()));
    }
    if conditions.is_empty() {
        (String::new(), values)
    } else {
        (format!("WHERE {}", conditions.join(" AND ")), values)
    }
}

fn contains_like(value: &str) -> String {
    let escaped = value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    format!("%{escaped}%")
}

fn read_count(row: &QueryResult, column: &str) -> AppResult<u64> {
    let value: i64 = row.try_get("", column).map_err(database_error)?;
    u64::try_from(value)
        .map_err(|_| AppError::Database(format!("租户容量统计字段 {column} 返回了负数")))
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
