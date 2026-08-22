const SESSION_KEY_PREFIX: &str = "online-user:session:";
const TENANT_INDEX_KEY_PREFIX: &str = "online-user:index:tenant:";
const USER_INDEX_KEY_PREFIX: &str = "online-user:index:user:";

pub fn session_key(tenant_id: &str, sid: &str) -> String {
    format!("{SESSION_KEY_PREFIX}{tenant_id}:{sid}")
}

pub fn tenant_index_key(tenant_id: &str) -> String {
    format!("{TENANT_INDEX_KEY_PREFIX}{tenant_id}")
}

pub fn tenant_user_index_key(tenant_id: &str, user_id: i64) -> String {
    format!("{USER_INDEX_KEY_PREFIX}{tenant_id}:{user_id}")
}
