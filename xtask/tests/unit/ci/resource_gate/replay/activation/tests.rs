use super::*;
use crate::ci::integration_configuration::test_fixture::{BASE, SECRET, fixture};
use crate::ci::resource_gate::replay::{
    ProcessEnvironment, ReplayRequest, ReplayRuntime, ToolExecutables, execute,
};
use std::{fs, io::Write};

fn write_roles(root: &Path) {
    for relative in [MYSQL, REDIS] {
        let directory = root.join(relative);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("app.test.toml"), BASE).unwrap();
    }
}

fn request(root: &Path, activation_gate: bool) -> ReplayRequest {
    let manifest = root.join("manifest.json");
    if !manifest.exists() {
        let categories = [
            "addition",
            "field",
            "permission",
            "relation",
            "sql",
            "rename",
            "delete",
        ];
        let cases = (0..20)
            .map(|index| {
                serde_json::json!({
                    "name": format!("case-{index}"),
                    "category": categories[index % categories.len()],
                    "base": format!("{:040x}", index + 1),
                    "head": format!("{:040x}", index + 101),
                    "frontendBase": format!("{:040x}", index + 201),
                    "frontendHead": format!("{:040x}", index + 301),
                    "targetedMode": if index % 2 == 0 { "targeted" } else { "full" },
                    "expected": if index % 2 == 0 { "pass" } else { "fail" },
                })
            })
            .collect::<Vec<_>>();
        let command = ["cargo", "xtask", "check", "ci", "resource-gate"];
        let content = serde_json::to_vec(&serde_json::json!({
            "formatVersion": 2,
            "targetedCommand": command,
            "fullCommand": command,
            "cases": cases,
        }))
        .unwrap();
        fs::File::create_new(&manifest)
            .unwrap()
            .write_all(&content)
            .unwrap();
    }
    ReplayRequest {
        backend: root.to_owned(),
        frontend: root.join("frontend"),
        manifest,
        work_dir: root.join(".local-tests/resource-gate-replay/work"),
        report: root.join(".local-tests/resource-gate-replay/report.json"),
        activation_gate,
    }
}

fn early_error(root: &Path, required: bool, value: &str) -> String {
    let request = request(root, required);
    let manifest = fs::read(&request.manifest).unwrap();
    let environment: ProcessEnvironment = [
        "RYFRAME_MYSQL_INTEGRATION",
        "RYFRAME_MYSQL_TLS_INTEGRATION",
        "RYFRAME_REDIS_INTEGRATION",
        "RYFRAME_MYSQL_HOST",
        "RYFRAME_REDIS_HOST",
        "RYFRAME_REDIS_DATABASE",
    ]
    .into_iter()
    .map(|key| (key.into(), value.into()))
    .collect();
    // manifest先正常校验；配置错误须在Git前失败，合法配置仅进入只读Git顶层核验。
    let tools = ToolExecutables {
        corepack: root.join("must-not-run-corepack"),
        sccache: root.join("must-not-run-sccache"),
    };
    let result = execute(
        &request,
        ReplayRuntime {
            base_environment: &environment,
            tools: Some(&tools),
        },
    );
    assert_eq!(fs::read(&request.manifest).unwrap(), manifest);
    result.unwrap_err()
}

#[test]
fn activation_rejects_old_flags_without_files_before_any_external_command() {
    let root = fixture();
    for value in ["1", "0", "15", "localhost"] {
        let error = early_error(root.path(), true, value);
        assert!(error.contains("必须提供MySQL和Redis"), "{error}");
    }
    let entries = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, [std::ffi::OsString::from("manifest.json")]);
}

#[test]
fn valid_files_ignore_disabled_environment_and_publish_exact_role_bytes() {
    let root = fixture();
    write_roles(root.path());
    let error = early_error(root.path(), true, "0");
    assert!(
        error.contains("后端 --repository 必须指向 Git 仓库顶层"),
        "必须通过manifest及配置验证后才进入Git顶层核验：{error}"
    );
    let inputs = Activation::load(root.path(), true).unwrap();
    let mut environment = BTreeMap::new();
    let evidence = inputs.record(&mut environment).unwrap();
    for key in [
        "mysqlIntegration",
        "mysqlTlsIntegration",
        "redisIntegration",
    ] {
        assert_eq!(environment[key], "1");
    }
    let serialized = serde_json::to_string(&evidence).unwrap();
    assert!(!serialized.contains(SECRET));
    assert!(!serialized.contains("jwt_secret"));
    let files = evidence["mysql"]["files"].as_array().unwrap();
    let selected = files
        .iter()
        .find(|file| file["path"].as_str().unwrap().ends_with("app.test.toml"))
        .unwrap();
    assert_eq!(selected["bytes"], BASE.len());
    assert_eq!(
        selected["sha256"],
        hex::encode(Sha256::digest(BASE.as_bytes()))
    );
    assert!(files.iter().any(|file| file["present"] == false));
    let isolated = root.path().join("isolated");
    fs::create_dir(&isolated).unwrap();
    let published = inputs.publish(&isolated).unwrap();
    for role in [MYSQL, REDIS] {
        assert_eq!(
            fs::read(isolated.join(role).join("app.test.toml")).unwrap(),
            BASE.as_bytes()
        );
    }
    published.verify().unwrap();
    assert!(
        inputs.publish(&isolated).is_err(),
        "已有发布目录必须拒绝，不能重建或覆盖"
    );
    inputs.verify().unwrap();
    fs::write(isolated.join(REDIS).join("app.test.toml"), "changed").unwrap();
    assert!(published.verify().is_err());
    assert!(isolated.join(MYSQL).is_dir(), "漂移现场必须保留");
}

