use ryframe_auth::{
    constant_time_eq,
    jwt::{self, TokenIdentity, TokenSettings},
};

fn token_settings(secret: &'static str) -> TokenSettings {
    TokenSettings::new(secret, "1h", "168h").expect("JWT 测试设置应有效")
}

fn token_identity() -> TokenIdentity<'static> {
    TokenIdentity {
        user_id: 42,
        tenant_id: "tenant-contract",
        tenant_session_version: 3,
        user_authorization_version: 7,
        username: "jwt-contract-user",
    }
}

#[test]
fn constant_time_comparison_checks_content_and_length() {
    assert!(constant_time_eq(b"monitor-token", b"monitor-token"));
    assert!(!constant_time_eq(b"monitor-token", b"monitor-other"));
    assert!(!constant_time_eq(b"monitor-token", b"monitor-token-long"));
}

#[test]
fn token_settings_parse_expirations_once() {
    let settings = TokenSettings::new("test-secret", "1h", "30m").unwrap();

    assert_eq!(settings.access_token_ttl_seconds(), 3_600);
    assert_eq!(settings.refresh_token_ttl_seconds(), 1_800);
}

#[test]
fn token_settings_reject_invalid_expiration() {
    assert!(TokenSettings::new("test-secret", "later", "7d").is_err());
}

#[test]
fn aws_lc_access_and_refresh_tokens_round_trip() {
    let settings = token_settings("ryframe-jwt-contract-secret-primary");
    let identity = token_identity();

    let (access_token, access_jti) =
        jwt::encode_access(&identity, &settings).expect("访问令牌应签发成功");
    let access = jwt::decode_token(&access_token, &settings).expect("访问令牌应验证成功");
    assert_eq!(access.sub, identity.user_id.to_string());
    assert_eq!(access.tenant_id, identity.tenant_id);
    assert_eq!(
        access.tenant_session_version,
        identity.tenant_session_version
    );
    assert_eq!(
        access.user_authorization_version,
        identity.user_authorization_version
    );
    assert_eq!(access.username, identity.username);
    assert_eq!(access.token_type, "access");
    assert_eq!(access.jti, access_jti);
    assert!(!access.sid.is_empty());

    let refresh_token = jwt::encode_refresh(&identity, &settings).expect("刷新令牌应签发成功");
    let refresh = jwt::decode_token(&refresh_token, &settings).expect("刷新令牌应验证成功");
    assert_eq!(refresh.sub, identity.user_id.to_string());
    assert_eq!(refresh.tenant_id, identity.tenant_id);
    assert_eq!(refresh.token_type, "refresh");
    assert!(!refresh.sid.is_empty());
    assert!(!refresh.jti.is_empty());
}

#[test]
fn aws_lc_jwt_rejects_a_different_secret() {
    let issuer = token_settings("ryframe-jwt-contract-secret-primary");
    let verifier = token_settings("ryframe-jwt-contract-secret-secondary");
    let (token, _) = jwt::encode_access(&token_identity(), &issuer).expect("访问令牌应签发成功");

    assert!(jwt::decode_token(&token, &verifier).is_err());
}

#[test]
fn aws_lc_jwt_rejects_expired_claims() {
    let settings = token_settings("ryframe-jwt-contract-secret-primary");
    let token = jwt::encode_refresh_for_session_at(
        &token_identity(),
        "expired-session",
        "expired-jti".to_owned(),
        1,
        1,
        &settings,
    )
    .expect("过期 fixture 应完成签名");

    assert!(jwt::decode_token(&token, &settings).is_err());
}
