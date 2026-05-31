use std::time::Duration;

/// token_blacklist 模块测试
/// 从 crates/ryframe-core/src/token_blacklist.rs 内联测试迁移
use ryframe_core::token_blacklist::{TokenBlacklist, blacklist_key};

#[tokio::test]
async fn test_blacklist_and_check_memory() {
    let bl = TokenBlacklist::new(None);
    let jti = "test-jti-001";

    // 初始不在黑名单
    assert!(!bl.is_blacklisted(jti).await);

    // 加入黑名单（60秒）
    bl.blacklist(jti, 60).await;
    assert!(bl.is_blacklisted(jti).await);
}

#[tokio::test]
async fn test_blacklist_expiry_memory() {
    let bl = TokenBlacklist::new(None);
    let jti = "test-jti-002";

    // 加入黑名单（1秒）
    bl.blacklist(jti, 1).await;
    assert!(bl.is_blacklisted(jti).await);

    // 等待过期
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!bl.is_blacklisted(jti).await);
}

#[tokio::test]
async fn test_multiple_jtis() {
    let bl = TokenBlacklist::new(None);
    bl.blacklist("jti-a", 60).await;
    bl.blacklist("jti-b", 60).await;

    assert!(bl.is_blacklisted("jti-a").await);
    assert!(bl.is_blacklisted("jti-b").await);
    assert!(!bl.is_blacklisted("jti-c").await);
}

#[tokio::test]
async fn test_blacklist_key_format() {
    assert_eq!(blacklist_key("abc123"), "token:blacklist:abc123");
}
