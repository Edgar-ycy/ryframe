use ryframe_kernel::{AppError, AppResult};

pub(super) fn validate_namespace_version(version: i64) -> AppResult<()> {
    if version < 0 {
        return Err(AppError::Database("缓存命名空间版本不能为负数".into()));
    }
    Ok(())
}

fn tenant_hash_tag(tenant_id: &str) -> String {
    format!("{{{tenant_id}}}")
}

pub(super) fn tenant_epoch_key(tenant_id: &str) -> String {
    format!("ryframe:authorization:{}:epoch", tenant_hash_tag(tenant_id))
}

pub(super) fn user_version_key(tenant_id: &str, user_id: i64) -> String {
    format!(
        "ryframe:authorization:{}:user:{user_id}:version",
        tenant_hash_tag(tenant_id)
    )
}

pub(super) fn snapshot_hash_key(tenant_id: &str, user_id: i64) -> String {
    format!(
        "ryframe:authorization:{}:user:{user_id}:snapshots",
        tenant_hash_tag(tenant_id)
    )
}

pub(super) fn tenant_value_hash_key(tenant_id: &str, namespace: &str) -> String {
    format!(
        "ryframe:tenant-cache:{}:{namespace}",
        tenant_hash_tag(tenant_id)
    )
}

pub(super) fn namespace_version_key(tenant_id: &str, namespace: &str) -> String {
    format!(
        "ryframe:tenant-cache:{}:{namespace}:version",
        tenant_hash_tag(tenant_id)
    )
}

pub(super) fn namespace_values_hash_key(tenant_id: &str, namespace: &str) -> String {
    format!(
        "ryframe:tenant-cache:{}:{namespace}:values",
        tenant_hash_tag(tenant_id)
    )
}
