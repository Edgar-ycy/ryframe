const ONLINE_USER_KEY_PREFIX: &str = "ryframe:v0.5:online-user:";
const TENANT_INDEX_KEY_PREFIX: &str = "ryframe:v0.9:online-user-index:tenant:";
const TENANT_USER_INDEX_KEY_PREFIX: &str = "ryframe:v0.9:online-user-index:tenant-user:";

pub(super) fn session_key(tenant_id: &str, sid: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:{sid}")
}

pub(super) fn tenant_pattern(tenant_id: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:*")
}

pub(super) fn tenant_index_key(tenant_id: &str) -> String {
    format!("{TENANT_INDEX_KEY_PREFIX}{tenant_id}")
}

pub(super) fn tenant_user_index_key(tenant_id: &str, user_id: i64) -> String {
    format!("{TENANT_USER_INDEX_KEY_PREFIX}{tenant_id}:{user_id}")
}
