use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::http::HeaderMap;
use ryframe_common::utils::ip::{TrustedProxySet, get_client_ip, is_internal_ip};

#[test]
fn direct_clients_cannot_spoof_forwarding_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
    let trusted = TrustedProxySet::new(&["127.0.0.1/32".into()]).unwrap();

    assert_eq!(
        trusted.client_ip(&headers, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))
    );
}

#[test]
fn trusted_proxy_chain_is_peeled_from_the_right() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "198.51.100.9, 10.0.0.10".parse().unwrap(),
    );
    let trusted = TrustedProxySet::new(&["127.0.0.1/32".into(), "10.0.0.0/8".into()]).unwrap();

    assert_eq!(
        trusted.client_ip(&headers, IpAddr::V4(Ipv4Addr::LOCALHOST)),
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9))
    );
}

#[test]
fn malformed_forwarding_values_fall_back_safely() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
    headers.insert("x-real-ip", "2001:db8::5".parse().unwrap());
    let trusted = TrustedProxySet::new(&["::1/128".into()]).unwrap();

    assert_eq!(
        trusted.client_ip(&headers, IpAddr::V6(Ipv6Addr::LOCALHOST)),
        "2001:db8::5".parse::<IpAddr>().unwrap()
    );
}

#[test]
fn direct_address_parser_handles_ipv4_and_ipv6() {
    assert_eq!(
        get_client_ip(&HeaderMap::new(), "192.168.1.1:8080"),
        "192.168.1.1"
    );
    assert_eq!(get_client_ip(&HeaderMap::new(), "[::1]:8080"), "::1");
    assert_eq!(get_client_ip(&HeaderMap::new(), "bad-address"), "unknown");
}

#[test]
fn internal_address_detection_uses_standard_library_semantics() {
    assert!(is_internal_ip("10.0.0.1"));
    assert!(is_internal_ip("192.168.1.100"));
    assert!(is_internal_ip("127.0.0.1"));
    assert!(is_internal_ip("fd00::1"));
    assert!(!is_internal_ip("8.8.8.8"));
}
