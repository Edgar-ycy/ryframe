use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::{
    recovery::monitoring::{config, execution::ProtectedInputs, permissions, staging},
    workspace::root_dir,
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = root_dir()
            .join(".local-tests")
            .join(format!("monitor-security-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn valid_acl() -> Value {
    json!({
        "current_sid": "S-1-5-21-123-456-789-1001",
        "owner_sid": "S-1-5-21-123-456-789-1001",
        "protected": true,
        "entries": [
            {"sid": "S-1-5-21-123-456-789-1001", "type": 0, "rights": 1179785, "inherited": false},
            {"sid": "S-1-5-18", "type": 0, "rights": 2032127, "inherited": false},
        ],
    })
}

#[test]
fn windows_acl_accepts_only_explicit_read_for_the_private_owner_and_system() {
    let value = valid_acl();
    let normalized = permissions::validate_windows_acl(&value).unwrap();
    assert_eq!(normalized["owner_sid"], value["owner_sid"]);
    let mut reordered = value.clone();
    reordered["entries"].as_array_mut().unwrap().reverse();
    assert_eq!(
        permissions::validate_windows_acl(&reordered).unwrap(),
        normalized
    );
    for owner in ["S-1-5-18", "S-1-5-32-544"] {
        let mut value = value.clone();
        value["owner_sid"] = json!(owner);
        assert!(permissions::validate_windows_acl(&value).is_ok());
    }
}

#[test]
fn windows_acl_rejects_unknown_owner_broad_subject_deny_inheritance_and_missing_read() {
    let original = valid_acl();
    for (pointer, bad) in [
        ("/owner_sid", json!("S-1-5-21-999")),
        ("/protected", json!(false)),
        ("/entries", json!([])),
        ("/entries/0/sid", json!("S-1-1-0")),
        ("/entries/0/sid", json!("S-1-5-32-545")),
        ("/entries/0/sid", json!("S-1-5-21-999")),
        ("/entries/0/type", json!(1)),
        ("/entries/0/type", json!(9)),
        ("/entries/0/inherited", json!(true)),
        ("/entries/0/rights", json!(0)),
        ("/entries/0/rights", json!(-1)),
        ("/entries/0/rights", json!(4294967296_u64)),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert!(
            permissions::validate_windows_acl(&value).is_err(),
            "{pointer}: {value}"
        );
    }
    let mut unknown = original;
    unknown["entries"][0]["other"] = json!(true);
    assert!(permissions::validate_windows_acl(&unknown).is_err());
}

#[test]
fn tool_environment_does_not_inherit_application_credentials_or_proxy() {
    let fixture = Fixture::new();
    let inherited = [
        ("SystemRoot", "system"),
        ("PATH", "tools"),
        ("LC_ALL", "zh_CN"),
        ("APP_DATABASE_PASSWORD", "private-db"),
        ("APP_AUTH_JWT_SECRET", "private-jwt"),
        ("APP_MONITOR_METRICS_BEARER_TOKEN", "private-metrics"),
        ("AWS_SECRET_ACCESS_KEY", "private-s3"),
        ("HTTP_PROXY", "private-proxy"),
        ("https_proxy", "private-proxy"),
        ("TEMP", "old-temp"),
        ("TMP", "old-tmp"),
        ("NO_PROXY", "*"),
    ]
    .map(|(key, value)| (key.to_owned(), value.to_owned()));
    let value = config::safe_environment_from(&fixture.0, inherited).unwrap();
    assert_eq!(value.len(), 6);
    assert_eq!(value["SystemRoot"], "system");
    assert_eq!(value["PATH"], "tools");
    assert_eq!(value["LC_ALL"], "zh_CN");
    assert_eq!(value["TEMP"], fixture.0.to_string_lossy());
    assert_eq!(value["TMP"], fixture.0.to_string_lossy());
    assert_eq!(value["NO_PROXY"], "127.0.0.1,localhost,::1");
    assert!(!serde_json::to_string(&value).unwrap().contains("private-"));
}

#[test]
fn credential_read_detects_permission_drift_without_exposing_secret() {
    let fixture = Fixture::new();
    let path = fixture.0.join("metrics-token.txt");
    fs::write(&path, "private-metrics-token\n").unwrap();
    let calls = std::cell::Cell::new(0);
    let result = staging::read_credential_with_security(&path, |_| {
        let count = calls.get();
        calls.set(count + 1);
        Ok(json!({"generation": count}))
    });
    let message = result.unwrap_err().to_string();
    assert!(message.contains("权限"));
    assert!(!message.contains("private-metrics-token"));
}

#[test]
fn execution_guard_rechecks_digest_and_reports_both_primary_and_drift_errors() {
    let fixture = Fixture::new();
    let path = fixture.0.join("tool");
    fs::write(&path, b"original").unwrap();
    let guard = ProtectedInputs::open([path.clone()]).unwrap();
    guard.assert_unchanged().unwrap();
    assert_eq!(guard.finish(Ok(17)).unwrap(), 17);
    assert_eq!(
        guard
            .finish::<()>(Err("primary".into()))
            .unwrap_err()
            .to_string(),
        "primary"
    );
    #[cfg(windows)]
    {
        assert!(fs::write(&path, b"changed!").is_err());
        assert!(fs::rename(&path, fixture.0.join("replacement")).is_err());
        assert!(fs::remove_file(&path).is_err());
    }
    #[cfg(unix)]
    {
        fs::write(&path, b"changed!").unwrap();
        let message = guard
            .finish::<()>(Err("primary".into()))
            .unwrap_err()
            .to_string();
        assert!(message.contains("primary"));
        assert!(message.contains("复核同时失败"));
    }
    drop(guard);
    fs::write(&path, b"changed!").unwrap();
}

#[test]
fn execution_guard_rejects_non_file_inputs() {
    let fixture = Fixture::new();
    assert!(ProtectedInputs::open([fixture.0.clone()]).is_err());
    assert!(ProtectedInputs::open([fixture.0.join("absent")]).is_err());
}
