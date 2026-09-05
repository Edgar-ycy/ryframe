// 等价性测试需要把冻结 baseline 作为独立只读事实源再次编译，禁止为消除 lint 修改冻结迁移。
#[cfg(any(not(feature = "migration"), test))]
#[allow(clippy::duplicate_mod)]
#[path = "m20260820_000000_control_baseline/base.rs"]
mod base;
// 与上方 base 模块相同，这里有意保留冻结 schema 的独立编译路径。
#[cfg(any(not(feature = "migration"), test))]
#[allow(clippy::duplicate_mod)]
#[path = "m20260820_000000_control_baseline/schema.rs"]
mod schema;

#[cfg(any(not(feature = "migration"), test))]
#[allow(clippy::duplicate_mod)]
#[path = "m20260820_000000_control_baseline/jobs.rs"]
mod jobs;

/// 当前控制库基线的只读 DDL 事实源，不编译迁移执行 trait。
#[cfg(any(not(feature = "migration"), test))]
pub fn ddl_statements() -> impl Iterator<Item = &'static str> {
    base::BASELINE_STATEMENTS
        .iter()
        .copied()
        .filter(|statement| !is_seed_statement(statement))
        .chain(schema::lifecycle_table_statements())
        .chain(schema::tenant_config_table_statements())
        .chain(schema::product_capability_table_statements())
        .chain(schema::tenant_data_control_table_statements())
        .chain([
            jobs::BACKGROUND_JOB_ATTEMPT_DDL,
            schema::OUTBOX_EVENT_DDL,
            schema::EXPORT_JOB_DDL,
            schema::RESOURCE_OWNERSHIP_DDL,
        ])
}

#[cfg(all(feature = "migration", not(test)))]
pub fn ddl_statements() -> impl Iterator<Item = &'static str> {
    super::m20260820_000000_control_baseline::ddl_statements()
}

#[cfg(any(not(feature = "migration"), test))]
pub fn schema_fingerprint() -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for statement in ddl_statements() {
        for byte in statement.trim().bytes().chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[cfg(all(feature = "migration", not(test)))]
pub fn schema_fingerprint() -> String {
    super::m20260820_000000_control_baseline::schema_fingerprint()
}

#[cfg(any(not(feature = "migration"), test))]
fn is_seed_statement(statement: &str) -> bool {
    statement.trim_start().starts_with("INSERT INTO")
}

#[cfg(all(test, feature = "migration"))]
mod tests {
    #[test]
    fn read_only_contract_matches_frozen_baseline() {
        assert_eq!(
            super::ddl_statements().collect::<Vec<_>>(),
            crate::migration::m20260820_000000_control_baseline::ddl_statements()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            super::schema_fingerprint(),
            crate::migration::m20260820_000000_control_baseline::schema_fingerprint()
        );
    }
}
