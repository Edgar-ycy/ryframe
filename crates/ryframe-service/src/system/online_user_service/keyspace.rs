const ONLINE_USER_KEY_PREFIX: &str = "ryframe:v0.5:online-user:";

pub(super) fn session_key(tenant_id: &str, sid: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:{sid}")
}

pub(super) fn tenant_pattern(tenant_id: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:*")
}

#[cfg(test)]
mod tests {
    use super::{session_key, tenant_pattern};

    #[test]
    fn keyspace_keeps_tenants_and_sessions_separate() {
        assert_eq!(
            session_key("tenant-a", "session-1"),
            "ryframe:v0.5:online-user:tenant-a:session-1"
        );
        assert_eq!(
            tenant_pattern("tenant-a"),
            "ryframe:v0.5:online-user:tenant-a:*"
        );
    }
}
