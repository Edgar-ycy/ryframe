use ryframe_auth::{constant_time_eq, jwt::TokenSettings};

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
