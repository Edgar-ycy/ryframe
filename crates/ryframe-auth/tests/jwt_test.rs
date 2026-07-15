use ryframe_auth::jwt::{TokenIdentity, decode_token, encode_access, parse_duration};
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
        ..Default::default()
    };
    let user_id = 1234567890123456789i64;
    let identity = TokenIdentity {
        user_id,
        tenant_id: "tenant-a",
        tenant_session_version: 1,
        user_auth_version: 1,
        username: "admin",
    };
    let (token, jti) = encode_access(&identity, &config).unwrap();
    let claims = decode_token(&token, &config.jwt_secret).unwrap();

    assert_eq!(claims.sub, user_id.to_string());
    assert_eq!(claims.tenant_id, "tenant-a");
    assert_eq!(claims.tenant_session_version, 1);
    assert_eq!(claims.user_auth_version, 1);
    assert_eq!(claims.username, "admin");
    assert_eq!(claims.token_type, "access");
    assert!(!claims.jti.is_empty());
    assert_eq!(claims.jti, jti);
}
