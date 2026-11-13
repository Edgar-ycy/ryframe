//! B0 产品基线和已登记执行适配的纯身份；不访问源码、文件或运行资源。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const BACKEND: &str = "815c5eafb09d4b493319d255fb7ab88ddd02c8b6";
pub(crate) const FRONTEND: &str = "0087ea2ecf62530d042b9e52f5c950fb34c66d78";
pub(crate) const COMMIT: &str = "c9d88b76a3b768da672855f94fd9233779f8878f";
pub(crate) const BASE_TREE: &str = "aeb1824ee52ea12d53d0cf87c327c7be3d1e12b4";
pub(crate) const TREE: &str = "dee141b8d481f4cd7fc9da3896c3b311e84cb66c";
pub(crate) const PATCH: &str = "xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch";
pub(crate) const PATCH_SHA256: &str =
    "ed1a97da064e58ffb35c1eeb5a748cbfce7c567f9d19141de907d2cd982f5307";
pub(crate) const PATCH_LENGTH: usize = 189974;
pub(crate) const PATHS: &[&str] = &[
    "catalog/migrations.lock.toml",
    "crates/ryframe-db/src/entities/background_job.rs",
    "crates/ryframe-db/src/migration/baseline_contract.rs",
    "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/backup.rs",
    "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/base.rs",
    "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/base/tables.rs",
    "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/jobs.rs",
    "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
    "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/schema.rs",
    "crates/ryframe-db/src/migration/m20261013_000000_password_change.rs",
    "crates/ryframe-db/src/migration/m20261017_000000_data_scope_projection.rs",
    "crates/ryframe-db/src/migration/mod.rs",
    "crates/ryframe-db/src/migration/schema.rs",
    "crates/ryframe-db/src/migration/schema/business_migration_ledger.rs",
    "crates/ryframe-db/src/migration/schema/expected.rs",
    "crates/ryframe-db/src/migration/schema/expected/clauses.rs",
    "crates/ryframe-db/src/migration/schema/inspect.rs",
    "crates/ryframe-db/src/migration/schema/normalize.rs",
    "crates/ryframe-db/src/migration/schema/normalize/check.rs",
    "crates/ryframe-db/src/migration/schema/normalize/check/rules.rs",
    "crates/ryframe-db/src/migration/schema/types.rs",
    "crates/ryframe-db/src/migration/schema/verify.rs",
    "crates/ryframe-db/src/migration/schema/verify/ledger_tests.rs",
    "crates/ryframe-db/src/repositories/background_job_repo/claim.rs",
    "crates/ryframe-db/src/repositories/background_job_repo/enqueue.rs",
    "crates/ryframe-db/tests/mapping_contracts/export_mappings.rs",
    "crates/ryframe-db/tests/mapping_contracts/job_and_file_mappings.rs",
    "crates/ryframe-db/tests/mapping_contracts/migration_contracts.rs",
    "crates/ryframe-tenant-db/src/migration/catalog.rs",
    "crates/ryframe-tenant-db/src/migration/database_adapter.rs",
    "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
    "crates/ryframe-tenant-db/src/migration/m20261017_000000_data_scope_projection.rs",
    "crates/ryframe-tenant-db/src/migration/mod.rs",
    "crates/ryframe-tenant-db/src/migration/runtime.rs",
    "crates/ryframe-tenant-db/src/migration/schema.rs",
    "crates/ryframe-tenant-db/tests/tenant_mapping_contracts/migration_contracts.rs",
    "sql/ryframe_config.sql",
    "xtask/src/cli.rs",
];
pub(crate) const CONTRACT: &str = "legacy-stable-readiness-b0-v1";
pub(crate) const PATCH_BYTES: &[u8] =
    include_bytes!("../../assets/baseline-adapters/stable-readiness-b0-v1.patch");

pub(crate) fn evidence() -> crate::Result<Value> {
    validate_patch(PATCH_BYTES)?;
    Ok(expected())
}

pub(crate) fn validate_patch(bytes: &[u8]) -> crate::Result<()> {
    if bytes.len() != PATCH_LENGTH || hex::encode(Sha256::digest(bytes)) != PATCH_SHA256 {
        return Err("B0 编译适配资产的长度或摘要与固定登记不同".into());
    }
    Ok(())
}

pub(crate) fn expected() -> Value {
    json!({"contract":CONTRACT,"base_backend_sha":BACKEND,"base_frontend_sha":FRONTEND,
        "reference_adapter_sha":COMMIT,"adapter_tree":TREE,"reconstructed_tree":TREE,
        "adapter_paths":PATHS,"patch":{"path":PATCH,"bytes":PATCH_LENGTH,"sha256":PATCH_SHA256}})
}

pub(crate) fn matches(authority: &Value) -> bool {
    authority["frontend_sha"] == FRONTEND
        && authority["backend_adapter_contract"] == CONTRACT
        && authority["backend_product_sha"] == BACKEND
        && authority["backend_execution_sha"] == COMMIT
}
