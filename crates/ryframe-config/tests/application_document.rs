use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use ryframe_config::{AppConfig, Environment, JobWorkerMode};

const SECRET: &str = "application-document-private-marker";

struct Fixture(PathBuf);

impl Fixture {
    fn new(common: Option<&str>, selected: Option<&str>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/application-document");
        fs::create_dir_all(&parent).unwrap();
        let directory = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        for (name, source) in [("app.toml", common), ("app.dev.toml", selected)] {
            if let Some(source) = source {
                fs::write(directory.join(name), source).unwrap();
            }
        }
        Self(directory)
    }

    fn complete(edit: impl FnOnce(&mut toml::Table)) -> Self {
        let mut table: toml::Table =
            toml::from_str(include_str!("../../../config/app.toml")).unwrap();
        edit(&mut table);
        Self::new(Some(&toml::to_string(&table).unwrap()), Some(""))
    }

    fn document(&self) -> toml::Table {
        AppConfig::read_document(&self.0, Environment::Dev).unwrap()
    }

    fn load(&self) -> AppConfig {
        AppConfig::load(&self.0, Environment::Dev).unwrap()
    }

    fn assert_shared_error(&self, field: &str, reason: &str) {
        let before = self.snapshot();
        let raw = AppConfig::read_document(&self.0, Environment::Dev)
            .unwrap_err()
            .to_string();
        let typed = AppConfig::load(&self.0, Environment::Dev)
            .unwrap_err()
            .to_string();
        assert_eq!(typed, raw);
        assert!(raw.contains(&self.0.display().to_string()), "{raw}");
        assert!(raw.contains(field), "{raw}");
        assert!(raw.contains(reason), "{raw}");
        assert!(raw.contains("请"), "{raw}");
        assert!(!raw.contains(SECRET), "{raw}");
        assert_eq!(self.snapshot(), before);
    }

    fn snapshot(&self) -> [Option<Vec<u8>>; 2] {
        ["app.toml", "app.dev.toml"].map(|name| fs::read(self.0.join(name)).ok())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn selected_only_document_is_valid_and_matches_the_full_loader_without_writes() {
    let fixture = Fixture::complete(|table| {
        table["rate_limit"]["capacity"] = 137.into();
    });
    let expected = fixture.document();
    fs::write(
        fixture.0.join("app.dev.toml"),
        toml::to_string(&expected).unwrap(),
    )
    .unwrap();
    fs::remove_file(fixture.0.join("app.toml")).unwrap();
    let before = fixture.snapshot();
    assert_eq!(fixture.document(), expected);
    let loaded = fixture.load();
    assert_eq!(loaded.rate_limit.capacity, 137);
    assert_eq!(loaded.environment, Environment::Dev);
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn selected_environment_is_required_even_with_a_valid_common_document() {
    for common in [None, Some(include_str!("../../../config/app.toml"))] {
        let fixture = Fixture::new(common, None);
        fixture.assert_shared_error("app.dev.toml", "UTF-8");
    }
}

#[test]
fn invalid_common_is_never_ignored_when_selected_is_valid() {
    let malformed = format!("password='{SECRET}'\n[broken");
    let fixture = Fixture::new(Some(&malformed), Some("[rate_limit]\ncapacity=137\n"));
    fixture.assert_shared_error("app.toml", "TOML 语法错误");
    let raw = AppConfig::read_document(&fixture.0, Environment::Dev)
        .unwrap_err()
        .to_string();
    assert!(raw.contains("行第"));
    fs::write(fixture.0.join("app.toml"), [0xff, 0xfe]).unwrap();
    fixture.assert_shared_error("app.toml", "UTF-8");
}

#[test]
fn tables_merge_recursively_and_arrays_replace_instead_of_append() {
    let fixture = Fixture::new(
        Some("[probe]\nkeep=1\nitems=[1,2]\n[probe.child]\nleft='common'\nright=2\n"),
        Some("[probe]\nitems=[3]\n[probe.child]\nright=4\n"),
    );
    let before = fixture.snapshot();
    let table = fixture.document();
    assert_eq!(table["probe"]["keep"].as_integer(), Some(1));
    assert_eq!(table["probe"]["child"]["left"].as_str(), Some("common"));
    assert_eq!(table["probe"]["child"]["right"].as_integer(), Some(4));
    assert_eq!(
        table["probe"]["items"].as_array().unwrap(),
        &[toml::Value::Integer(3)]
    );
    assert!(AppConfig::load(&fixture.0, Environment::Dev).is_err());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn document_and_load_share_secret_and_removed_database_prechecks() {
    let encoded = format!("[auth]\njwt_secret='ENC[{SECRET}]'\n");
    let fixture = Fixture::new(None, Some(&encoded));
    fixture.assert_shared_error("auth.jwt_secret", "ENC[...]");
    let removed = format!("[database.primary]\ndriver='{SECRET}'\n");
    let fixture = Fixture::new(None, Some(&removed));
    fixture.assert_shared_error("database.driver", "was removed");
}

#[test]
fn document_and_load_share_flattened_connection_prechecks() {
    for collection in ["replicas", "sources"] {
        let fixture = Fixture::complete(|table| {
            let mut connection = table["database"]["primary"].as_table().unwrap().clone();
            connection.insert("name".into(), "document-connection".into());
            connection.insert("port".into(), SECRET.into());
            table["database"].as_table_mut().unwrap().insert(
                collection.into(),
                toml::Value::Array(vec![toml::Value::Table(connection)]),
            );
        });
        fixture.assert_shared_error(&format!("database.{collection}[0].port"), "类型不正确");
    }
}

#[test]
fn raw_document_does_not_claim_complete_typed_or_runtime_validation() {
    let fixture = Fixture::complete(|table| {
        table["rate_limit"]["capacity"] = SECRET.into();
    });
    assert_eq!(
        fixture.document()["rate_limit"]["capacity"].as_str(),
        Some(SECRET)
    );
    let error = AppConfig::load(&fixture.0, Environment::Dev)
        .unwrap_err()
        .to_string();
    assert!(error.contains("capacity"), "{error}");
    assert!(error.contains("反序列化失败"), "{error}");
    assert!(error.contains("请"), "{error}");
    assert!(!error.contains(SECRET), "{error}");
    let fixture = Fixture::complete(|table| {
        table["app"]["port"] = 0.into();
    });
    assert_eq!(fixture.document()["app"]["port"].as_integer(), Some(0));
    let error = AppConfig::load(&fixture.0, Environment::Dev)
        .unwrap_err()
        .to_string();
    assert!(error.contains("app.port"), "{error}");
    assert!(error.contains("必须大于 0"), "{error}");
}

#[test]
fn runtime_job_defaults_are_applied_only_by_load_and_explicit_modes_survive() {
    let fixture = Fixture::complete(|table| {
        table["jobs"].as_table_mut().unwrap().remove("mode");
    });
    let before = fixture.snapshot();
    assert!(fixture.document()["jobs"].get("mode").is_none());
    assert_eq!(fixture.load().jobs.mode, JobWorkerMode::Embedded);
    assert_eq!(fixture.snapshot(), before);
    fs::write(fixture.0.join("app.dev.toml"), "[jobs]\nmode='external'\n").unwrap();
    assert_eq!(
        fixture.document()["jobs"]["mode"].as_str(),
        Some("external")
    );
    assert_eq!(fixture.load().jobs.mode, JobWorkerMode::External);
}
