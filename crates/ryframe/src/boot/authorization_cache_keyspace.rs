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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_keys_share_one_cluster_hash_tag() {
        let epoch = tenant_epoch_key("tenant-a");
        let user = user_version_key("tenant-a", 42);
        let snapshot = snapshot_hash_key("tenant-a", 42);

        assert!(epoch.contains("{tenant-a}"));
        assert!(user.contains("{tenant-a}"));
        assert!(snapshot.contains("{tenant-a}"));
        assert_ne!(epoch, tenant_epoch_key("tenant-b"));
    }
}