#[test]
fn malformed_or_unsafe_present_roles_fail_even_without_activation() {
    for (role, from, to) in [
        (MYSQL, "host = \"127.0.0.1\"", "host = \"remote.example\""),
        (MYSQL, "[auth]", "tls_mode = \"disabled\"\n[auth]"),
        (MYSQL, "max_connections = 2", "max_connections = 0"),
        (MYSQL, "scope_id = \"service-fixture\"", "scope_id = \"\""),
        (MYSQL, "port = 1", "port = 0"),
        (MYSQL, "username = \"fixture\"", "username = 12"),
        (
            MYSQL,
            "[database.primary]",
            "[database.primary]\nunknown_field = 1",
        ),
        (REDIS, "host = \"127.0.0.1\"", "host = \"remote.example\""),
        (REDIS, "database = 15", "database = 0"),
        (REDIS, "mode = \"required\"", "mode = \"optional\""),
        (REDIS, "timeout_secs = 7", "timeout_secs = 0"),
        (
            REDIS,
            "tls = false",
            "tls = false\ntls_ca = \"missing.pem\"",
        ),
        (REDIS, "[redis]", "[redis]\nunknown_field = 1"),
    ] {
        let root = fixture();
        write_roles(root.path());
        let path = root.path().join(role).join("app.test.toml");
        fs::write(&path, BASE.replace(from, to)).unwrap();
        for required in [false, true] {
            let error = early_error(root.path(), required, "1");
            assert!(
                error.replace('\\', "/").contains(
                    root.path()
                        .join(role)
                        .to_string_lossy()
                        .replace('\\', "/")
                        .as_str()
                ),
                "必须拒绝当前角色配置：{error}"
            );
            assert!(!error.contains(SECRET));
        }
        assert!(
            !root
                .path()
                .join(".local-tests/resource-gate-replay")
                .exists()
        );
    }
}

#[test]
fn missing_selected_file_is_an_error_but_absent_roles_are_recorded_and_guarded() {
    let root = fixture();
    let inputs = Activation::load(root.path(), false).unwrap();
    let mut environment = BTreeMap::new();
    let evidence = inputs.record(&mut environment).unwrap();
    assert_eq!(environment["mysqlIntegration"], "0");
    assert_eq!(environment["mysqlTlsIntegration"], "0");
    assert_eq!(environment["redisIntegration"], "0");
    assert_eq!(evidence["mysql"]["present"], false);
    write_roles(root.path());
    assert!(inputs.verify().is_err(), "缺失来源出现也属于漂移");
    for role in [MYSQL, REDIS] {
        let selected = root.path().join(role).join("app.test.toml");
        fs::remove_file(&selected).unwrap();
        let error = early_error(root.path(), false, "1");
        assert!(
            error
                .replace('\\', "/")
                .contains(selected.to_string_lossy().replace('\\', "/").as_str()),
            "必须拒绝缺失的selected文件：{error}"
        );
        fs::write(&selected, BASE).unwrap();
    }
}

#[test]
fn merged_roles_accept_exact_loopbacks_and_keep_optional_base_binding() {
    for host in ["127.0.0.1", "localhost", "::1"] {
        let root = fixture();
        write_roles(root.path());
        for role in [MYSQL, REDIS] {
            let directory = root.path().join(role);
            let base = format!(
                "# raw base\r\n{}",
                BASE.replace("127.0.0.1", host).replace('\n', "\r\n")
            );
            fs::write(directory.join("app.toml"), base).unwrap();
            fs::write(
                directory.join("app.test.toml"),
                "# selected overlay\r\n\r\n",
            )
            .unwrap();
        }
        let captured = Activation::load(root.path(), true).unwrap();
        let mut environment = BTreeMap::new();
        let record = captured.record(&mut environment).unwrap();
        assert!(
            record["mysql"]["files"]
                .as_array()
                .unwrap()
                .iter()
                .all(|file| file["present"] == true)
        );
        let isolated = root.path().join("isolated");
        fs::create_dir(&isolated).unwrap();
        let published = captured.publish(&isolated).unwrap();
        for role in [MYSQL, REDIS] {
            for name in ["app.toml", "app.test.toml"] {
                assert_eq!(
                    fs::read(root.path().join(role).join(name)).unwrap(),
                    fs::read(isolated.join(role).join(name)).unwrap()
                );
            }
        }
        fs::write(isolated.join(MYSQL).join("unknown.toml"), "unknown-write").unwrap();
        assert!(published.verify().is_err());
        assert!(isolated.join(MYSQL).join("unknown.toml").exists());
        captured.verify().unwrap();
        fs::remove_file(root.path().join(MYSQL).join("app.toml")).unwrap();
        assert!(captured.verify().is_err());
    }
}

