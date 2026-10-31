use ryframe_auth::{
    constant_time_eq,
    jwt::{self, TokenIdentity, TokenSettings},
};
use ryframe_kernel::AppError;

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
fn token_settings_convert_supported_expirations_to_seconds() {
    for (access, refresh, access_seconds, refresh_seconds) in [
        ("1h", "30m", 3_600, 1_800),
        ("30m", "1h", 1_800, 3_600),
        ("30s", "120", 30, 120),
        ("120", "45s", 120, 45),
        (" 2 h ", " 3 m ", 7_200, 180),
        (" 3 m ", " 2 h ", 180, 7_200),
        (" 4 s ", "  5  ", 4, 5),
        ("  5  ", " 4 s ", 5, 4),
        ("0", "0", 0, 0),
    ] {
        let settings = TokenSettings::new("test-secret", access, refresh)
            .expect("支持的有效期写法应能构造设置");
        assert_eq!(settings.access_token_ttl_seconds(), access_seconds);
        assert_eq!(settings.refresh_token_ttl_seconds(), refresh_seconds);
    }
}

#[test]
fn token_settings_reject_invalid_expiration() {
    const SECRET: &str = "private-expiration-test-secret";
    for (invalid, reason) in [
        ("later".to_owned(), "格式无效"),
        ("7d".to_owned(), "格式无效"),
        ("1H".to_owned(), "格式无效"),
        (format!("{}0", usize::MAX), "数值超出"),
        (format!("{}h", usize::MAX / 3_600 + 1), "换算为秒时溢出"),
        (format!("{}m", usize::MAX / 60 + 1), "换算为秒时溢出"),
        (usize::MAX.to_string(), "与当前时间相加"),
    ] {
        for (access, refresh, field, other_field) in [
            (
                invalid.as_str(),
                "168h",
                "auth.access_token_expire",
                "auth.refresh_token_expire",
            ),
            (
                "1h",
                invalid.as_str(),
                "auth.refresh_token_expire",
                "auth.access_token_expire",
            ),
        ] {
            let error = TokenSettings::new(SECRET, access, refresh)
                .err()
                .expect("单个非法有效期必须返回错误");
            assert!(matches!(&error, AppError::Config(_)));
            let message = error.to_string();
            assert!(message.contains(field), "{message}");
            assert!(!message.contains(other_field), "{message}");
            assert!(message.contains(reason), "{message}");
            assert!(message.contains("请"), "{message}");
            if reason != "与当前时间相加" {
                assert!(message.contains("h/m/s"), "{message}");
                assert!(message.contains("纯数字秒"), "{message}");
            }
            assert!(!message.contains(SECRET));
            assert!(!message.contains(&invalid));
        }
    }
}

#[test]
fn csrf_expiration_overflow_returns_a_configuration_error() {
    const SECRET: &str = "private-csrf-expiration-secret";
    let settings = token_settings(SECRET);
    let token = jwt::encode_csrf(&settings, Some("csrf-session"), 60)
        .expect("可表示的 CSRF 有效期应能签发");
    let claims = jwt::decode_csrf(&token, &settings).expect("正常 CSRF 令牌应能验证");
    assert_eq!(claims.sid.as_deref(), Some("csrf-session"));

    let error = jwt::encode_csrf(&settings, Some("csrf-session"), usize::MAX)
        .expect_err("超大 CSRF 有效期必须返回错误");
    assert!(matches!(&error, AppError::Config(_)));
    let message = error.to_string();
    assert!(message.contains("CSRF ttl_seconds"), "{message}");
    assert!(message.contains("与当前时间相加"), "{message}");
    assert!(message.contains("请减小有效期"), "{message}");
    assert!(!message.contains(SECRET));
}

#[test]
fn aws_lc_access_and_refresh_tokens_round_trip() {
    let settings = TokenSettings::new("ryframe-jwt-contract-secret-primary", "30s", "90s")
        .expect("秒单位有效期应能构造设置");
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
    assert_eq!(access.exp - access.iat, 30);

    let refresh_token = jwt::encode_refresh(&identity, &settings).expect("刷新令牌应签发成功");
    let refresh = jwt::decode_token(&refresh_token, &settings).expect("刷新令牌应验证成功");
    assert_eq!(refresh.sub, identity.user_id.to_string());
    assert_eq!(refresh.tenant_id, identity.tenant_id);
    assert_eq!(refresh.token_type, "refresh");
    assert!(!refresh.sid.is_empty());
    assert!(!refresh.jti.is_empty());
    assert_eq!(refresh.exp - refresh.iat, 90);
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
