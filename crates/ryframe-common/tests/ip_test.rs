use axum::http::HeaderMap;
use ryframe_common::utils::ip::{get_client_ip, is_internal_ip};

#[test]
fn test_get_client_ip_and_internal() {
    let mut h = HeaderMap::new();
    h.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
    h.insert("x-real-ip", "10.0.0.1".parse().unwrap());
    assert_eq!(get_client_ip(&h, "127.0.0.1:8080"), "1.2.3.4");

    let mut h2 = HeaderMap::new();
    h2.insert("x-real-ip", "10.0.0.1".parse().unwrap());
    assert_eq!(get_client_ip(&h2, "127.0.0.1:8080"), "10.0.0.1");

    assert_eq!(
        get_client_ip(&HeaderMap::new(), "192.168.1.1:8080"),
        "192.168.1.1"
    );

    assert!(is_internal_ip("10.0.0.1"));
    assert!(is_internal_ip("192.168.1.100"));
    assert!(is_internal_ip("127.0.0.1"));
    assert!(!is_internal_ip("8.8.8.8"));
}
