use super::{
    API, COMMON, INSTANCE, RoleConfiguration, TEST, WORKER, configuration_files,
    configuration_identity,
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
fn derives_scheduler_and_worker_instance_without_changing_other_configuration() {
    let cases: [(&[u8], &[u8]); 6] = [
        (BASE, b"snowflake_worker_id=7\n[jobs]\nmode='external'\nscheduler_enabled=false\n"),
        (BASE, b"snowflake_worker_id=7\n[jobs]\nmode='external'\nscheduler_enabled=true\n"),
        (BASE, API_TEST),
        (BASE, b"snowflake_worker_id=7\n"),
        (b"scope_id='virtual'\n[jobs]\nscheduler_enabled=false\nqueue=['one','two']\n[jobs.limits]\ncount=3\n", API_TEST),
        (b"scope_id='virtual'\nsnowflake_worker_id=7\n[jobs]\nscheduler_enabled=true\n", b"[jobs]\nmode='external'\n"),
    ];
    for (common, selected) in cases {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("fresh");
        fs::create_dir(&source).unwrap();
        fs::write(source.join(COMMON), common).unwrap();
        fs::write(source.join(TEST), selected).unwrap();
        let mut expected = document(&source);
        expected
            .entry("jobs")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap()
            .insert("scheduler_enabled".into(), toml::Value::Boolean(true));
        let configuration = RoleConfiguration::capture(&source, &binding(&source), 2).unwrap();
        let output = root.path().join("roles");
        configuration.publish(&output).unwrap();
        configuration.verify(&output).unwrap();

        for (role, id) in [(API, 7), (WORKER, 2)] {
            let directory = output.join(role);
            expected.insert(INSTANCE.into(), toml::Value::Integer(id));
            let actual = document(&directory);
            assert_eq!(actual, expected);
            assert_eq!(actual["jobs"]["scheduler_enabled"].as_bool(), Some(true));
            assert_eq!(actual[INSTANCE].as_integer(), Some(id));
            assert_eq!(fs::read(directory.join(COMMON)).unwrap(), common);
        }
        assert_eq!(fs::read(source.join(COMMON)).unwrap(), common);
        assert_eq!(fs::read(source.join(TEST)).unwrap(), selected);
    }
}

fn document(directory: &Path) -> toml::Table {
    ryframe_config::AppConfig::read_document(directory, ryframe_config::Environment::Test).unwrap()
}

#[test]
fn refuses_source_identity_drift_invalid_worker_instances_and_scheduler_types() {
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

    let cases: [(&[u8], &[u8], &str); 4] = [
        (
            BASE,
            b"snowflake_worker_id=7\njobs='fixture-private-value'\n",
            "fresh jobs 必须是表",
        ),
        (
            BASE,
            b"snowflake_worker_id=7\n[jobs]\nscheduler_enabled='fixture-private-value'\n",
            "fresh jobs.scheduler_enabled 必须是布尔值",
        ),
        (
            b"jobs='fixture-private-value'\n",
            b"snowflake_worker_id=7\n",
            "fresh jobs 必须是表",
        ),
        (
            b"[jobs]\nscheduler_enabled='fixture-private-value'\n",
            API_TEST,
            "fresh jobs.scheduler_enabled 必须是布尔值",
        ),
    ];
    for (common, selected, message) in cases {
        fs::write(source.join(COMMON), common).unwrap();
        fs::write(source.join(TEST), selected).unwrap();
        let error = RoleConfiguration::capture(&source, &binding(&source), 2)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, message);
        assert!(!error.contains("fixture-private-value"));
        assert_eq!(fs::read(source.join(COMMON)).unwrap(), common);
        assert_eq!(fs::read(source.join(TEST)).unwrap(), selected);
    }
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

    for (role, original) in [(API, &configuration.api), (WORKER, &configuration.worker)] {
        let path = output.join(role).join(TEST);
        let mut changed: toml::Table =
            toml::from_str(std::str::from_utf8(original).unwrap()).unwrap();
        changed["jobs"]
            .as_table_mut()
            .unwrap()
            .insert("scheduler_enabled".into(), toml::Value::Boolean(false));
        fs::write(&path, toml::to_string(&changed).unwrap()).unwrap();
        assert!(configuration.verify(&output).is_err());
        fs::write(&path, original).unwrap();
        configuration.verify(&output).unwrap();
    }
    let mut same_semantics = configuration.api.clone();
    same_semantics.extend_from_slice(b"\n# changed bytes\n");
    fs::write(output.join(API).join(TEST), same_semantics).unwrap();
    assert!(configuration.verify(&output).is_err());
}
