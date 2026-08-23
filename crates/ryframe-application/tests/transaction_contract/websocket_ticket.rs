use std::{collections::HashMap, sync::Arc};

use ryframe_kernel::{ActorContext, DataScope};
use tokio::sync::Mutex;

use super::*;

#[derive(Default)]
struct MemoryTicketStore {
    values: Mutex<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl WebSocketTicketStore for MemoryTicketStore {
    async fn put(&self, key: String, value: String, _ttl_secs: u64) -> AppResult<()> {
        self.values.lock().await.insert(key, value);
        Ok(())
    }

    async fn take(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.values.lock().await.remove(key))
    }
}

#[tokio::test]
async fn issued_ticket_can_only_be_consumed_once() {
    let store = Arc::new(MemoryTicketStore::default());
    let service = WebSocketTicketService::new(
        Some(store),
        MessagingPolicy::new(true, 60, 7, 100).expect("策略应有效"),
    );
    let principal = RequestPrincipal {
        actor: ActorContext {
            user_id: 42,
            tenant_id: "tenant-a".into(),
            username: "tester".into(),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::SelfOnly,
            custom_dept_ids: Vec::new(),
            include_self: true,
            is_super_admin: false,
        },
        tenant_authorization_epoch: 0,
        preferred_locale: None,
        roles: Vec::new(),
        role_ids: Vec::new(),
        permissions: Vec::new(),
        tenant_request_limit_per_minute: 0,
    };
    let claims = Claims {
        sub: "42".into(),
        tenant_id: "tenant-a".into(),
        tenant_session_version: 3,
        user_authorization_version: 4,
        username: "tester".into(),
        token_type: "access".into(),
        sid: "session-a".into(),
        jti: "token-a".into(),
        iat: 1,
        exp: 2,
    };

    let grant = service
        .issue(&principal, &claims, "en-GB")
        .await
        .expect("票据应签发成功");
    let consumed = service
        .consume(&grant.ticket)
        .await
        .expect("首次消费应成功");
    assert_eq!(consumed.user_id, 42);
    assert_eq!(consumed.locale, "en-US");
    assert!(service.consume(&grant.ticket).await.is_err());
}
