use super::*;
use std::cell::Cell;

struct NoSourceCommands(Cell<usize>);

impl request::SourceObserver for NoSourceCommands {
    fn snapshot(&self, _: &Path) -> Result<Value> {
        self.0.set(self.0.get() + 1);
        Err("路径失败前不应观察执行源码".into())
    }
}

#[test]
fn canonical_cli_request_and_native_target_share_required_config_and_real_toml() {
    let fixture = Fixture::new();
    let canonical = crate::recovery::clone::native::target_verify::binding_request::validate(
        &fixture.backend,
        &fixture.request,
        &Value::Null,
    )
    .unwrap();
    assert_eq!(canonical["selected"], fixture.review["scopes"]["seed"]);
    assert_eq!(
        fixture.request["target"]["databases"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let binding = fixture.validate().unwrap();
    let generation = request_configuration::validate(
        &fixture.backend,
        &fixture.request,
        &binding,
        &fixture.environment,
    )
    .unwrap();
    assert_eq!(
        generation["configuration"]["directory"],
        fixture.request["config_dir"]
    );
    let file = fixture.backend.join("config/app.test.toml");
    let descriptor = bound(&file);
    assert_eq!(
        generation["configuration"]["files"]["app.test.toml"]["sha256"],
        descriptor["sha256"]
    );
    assert_eq!(
        generation["configuration"]["files"]["app.test.toml"]["bytes"],
        descriptor["bytes"]
    );
    manifest::bound_file(
        &crate::workspace::root_dir(),
        &descriptor,
        "真实配置文件",
        true,
    )
    .unwrap();
}

#[test]
fn missing_retired_dual_relative_and_outside_config_fail_before_source_commands() {
    let fixture = Fixture::new();
    let mut missing = fixture.request.clone();
    missing.as_object_mut().unwrap().remove("config_dir");
    let mut retired = missing.clone();
    retired["configuration_directory"] = fixture.request["config_dir"].clone();
    let mut dual = fixture.request.clone();
    dual["configuration_directory"] = fixture.request["config_dir"].clone();
    let mut invalid = vec![missing, retired, dual];
    for path in [
        Value::Null,
        json!(""),
        json!("config"),
        json!(7),
        json!(fixture.backend.parent().unwrap()),
        json!(fixture.backend.join("config/app.toml")),
    ] {
        let mut request = fixture.request.clone();
        request["config_dir"] = path;
        invalid.push(request);
    }
    for request in invalid {
        let observer = NoSourceCommands(Cell::new(0));
        assert!(request::validate_with(&fixture.backend, &request, &observer).is_err());
        assert_eq!(observer.0.get(), 0);
        assert!(
            crate::recovery::clone::native::target_verify::binding_request::validate(
                &fixture.backend,
                &request,
                &Value::Null,
            )
            .is_err()
        );
    }
}

#[test]
fn nested_execution_rejects_coordinator_config_and_accepts_its_own_config_suffix() {
    let fixture = Fixture::new();
    let execution = fixture.local.join("execution");
    let directory = execution.join("private").join("config");
    fs::create_dir_all(&directory).unwrap();
    for name in ["app.toml", "app.test.toml"] {
        fs::copy(
            fixture.backend.join("config").join(name),
            directory.join(name),
        )
        .unwrap();
    }
    let mut request = fixture.request.clone();
    request["execution_backend"] = json!({"path": execution});
    assert!(
        request_configuration::fresh_files(&fixture.backend, &fixture.environment, &request)
            .is_err()
    );
    request["config_dir"] = json!(directory);
    let (actual, _) =
        request_configuration::fresh_files(&execution, &fixture.environment, &request).unwrap();
    assert_eq!(actual, directory);
    request["config_dir"] = json!(execution.join("private"));
    assert!(
        request_configuration::fresh_files(&execution, &fixture.environment, &request).is_err()
    );
}

#[test]
fn selected_toml_missing_changed_or_rebound_is_rejected_without_config_fallback() {
    let fixture = Fixture::new();
    let binding = fixture.validate().unwrap();
    let selected = fixture.backend.join("config/app.test.toml");
    let descriptor = bound(&selected);
    fs::write(&selected, "# 配置字节已变化\n").unwrap();
    assert!(
        request_configuration::validate(
            &fixture.backend,
            &fixture.request,
            &binding,
            &fixture.environment
        )
        .is_err()
    );
    assert!(
        manifest::bound_file(
            &crate::workspace::root_dir(),
            &descriptor,
            "真实配置文件",
            true
        )
        .is_err()
    );
    fs::remove_file(&selected).unwrap();
    assert!(
        request_configuration::fresh_files(
            &fixture.backend,
            &fixture.environment,
            &fixture.request
        )
        .is_err()
    );
    let wrong_environment = BTreeMap::from([("APP_ENV".into(), "prod".into())]);
    assert!(
        request_configuration::fresh_files(&fixture.backend, &wrong_environment, &fixture.request)
            .is_err()
    );
}

#[test]
fn linked_config_is_rejected_before_source_commands() {
    let fixture = Fixture::new();
    let parent = fixture.local.join("alias");
    fs::create_dir(&parent).unwrap();
    let alias = parent.join("config");
    let source = fixture.backend.join("config");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        assert!(
            std::process::Command::new("cmd")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(&alias)
                .arg(&source)
                .creation_flags(0x0800_0000)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let mut request = fixture.request.clone();
    request["config_dir"] = json!(alias);
    let observer = NoSourceCommands(Cell::new(0));
    assert!(request::validate_with(&fixture.backend, &request, &observer).is_err());
    assert_eq!(observer.0.get(), 0);
    #[cfg(windows)]
    fs::remove_dir(&alias).unwrap();
    #[cfg(unix)]
    fs::remove_file(&alias).unwrap();
}
