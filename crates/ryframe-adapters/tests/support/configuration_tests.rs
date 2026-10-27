use super::configuration::{Captured, Configuration, OwnedConfiguration, files};
use std::{fs, path::Path};

#[path = "../../../../xtask/src/ci/integration_configuration/test_fixture.rs"]
mod fixture;
use fixture::{SECRET, fixture, source};

fn safe(error: &str, path: &Path) {
    assert!(
        error.contains(path.to_string_lossy().as_ref()),
        "诊断缺少相关路径"
    );
    assert!(!error.contains(SECRET), "诊断泄露凭据");
    assert!(error.contains("请") || error.contains("处理="));
}

#[test]
fn missing_directory_skips_but_existing_invalid_input_fails() {
    let scope = fixture();
    let missing = scope.path().join("missing");
    assert!(!files::directory_available(&missing).unwrap());
    fs::create_dir(&missing).unwrap();
    let error = Configuration::from_directory(scope.path(), &missing, "missing", false)
        .err()
        .unwrap();
    safe(&error, &missing.join("app.test.toml"));
    fs::write(
        missing.join("app.test.toml"),
        format!("password = '{SECRET}'\n[broken"),
    )
    .unwrap();
    let error = Configuration::from_directory(scope.path(), &missing, "invalid", false)
        .err()
        .unwrap();
    safe(&error, &missing.join("app.test.toml"));
    assert!(error.contains("TOML") && error.contains("byte/span="));
    fs::remove_file(missing.join("app.test.toml")).unwrap();
    fs::create_dir(missing.join("app.test.toml")).unwrap();
    assert!(Captured::load(&missing, false).is_err());
}

#[test]
fn typed_fields_size_limit_and_io_diagnostics_fail_without_secret_values() {
    let scope = fixture();
    let source = source(scope.path(), true);
    let selected = source.join("app.test.toml");
    fs::write(&selected, format!("[redis]\nport='{SECRET}'\n")).unwrap();
    let error = Captured::load(&source, false).err().unwrap();
    safe(&error, &selected);
    assert!(error.contains("port"));
    let file = fs::OpenOptions::new().write(true).open(&selected).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let error = Captured::load(&source, false).err().unwrap();
    safe(&error, &selected);
    assert!(error.contains("16MiB"));
    let error = files::io_error(
        &selected,
        "离线权限诊断",
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, SECRET),
    );
    safe(&error, &selected);
    assert!(error.contains("PermissionDenied") && error.contains("rawcode=None"));
}

#[test]
fn only_scope_changes_and_all_redis_fields_and_tls_material_are_preserved() {
    let scope = fixture();
    let source = source(scope.path(), true);
    let mut overlay = toml::Table::new();
    let mut redis = toml::Table::new();
    redis.insert("tls".into(), true.into());
    for field in ["tls_ca", "tls_client_cert", "tls_client_key"] {
        let path = scope.path().join(format!("{field}.pem"));
        fs::write(&path, format!("isolated-{field}-bytes")).unwrap();
        redis.insert(field.into(), path.to_string_lossy().into_owned().into());
    }
    overlay.insert("redis".into(), redis.into());
    fs::write(
        source.join("app.test.toml"),
        toml::to_string(&overlay).unwrap(),
    )
    .unwrap();
    let captured = Captured::load(&source, false).unwrap();
    let original = captured.derived_files(&captured.selected).unwrap();
    let first = Configuration::from_directory(scope.path(), &source, "parallel", false).unwrap();
    let second = Configuration::from_directory(scope.path(), &source, "parallel", false).unwrap();
    let left = first.app.redis.as_ref().unwrap();
    let right = captured.app.redis.as_ref().unwrap();
    assert_ne!(
        left.namespace(),
        second.app.redis.as_ref().unwrap().namespace()
    );
    assert_ne!(left.namespace(), right.namespace());
    assert!(left.password == right.password && left.host == right.host && left.port == right.port);
    assert_eq!(
        (
            left.database,
            left.max_pool_size,
            left.timeout_secs,
            left.mode,
            left.tls
        ),
        (
            right.database,
            right.max_pool_size,
            right.timeout_secs,
            right.mode,
            right.tls
        )
    );
    assert!(
        left.tls_ca == right.tls_ca
            && left.tls_client_cert == right.tls_client_cert
            && left.tls_client_key == right.tls_client_key
    );
    let target = &first.owned.as_ref().unwrap().directory;
    assert!(fs::read(target.join("app.toml")).unwrap() == original["app.toml"]);
    let mut selected: toml::Table =
        files::parse(&fs::read(target.join("app.test.toml")).unwrap(), target).unwrap();
    let derived = selected.remove("scope_id").unwrap();
    assert_eq!(derived.as_str().unwrap(), first.app.scope_id.as_str());
    assert!(selected == captured.selected);
    captured.verify().unwrap();
    let first_container = first.container.as_ref().unwrap().path().to_owned();
    drop((first, second));
    assert!(!first_container.exists());
    captured.verify().unwrap();
}

