use ryframe_auth::jwt::{decode_token, encode_access, parse_duration};
use ryframe_config::AuthConfig;

#[test]
fn test_parse_duration() {
    assert_eq!(parse_duration("1h").unwrap(), 3600);
    assert_eq!(parse_duration("30m").unwrap(), 1800);
    assert_eq!(parse_duration("3600").unwrap(), 3600);
    assert!(parse_duration("abc").is_err());
}

#[test]
fn test_encode_decode_roundtrip() {
    let config = AuthConfig {
        jwt_secret: "test-secret".into(),
        access_token_expire: "1h".into(),
        refresh_token_expire: "168h".into(),
        max_login_attempts: 5,
        lockout_duration_minutes: 30,
        enable_password_complexity: true,
    };
    let user_id = 1234567890123456789i64;
    let roles = vec!["admin".to_string()];
    let perms = vec!["system:user:list".to_string()];

    let (token, jti) = encode_access(user_id, "admin", &roles, &perms, &config).unwrap();
    let claims = decode_token(&token, &config.jwt_secret).unwrap();

    assert_eq!(claims.sub, user_id.to_string());
    assert_eq!(claims.username, "admin");
    assert_eq!(claims.roles, roles);
    assert_eq!(claims.perms, perms);
    assert_eq!(claims.token_type, "access");
    assert!(!claims.jti.is_empty());
    assert_eq!(claims.jti, jti);
}
