use super::*;
use crate::recovery::fixture_control::{
    environment::protocol::tests::published_plan, source_pair::execution::tests::Fixture as Sources,
};
use std::{fs, io::ErrorKind};

struct Fixture {
    _sources: Sources,
    backend: PathBuf,
    bootstrap: PathBuf,
    configuration: PathBuf,
    environment: PathBuf,
    credential: PathBuf,
    receipt: Value,
}

impl Fixture {
    fn new() -> Self {
        let sources = Sources::new();
        let (_, plan) = published_plan(&sources);
        let backend = sources.backend.clone();
        let root = backend.join(".local-tests/prepared-window");
        fs::create_dir(&root).unwrap();
        let execution = PathBuf::from(plan["execution_backend"].as_str().unwrap());
        let configuration = execution.join(".local-tests/window-configuration");
        fs::create_dir_all(&configuration).unwrap();
        fs::write(configuration.join("app.toml"), b"# shared\n").unwrap();
        fs::write(
            configuration.join("app.test.toml"),
            b"scope_id='window-test'\n",
        )
        .unwrap();
        let environment = root.join("environment.json");
        let private = json!({"APP_ENV":"test"});
        json_io::write_new(&environment, &json!({"environment":private})).unwrap();
        let credential = root.join("secret.txt");
        fs::write(&credential, b"synthetic-private-marker").unwrap();
        let receipt = json!({"format_version":1,"kind":"reference-fixture-environment","status":"prepared",
            "services_started":false,"remote_writes":0,"historical_data_used":false,
            "secret_files":{"secret.txt":binding(&credential).unwrap()},"execution_backend":execution,
            "plan":plan,"config_dir":configuration,
            "configuration_sha256":crate::recovery::fixture_control::artifact::runtime::configuration_digest(&configuration).unwrap(),
            "environment_sha256":digest::plan_hash(&private).unwrap()});
        let bootstrap = root.join("bootstrap.json");
        json_io::write_new(&bootstrap, &receipt).unwrap();
        Self {
            _sources: sources,
            backend,
            bootstrap,
            configuration,
            environment,
            credential,
            receipt,
        }
    }
}

#[test]
fn prepared_window_preserves_original_public_result_and_all_consumed_file_bytes() {
    let fixture = Fixture::new();
    let paths = [
        &fixture.bootstrap,
        &fixture.environment,
        &fixture.credential,
    ];
    let before = paths
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let value = load(&fixture.backend, &fixture.bootstrap, &json!(["secret.txt"])).unwrap();
    assert_eq!(
        value,
        json!([fixture.bootstrap, fixture.receipt["execution_backend"], {"APP_ENV":"test"}, fixture.receipt])
    );
    for (path, bytes) in paths.into_iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn prepared_window_rejects_configuration_environment_credential_and_bootstrap_changes() {
    let fixture = Fixture::new();
    for path in [
        fixture.configuration.join("app.test.toml"),
        fixture.environment.clone(),
        fixture.credential.clone(),
        fixture.bootstrap.clone(),
    ] {
        let original = fs::read(&path).unwrap();
        let window =
            Window::open(&fixture.backend, &fixture.bootstrap, &json!(["secret.txt"])).unwrap();
        match fs::write(&path, b"changed-window-input") {
            Ok(()) => assert!(window.verify().is_err()),
            Err(error) => {
                assert!(
                    error.kind() == ErrorKind::PermissionDenied
                        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
                    "受保护文件写入出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
                    error.kind(),
                    error.raw_os_error()
                );
                assert_eq!(fs::read(&path).unwrap(), original);
                window.verify().unwrap();
            }
        }
        drop(window);
        fs::write(path, original).unwrap();
    }
    let retained = fixture
        .configuration
        .with_file_name("retained-configuration");
    let window =
        Window::open(&fixture.backend, &fixture.bootstrap, &json!(["secret.txt"])).unwrap();
    let replaced = match fs::rename(&fixture.configuration, &retained) {
        Ok(()) => {
            fs::create_dir(&fixture.configuration).unwrap();
            for name in ["app.toml", "app.test.toml"] {
                fs::copy(retained.join(name), fixture.configuration.join(name)).unwrap();
            }
            assert!(window.verify().is_err());
            true
        }
        Err(error) => {
            assert!(
                error.kind() == ErrorKind::PermissionDenied
                    || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
                "受保护配置目录替换出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
                error.kind(),
                error.raw_os_error()
            );
            window.verify().unwrap();
            false
        }
    };
    drop(window);
    if replaced {
        for name in ["app.toml", "app.test.toml"] {
            fs::remove_file(fixture.configuration.join(name)).unwrap();
        }
        fs::remove_dir(&fixture.configuration).unwrap();
        fs::rename(retained, &fixture.configuration).unwrap();
    }
}

#[test]
fn prepared_window_rejects_same_bytes_bootstrap_and_secret_replacement_and_keeps_failure_postchecks()
 {
    let fixture = Fixture::new();
    for path in [&fixture.bootstrap, &fixture.credential] {
        let original = fs::read(path).unwrap();
        let retained = path.with_extension("retained");
        let window =
            Window::open(&fixture.backend, &fixture.bootstrap, &json!(["secret.txt"])).unwrap();
        let replaced = match fs::rename(path, &retained) {
            Ok(()) => {
                fs::write(path, &original).unwrap();
                assert!(window.verify().is_err());
                true
            }
            Err(error) => {
                assert!(
                    error.kind() == ErrorKind::PermissionDenied
                        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
                    "受保护文件同字节替换出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
                    error.kind(),
                    error.raw_os_error()
                );
                window.verify().unwrap();
                false
            }
        };
        drop(window);
        if replaced {
            fs::remove_file(path).unwrap();
            fs::rename(&retained, path).unwrap();
        }
    }
    let configuration = fixture.configuration.join("app.test.toml");
    let original = fs::read(&configuration).unwrap();
    fs::write(&configuration, b"changed-config").unwrap();
    let error = load(&fixture.backend, &fixture.bootstrap, &json!(["secret.txt"])).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("夹具配置目录或配置文件与 bootstrap 绑定不一致"));
    assert!(message.contains("prepared 后验同时失败"));
    assert!(!message.contains("synthetic-private-marker"));
    fs::write(configuration, original).unwrap();
}