#[test]
fn missing_common_stays_missing_and_source_drift_is_not_recaptured() {
    let scope = fixture();
    let source = source(scope.path(), false);
    let captured = Captured::load(&source, false).unwrap();
    let snapshot =
        Configuration::from_directory(scope.path(), &source, "no-common", false).unwrap();
    assert!(
        !snapshot
            .owned
            .as_ref()
            .unwrap()
            .directory
            .join("app.toml")
            .exists()
    );
    drop(snapshot);
    fs::write(source.join("app.test.toml"), format!("changed='{SECRET}'")).unwrap();
    let error = captured.derived_files(&captured.selected).err().unwrap();
    safe(&error, &source.join("app.test.toml"));
    assert!(error.contains("漂移") && error.contains("禁止重新捕获"));
}

#[test]
fn owned_directory_never_overwrites_or_deletes_foreign_or_changed_files() {
    let scope = fixture();
    let destination = scope.path().join("owned");
    fs::create_dir(&destination).unwrap();
    let foreign = destination.join("app.test.toml");
    fs::write(&foreign, SECRET).unwrap();
    let files =
        std::collections::BTreeMap::from([("app.test.toml".into(), b"scope_id='test'".to_vec())]);
    let error = OwnedConfiguration::create(&destination, &files)
        .err()
        .unwrap();
    safe(&error, &destination);
    assert!(fs::read(&foreign).unwrap() == SECRET.as_bytes());
    fs::remove_file(&foreign).unwrap();
    fs::remove_dir(&destination).unwrap();
    let owned = OwnedConfiguration::create(&destination, &files).unwrap();
    let foreign = destination.join("foreign.txt");
    fs::write(&foreign, SECRET).unwrap();
    assert!(owned.remove().is_err());
    assert!(foreign.exists() && destination.join("app.test.toml").exists());
    fs::remove_file(&foreign).unwrap();
    let changed = scope.path().join("changed");
    let owned = OwnedConfiguration::create(&changed, &files).unwrap();
    fs::write(changed.join("app.test.toml"), SECRET).unwrap();
    assert!(owned.remove().is_err());
    assert!(fs::read(changed.join("app.test.toml")).unwrap() == SECRET.as_bytes());
}

#[test]
fn https_metadata_is_strict_and_ca_is_captured_with_identity() {
    let scope = fixture();
    let source = source(scope.path(), true);
    let path = source.join("test.toml");
    fs::write(&path, format!("unknown='{SECRET}'")).unwrap();
    let error = Captured::load(&source, true).err().unwrap();
    safe(&error, &path);
    let ca = scope.path().join("ca.pem");
    fs::write(&ca, b"ca-fixture").unwrap();
    fs::write(
        &path,
        format!("ca_certificate={}\n", serde_json::to_string(&ca).unwrap()),
    )
    .unwrap();
    let snapshot = Configuration::from_directory(scope.path(), &source, "https", true).unwrap();
    assert_eq!(snapshot.ca().unwrap(), b"ca-fixture");
    drop(snapshot);
    let captured = Captured::load(&source, true).unwrap();
    fs::write(&ca, b"different-ca").unwrap();
    let error = captured.verify().unwrap_err();
    safe(&error, &ca);
}

#[cfg(any(unix, windows))]
#[test]
fn linked_sources_and_ancestors_are_rejected() {
    let scope = fixture();
    let source = source(scope.path(), true);
    let alias = scope.path().join("alias");
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&source, &alias);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(&source, &alias);
    result.expect("必须启用本地符号链接权限，链接场景不能静默跳过");
    assert!(Captured::load(&alias, false).is_err());
    let selected = source.join("app.test.toml");
    fs::remove_file(&selected).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join("app.toml"), &selected).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(source.join("app.toml"), &selected).unwrap();
    assert!(Captured::load(&source, false).is_err());
}
