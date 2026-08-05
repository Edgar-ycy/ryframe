const ONLINE_USER_KEY_PREFIX: &str = "ryframe:v0.5:online-user:";

pub(super) fn session_key(tenant_id: &str, sid: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:{sid}")
}

pub(super) fn tenant_pattern(tenant_id: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:*")
}
