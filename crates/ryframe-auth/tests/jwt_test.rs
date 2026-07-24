use ryframe_auth::jwt::{
    TokenIdentity, decode_csrf, decode_token, encode_access, encode_access_for_session,
    encode_csrf, encode_refresh_for_session, encode_refresh_for_session_at, generate_jti, new_sid,
    parse_duration,
};
use ryframe_config::AuthConfig;

#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
struct GoldenHs256Claims {
    sub: String,
    name: String,
    iat: usize,
}

#[test]
fn hs256_matches_fixed_golden_vectors() {
    const SECRET: &str = "your-256-bit-secret";
    const STANDARD_TOKEN: &str = concat!(
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.",
        "eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.",
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
    );
    const GENERATED_TOKEN: &str = concat!(
        "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.",
        "eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.",
        "wrJ__8Q_6BcB2ug9370TBuK5JoAjErqsQtYf7aLcFBk"
    );
    let expected = GoldenHs256Claims {
        sub: "1234567890".into(),
        name: "John Doe".into(),
        iat: 1_516_239_022,
    };

    let encoded = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &expected,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    assert_eq!(encoded, GENERATED_TOKEN);

    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    let decoded = jsonwebtoken::decode::<GoldenHs256Claims>(
        STANDARD_TOKEN,
        &jsonwebtoken::DecodingKey::from_secret(SECRET.as_bytes()),
        &validation,
    )
    .unwrap();
    assert_eq!(decoded.claims, expected);
}

#[test]
fn test_parse_duration() {
    assert_eq!(parse_duration("1h").unwrap(), 3600);
    assert_eq!(parse_duration("30m").unwrap(), 1800);
    assert_eq!(parse_duration("3600").unwrap(), 3600);
    assert!(parse_duration("abc").is_err());
}

#[test]
fn refresh_token_can_be_reconstructed_from_committed_rotation_metadata() {
    let config = AuthConfig {
        jwt_secret: "test-secret".into(),
        ..Default::default()
    };
    let identity = TokenIdentity {
        user_id: 42,
        tenant_id: "tenant-a",
        tenant_session_version: 3,
        user_auth_version: 7,
        username: "alice",
    };
    let issued_at = chrono::Utc::now().timestamp() as usize;
    let absolute_exp = issued_at + 600;
    let first = encode_refresh_for_session_at(
        &identity,
        "sid-recovered",
        "committed-jti".into(),
        issued_at,
        absolute_exp,
        &config,
    )
    .unwrap();
    let recovered = encode_refresh_for_session_at(
        &identity,
        "sid-recovered",
        "committed-jti".into(),
        issued_at,
        absolute_exp,
        &config,
    )
    .unwrap();

    assert_eq!(first, recovered);
    let claims = decode_token(&recovered, &config.jwt_secret).unwrap();
    assert_eq!(claims.jti, "committed-jti");
    assert_eq!(claims.iat, issued_at);
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

#[test]
fn access_refresh_and_csrf_share_the_stable_session_id() {
    let config = AuthConfig {
        jwt_secret: "test-secret".into(),
        ..Default::default()
    };
    let identity = TokenIdentity {
        user_id: 42,
        tenant_id: "tenant-a",
        tenant_session_version: 3,
        user_auth_version: 7,
        username: "alice",
    };
    let sid = new_sid();
    let refresh_jti = generate_jti();
    let absolute_exp = chrono::Utc::now().timestamp() as usize + 600;
    let (access, access_jti) = encode_access_for_session(&identity, &sid, &config).unwrap();
    let refresh =
        encode_refresh_for_session(&identity, &sid, refresh_jti.clone(), absolute_exp, &config)
            .unwrap();
    let csrf = encode_csrf(&config.jwt_secret, Some(&sid), 300).unwrap();

    let access_claims = decode_token(&access, &config.jwt_secret).unwrap();
    let refresh_claims = decode_token(&refresh, &config.jwt_secret).unwrap();
    let csrf_claims = decode_csrf(&csrf, &config.jwt_secret).unwrap();
    assert_eq!(access_claims.sid, sid);
    assert_eq!(refresh_claims.sid, access_claims.sid);
    assert_eq!(csrf_claims.sid.as_deref(), Some(access_claims.sid.as_str()));
    assert_eq!(access_claims.jti, access_jti);
    assert_eq!(refresh_claims.jti, refresh_jti);
    assert_ne!(access_claims.jti, refresh_claims.jti);
    assert_eq!(refresh_claims.exp, absolute_exp);
}
