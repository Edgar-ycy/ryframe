use super::{
    API, COMMON, RoleConfiguration, TEST, WORKER, configuration_files, configuration_identity,
};
use serde_json::json;
use std::{collections::BTreeMap, fs, path::Path};

const BASE: &[u8] = b"# fixture common defaults\r\nscope_id = 'virtual'\r\n";
const API_TEST: &[u8] = b"snowflake_worker_id = 7\r\n\r\n[jobs]\r\nmode = 'external'\r\n";

fn binding(source: &Path) -> String {
    let (directory, files) = configuration_files(
        source.parent().unwrap(),
        &BTreeMap::from([("APP_ENV".into(), "test".into())]),
        Some(&json!(source)),
    )
    .unwrap();
    configuration_identity(&directory, &files).unwrap()
}

#[test]
fn publishes_api_bytes_and_changes_only_the_worker_instance() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("fresh");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(COMMON), BASE).unwrap();
    fs::write(source.join(TEST), API_TEST).unwrap();
    let configuration = RoleConfiguration::capture(&source, &binding(&source), 2).unwrap();
    let output = root.path().join("roles");
    configuration.publish(&output).unwrap();
    configuration.verify(&output).unwrap();

    let api = output.join(API);
    let worker = output.join(WORKER);
    assert_eq!(fs::read(api.join(COMMON)).unwrap(), BASE);
    assert_eq!(fs::read(api.join(TEST)).unwrap(), API_TEST);
    assert_eq!(fs::read(worker.join(COMMON)).unwrap(), BASE);
    let expected = std::str::from_utf8(API_TEST)
        .unwrap()
        .replace("snowflake_worker_id = 7", "snowflake_worker_id = 2");
    let expected = if cfg!(windows) {
        expected
    } else {
        expected.replace("\r\n", "\n")
    };
    assert_eq!(fs::read(worker.join(TEST)).unwrap(), expected.as_bytes());
    assert_eq!(fs::read(source.join(TEST)).unwrap(), API_TEST);
}

#[test]
fn refuses_source_identity_drift_and_invalid_worker_instances() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("fresh");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(TEST), API_TEST).unwrap();
    let expected = binding(&source);
    assert!(RoleConfiguration::capture(&source, &expected, 7).is_err());
    assert!(RoleConfiguration::capture(&source, &expected, 1024).is_err());

    let configuration = RoleConfiguration::capture(&source, &expected, 1023).unwrap();
    assert!(configuration.common.is_none());
    fs::write(source.join(TEST), b"snowflake_worker_id = 8\n").unwrap();
    assert!(RoleConfiguration::capture(&source, &expected, 2).is_err());
}

#[test]
fn protects_existing_output_and_rejects_extra_members_and_changed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("fresh");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(TEST), API_TEST).unwrap();
    let configuration = RoleConfiguration::capture(&source, &binding(&source), 2).unwrap();

    let existing = root.path().join("existing");
    fs::create_dir(&existing).unwrap();
    let sentinel = existing.join("prior");
    fs::write(&sentinel, b"keep").unwrap();
    assert!(configuration.publish(&existing).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    assert!(!existing.join(API).exists());

    let output = root.path().join("roles");
    configuration.publish(&output).unwrap();
    assert!(!output.join(API).join(COMMON).exists());
    assert!(!output.join(WORKER).join(COMMON).exists());

    let extra = output.join(API).join("extra");
    fs::write(&extra, b"unexpected").unwrap();
    assert!(configuration.verify(&output).is_err());
    fs::remove_file(extra).unwrap();
    configuration.verify(&output).unwrap();

    fs::write(output.join(WORKER).join(TEST), API_TEST).unwrap();
    assert!(configuration.verify(&output).is_err());
}
