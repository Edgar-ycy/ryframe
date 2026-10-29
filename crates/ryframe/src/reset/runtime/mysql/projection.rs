//! 重建仅保留 seeder 为每个租户登记的未投递数据范围投影事件。
use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use crate::reset::{ResetError, ResetResult};

// 与 seeder 的 bootstrap 记录语义核对；不依赖表默认的 max_attempts=5。
const SEEDS: &str = "SELECT CAST(JSON_OBJECT(
    'tenant_id', t.tenant_id, 'source_version', t.data_scope_version,
    'payload', e.payload, 'pristine', CASE WHEN
        e.id > 0
        AND CAST(e.event_type AS BINARY) = CAST(? AS BINARY)
        AND CAST(e.aggregate_type AS BINARY) = CAST('tenant_data_scope' AS BINARY)
        AND CAST(e.aggregate_id AS BINARY) = CAST(t.tenant_id AS BINARY)
        AND CAST(e.dedupe_key AS BINARY) =
            CAST(CONCAT('bootstrap:', t.tenant_id, ':', t.data_scope_version) AS BINARY)
        AND CAST(e.status AS BINARY) = CAST('pending' AS BINARY)
        AND e.attempts = 0 AND e.max_attempts = 20
        AND e.lease_owner IS NULL AND e.lease_until IS NULL
        AND e.published_at IS NULL AND e.last_error IS NULL
        AND e.traceparent IS NULL AND e.tracestate IS NULL
        AND e.available_at = e.created_at AND e.updated_at = e.created_at
        AND YEAR(e.created_at) BETWEEN 1000 AND 9999
        AND MONTH(e.created_at) BETWEEN 1 AND 12
        AND DAYOFMONTH(e.created_at) BETWEEN 1 AND DAYOFMONTH(LAST_DAY(e.created_at))
    THEN 1 ELSE 0 END) AS CHAR) AS seed
    FROM sys_outbox_event e LEFT JOIN sys_tenant t
        ON CAST(e.tenant_id AS BINARY) = CAST(t.tenant_id AS BINARY)";

pub(super) async fn verify<C>(db: &C) -> ResetResult<()>
where
    C: ConnectionTrait + ?Sized,
{
    let count = super::operations::scalar_i64(db, "SELECT COUNT(*) FROM sys_tenant", [])
        .await?
        .ok_or_else(|| ResetError::new("控制库租户数量查询缺少结果"))?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            SEEDS,
            [ryframe_kernel::DATA_SCOPE_PROJECTION_OUTBOX_EVENT_TYPE.into()],
        ))
        .await
        .map_err(|_| ResetError::new("控制库初始化投影 outbox 查询失败"))?;
    let seeds = rows
        .iter()
        .map(|row| {
            let text: String = row
                .try_get("", "seed")
                .map_err(|_| ResetError::new("控制库初始化投影 outbox 查询格式无效"))?;
            serde_json::from_str(&text)
                .map_err(|_| ResetError::new("控制库初始化投影 outbox JSON 格式无效"))
        })
        .collect::<ResetResult<Vec<Value>>>()?;
    verify_rows(count, &seeds)
}

pub(super) fn verify_rows(tenant_count: i64, seeds: &[Value]) -> ResetResult<()> {
    let invalid = || {
        ResetError::new(
            "控制库 sys_outbox_event 不符合逐租户唯一初始化投影种子；保留现场，核对租户、版本、载荷及未投递状态，禁止清空事件或自动重放",
        )
    };
    if usize::try_from(tenant_count).ok() != Some(seeds.len()) {
        return Err(invalid());
    }
    let mut tenants = BTreeSet::new();
    for seed in seeds {
        let tenant = seed["tenant_id"].as_str().filter(|value| !value.is_empty());
        let version = seed["source_version"].as_i64().filter(|value| *value > 0);
        let (Some(tenant), Some(version)) = (tenant, version) else {
            return Err(invalid());
        };
        if !tenants.insert(tenant)
            || seed["pristine"] != 1
            || seed["payload"] != json!({"tenant_id":tenant,"source_version":version})
        {
            return Err(invalid());
        }
    }
    Ok(())
}