#[test]
fn redis_tls_material_drift_and_missing_pair_fail_without_service_access() {
    let root = fixture();
    write_roles(root.path());
    let ca = root.path().join("redis-ca.pem");
    fs::write(&ca, "redis-ca-private-material").unwrap();
    let path = root.path().join(REDIS).join("app.test.toml");
    let tls = format!(
        "tls = true\ntls_ca = {}",
        toml::Value::String(ca.to_str().unwrap().into())
    );
    fs::write(&path, BASE.replace("tls = false", &tls)).unwrap();
    let captured = Activation::load(root.path(), true).unwrap();
    let mut environment = BTreeMap::new();
    let evidence = captured.record(&mut environment).unwrap();
    assert!(!evidence.to_string().contains("redis-ca-private-material"));
    fs::write(&ca, "changed-ca").unwrap();
    assert!(captured.verify().is_err());
    fs::write(
        &path,
        BASE.replace(
            "tls = false",
            &format!(
                "{tls}\ntls_client_cert = {}",
                toml::Value::String(ca.to_str().unwrap().into())
            ),
        ),
    )
    .unwrap();
    let error = early_error(root.path(), false, "1");
    assert!(
        error
            .replace('\\', "/")
            .contains(path.to_string_lossy().replace('\\', "/").as_str()),
        "必须拒绝当前Redis客户端证书配对：{error}"
    );
}

#[test]
fn tls_materials_and_optional_base_cannot_change_after_capture() {
    let root = fixture();
    write_roles(root.path());
    let ca = root.path().join("ca.pem");
    fs::write(&ca, "private-material-sentinel").unwrap();
    let path = root.path().join(MYSQL).join("app.test.toml");
    let ca_field = format!(
        "tls_mode = \"verify_ca\"\ntls_ca = {}\n[auth]",
        toml::Value::String(ca.to_str().unwrap().into())
    );
    fs::write(&path, BASE.replace("[auth]", &ca_field)).unwrap();
    let captured = Activation::load(root.path(), true).unwrap();
    let mut environment = BTreeMap::new();
    assert!(
        !captured
            .record(&mut environment)
            .unwrap()
            .to_string()
            .contains("private-material-sentinel")
    );
    fs::write(&ca, "different-material").unwrap();
    assert!(captured.verify().is_err());
    let captured = Activation::load(root.path(), true).unwrap();
    fs::remove_file(&ca).unwrap();
    assert!(captured.verify().is_err());
    fs::write(&ca, "restored").unwrap();
    let captured = Activation::load(root.path(), true).unwrap();
    fs::write(root.path().join(REDIS).join("app.toml"), "# appeared").unwrap();
    assert!(captured.verify().is_err());
}

#[cfg(any(unix, windows))]
#[test]
fn linked_role_directories_and_selected_files_are_not_treated_as_missing() {
    let root = fixture();
    write_roles(root.path());
    let role = root.path().join(MYSQL);
    let saved = root.path().join("saved-mysql");
    fs::rename(&role, &saved).unwrap();
    directory_link(&saved, &role);
    let error = early_error(root.path(), false, "1");
    assert!(
        error
            .replace('\\', "/")
            .contains(role.to_string_lossy().replace('\\', "/").as_str()),
        "必须拒绝链接角色目录：{error}"
    );
    #[cfg(windows)]
    fs::remove_dir(&role).unwrap();
    #[cfg(unix)]
    fs::remove_file(&role).unwrap();
    fs::rename(&saved, &role).unwrap();
    let selected = role.join("app.test.toml");
    fs::remove_file(&selected).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path().join("missing.toml"), &selected).unwrap();
    #[cfg(windows)]
    directory_link(root.path(), &selected);
    let error = early_error(root.path(), false, "1");
    assert!(
        error
            .replace('\\', "/")
            .contains(selected.to_string_lossy().replace('\\', "/").as_str()),
        "必须拒绝链接selected文件：{error}"
    );
    #[cfg(windows)]
    fs::remove_dir(&selected).unwrap();
    #[cfg(unix)]
    fs::remove_file(&selected).unwrap();
}

#[cfg(any(unix, windows))]
fn directory_link(source: &Path, destination: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(source, destination).unwrap();
    #[cfg(windows)]
    {
        // junction不要求符号链接特权，同时覆盖重解析点拒绝。
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(destination.to_string_lossy().replace('/', "\\"))
            .arg(source.to_string_lossy().replace('/', "\\"))
            .status()
            .unwrap();
        assert!(status.success());
    }
}
