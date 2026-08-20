const ONLINE_USER_KEY_PREFIX: &str = "online-user:session:";

pub(super) fn session_key(tenant_id: &str, sid: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:{sid}")
}
