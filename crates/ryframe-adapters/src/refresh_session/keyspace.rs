pub(super) const FAMILY_PREFIX: &str = "ryframe:v0.5:refresh-family:";
pub(super) const TENANT_INDEX_PREFIX: &str = "ryframe:v0.5:refresh-family-index:tenant:";
pub(super) const TENANT_USER_INDEX_PREFIX: &str = "ryframe:v0.5:refresh-family-index:tenant-user:";

pub(super) fn family(sid: &str) -> String {
    format!("{FAMILY_PREFIX}{sid}")
}

pub(super) fn tenant_index(tenant_id: &str) -> String {
    format!("{TENANT_INDEX_PREFIX}{tenant_id}")
}

pub(super) fn tenant_user_index(tenant_id: &str, user_id: i64) -> String {
    format!("{TENANT_USER_INDEX_PREFIX}{tenant_id}:{user_id}")
}
