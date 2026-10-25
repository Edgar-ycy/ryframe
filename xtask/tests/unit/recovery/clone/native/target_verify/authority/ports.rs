use super::*;
use serde_json::json;

fn row(address: &str, port: u16) -> Listener {
    serde_json::from_value(json!({"LocalAddress":address,"LocalPort":port,"State":2})).unwrap()
}

#[test]
fn idle_requires_the_complete_ipv4_ipv6_and_dual_stack_listener_boundary() {
    let ipv4 = endpoint("http://127.0.0.1:28080").unwrap();
    let ipv6 = endpoint("http://[::1]:28080/readyz").unwrap();
    assert!(require_idle(ipv4, &[]).is_ok());
    assert!(require_idle(ipv6, &[]).is_ok());
    for host in ["127.0.0.1", "0.0.0.0", "::"] {
        assert!(require_idle(ipv4, &[row(host, 28080)]).is_err());
    }
    for host in ["::1", "::"] {
        assert!(require_idle(ipv6, &[row(host, 28080)]).is_err());
    }
    assert!(require_idle(ipv4, &[row("::1", 28080), row("127.0.0.1", 28081)]).is_ok());
    assert!(require_idle(ipv6, &[row("0.0.0.0", 28080)]).is_ok());
    assert!(require_idle(ipv4, &[row("127.0.0.1", 28081), row("::", 28080)]).is_err());
}

#[test]
fn unknown_listener_observations_and_endpoint_overrides_never_prove_a_closed_port() {
    let target = endpoint("https://127.0.0.1:28080").unwrap();
    for value in [
        json!({"LocalAddress":"127.0.0.1","LocalPort":true,"State":2}),
        json!({"LocalAddress":"127.0.0.1","LocalPort":28080}),
        json!({"LocalAddress":"127.0.0.1","LocalPort":28080,"State":2,"extra":0}),
    ] {
        assert!(serde_json::from_value::<Listener>(value).is_err());
    }
    for value in [row("private-value", 28080), row("127.0.0.1", 0)] {
        let error = require_idle(target, &[value]).unwrap_err().to_string();
        assert!(!error.contains("private-value"));
    }
    let mut established = row("127.0.0.1", 28080);
    established.state = 5;
    assert!(require_idle(target, &[established]).is_err());
    for value in [
        "http://localhost:28080",
        "http://private:credential@127.0.0.1:28080",
        "http://127.0.0.1:28080/?private=value",
        "ftp://127.0.0.1:28080",
    ] {
        let error = endpoint(value).unwrap_err().to_string();
        assert!(!error.contains("credential"));
        assert!(!error.contains("value"));
    }
}
