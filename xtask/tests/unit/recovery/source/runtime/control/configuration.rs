use super::*;
use crate::recovery::clone::native::target_verify::input::request_configuration::canonical_path;

const PUBLIC: &str = "# 公共配置逐字节保留\r\n[api_docs]\r\nenabled = true\r\n";
const PRIVATE: &str =
    "snowflake_worker_id = 17\nscope_id = 'snapshot-original'\n[api_docs]\nenabled = false\n";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests/source-config-snapshot-unit");
        fs::create_dir_all(&base).unwrap();
        let temp = tempfile::tempdir_in(base).unwrap();
        let root = canonical_path(temp.path()).unwrap();
        let local = root.join(".local-tests");
        let source = local.join("source-config");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("app.test.toml"), PRIVATE).unwrap();
        Self {
            _temp: temp,
            root,
            source,
            output: local.join("runtime-configuration"),
        }
    }

    fn roles(&self) -> [PathBuf; 2] {
        [
            self.output.join("config"),
            self.output.join("worker-config"),
        ]
    }

    fn assert_role_fields(&self, input: &str) {
        let mut expected: toml::Table = toml::from_str(input).unwrap();
        for (role, id) in self.roles().into_iter().zip([17, 2]) {
            expected.insert("snowflake_worker_id".into(), toml::Value::Integer(id));
            let content = fs::read_to_string(role.join("app.test.toml")).unwrap();
            let actual: toml::Table = toml::from_str(&content).unwrap();
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn source_configuration_snapshots_both_roles_and_refuses_replay() {
    for public in [None, Some(PUBLIC)] {
        let fixture = Fixture::new();
        if let Some(public) = public {
            fs::write(fixture.source.join("app.toml"), public).unwrap();
        }
        let [api, worker] = fixture.roles();
        let source_before = source_configuration::capture(&fixture.root, &fixture.source).unwrap();
        assert_eq!(
            copy(&fixture.root, &fixture.output, &fixture.source).unwrap(),
            (api, worker)
        );
        fixture.assert_role_fields(PRIVATE);
        for role in fixture.roles() {
            let shared = role.join("app.toml");
            if let Some(public) = public {
                assert_eq!(fs::read(shared).unwrap(), public.as_bytes());
            } else {
                assert!(!shared.exists());
            }
        }

        let snapshot = || {
            fixture
                .roles()
                .map(|role| source_configuration::capture(&fixture.root, &role).unwrap())
        };
        let before = snapshot();
        let error = copy(&fixture.root, &fixture.output, &fixture.source).unwrap_err();
        assert!(error.to_string().contains("禁止覆盖"));
        assert_eq!(snapshot(), before);
        assert_eq!(
            source_configuration::capture(&fixture.root, &fixture.source).unwrap(),
            source_before
        );
    }
}

#[test]
fn source_configuration_rejects_drift_after_capture_and_preserves_snapshots() {
    let changed = PRIVATE.replace("snapshot-original", "snapshot-modified");
    for (filename, contents, expected) in [
        ("app.toml", PUBLIC, PRIVATE),
        ("app.test.toml", changed.as_str(), changed.as_str()),
    ] {
        let fixture = Fixture::new();
        let before = source_configuration::capture(&fixture.root, &fixture.source).unwrap();
        fs::write(fixture.source.join(filename), contents).unwrap();
        let drifted = source_configuration::capture(&fixture.root, &fixture.source).unwrap();
        assert_ne!(drifted, before);

        let error = copy_captured(&fixture.root, &fixture.output, &before).unwrap_err();
        assert!(error.to_string().contains("源配置在快照期间漂移"));
        fixture.assert_role_fields(expected);
        for role in fixture.roles() {
            assert!(!role.join("app.toml").exists());
        }
        assert_eq!(
            source_configuration::capture(&fixture.root, &fixture.source).unwrap(),
            drifted
        );
    }
}
