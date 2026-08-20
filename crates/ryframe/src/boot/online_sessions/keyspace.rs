const SESSION_KEY_PREFIX: &str = "online-user:session:";
const TENANT_INDEX_KEY_PREFIX: &str = "online-user:index:tenant:";
const USER_INDEX_KEY_PREFIX: &str = "online-user:index:user:";

pub(super) fn session_key(tenant_id: &str, sid: &str) -> String {
    format!("{SESSION_KEY_PREFIX}{tenant_id}:{sid}")
}

pub(super) fn tenant_index_key(tenant_id: &str) -> String {
    format!("{TENANT_INDEX_KEY_PREFIX}{tenant_id}")
}

pub(super) fn tenant_user_index_key(tenant_id: &str, user_id: i64) -> String {
    format!("{USER_INDEX_KEY_PREFIX}{tenant_id}:{user_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_and_indexes_have_distinct_keyspaces() {
        let session = session_key("tenant-a", "sid-a");
        let tenant = tenant_index_key("tenant-a");
        let user = tenant_user_index_key("tenant-a", 42);

        assert_ne!(session, tenant);
        assert_ne!(session, user);
        assert_ne!(tenant, user);
        assert!(session.ends_with("tenant-a:sid-a"));
    }
}
