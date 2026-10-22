use std::{fs, path::PathBuf};

use super::{deployment_assets, deployment_snapshot};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let source = super::super::workspace::root_dir();
        let parent = source.join("target/deployment-assets-tests");
        fs::create_dir_all(&parent).unwrap();
        let temporary = tempfile::tempdir_in(parent).unwrap();
        let root = temporary.path().join("后端 fixture");
        fs::create_dir(&root).unwrap();
        for (relative, bytes) in deployment_snapshot(&source) {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let operations = root.join("docs/operations.md");
        fs::create_dir_all(operations.parent().unwrap()).unwrap();
        fs::copy(source.join("docs/operations.md"), operations).unwrap();
        Self {
            _temporary: temporary,
            root,
        }
    }

    fn replace(&self, path: &str, before: &str, after: &str) {
        let path = self.root.join(path);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(before), "篡改前像必须存在");
        fs::write(path, text.replacen(before, after, 1)).unwrap();
    }

    fn violations(&self) -> Vec<String> {
        deployment_assets::source_violations(&self.root).unwrap()
    }
}

#[test]
fn public_templates_are_standalone_valid_production_configs_with_unique_ids() {
    let fixture = Fixture::new();
    assert!(fixture.violations().is_empty());
    let mut ids = std::collections::BTreeSet::new();
    for (role, id) in [
        ("control", 3),
        ("tenant-data", 4),
        ("api", 1),
        ("worker", 2),
    ] {
        let directory = fixture.root.join("deploy/templates").join(role);
        assert!(!directory.join("app.toml").exists());
        let config =
            ryframe_config::AppConfig::load(&directory, ryframe_config::Environment::Prod).unwrap();
        assert_eq!(config.snowflake_worker_id, id);
        assert!(ids.insert(config.snowflake_worker_id));
        if role == "control" {
            let password = config
                .bootstrap
                .admin_password
                .as_deref()
                .expect("control 初始化必须显式配置密码");
            assert!(!password.trim().is_empty());
            assert_eq!(password, "INVALID-EXAMPLE-Admin-Replace!123456789");
        }
    }
    assert!(!fixture.root.join("deploy/config").exists());
    assert!(!fixture.root.join("deploy/tls").exists());
}

#[test]
fn control_template_requires_an_explicit_nonempty_initialization_password() {
    for replacement in ["", "admin_password = \"\""] {
        let fixture = Fixture::new();
        fixture.replace(
            "deploy/templates/control/app.prod.toml",
            "admin_password = \"INVALID-EXAMPLE-Admin-Replace!123456789\"",
            replacement,
        );
        let errors = fixture.violations().join("\n");
        assert!(errors.contains("bootstrap.admin_password"), "{errors}");
        assert!(!errors.contains("INVALID-EXAMPLE-Admin-Replace!123456789"));
    }
}

#[test]
fn compose_rejects_environment_injection_interpolation_and_secrets() {
    for (before, after, expected) in [
        (
            "APP_ENV: prod",
            "APP_ENV: prod\n    APP_APP_PORT: 8081",
            "environment",
        ),
        ("APP_ENV: prod", "APP_ENV: dev", "environment"),
        (
            "registry.example.invalid/ryframe@",
            "${IMAGE}/ryframe@",
            "插值",
        ),
        (
            "  api:\n",
            "  api:\n    env_file: private.env\n",
            "env_file",
        ),
        ("  api:\n", "  api:\n    secrets: [private]\n", "secrets"),
    ] {
        let fixture = Fixture::new();
        fixture.replace("deploy/compose.prod.yml", before, after);
        assert!(
            fixture
                .violations()
                .iter()
                .any(|error| error.contains(expected))
        );
    }
}

#[test]
fn compose_rejects_writable_wrong_or_automatically_created_mounts() {
    for (before, after) in [
        ("        read_only: true", "        read_only: false"),
        (
            "source: ./deploy/config/control",
            "source: ./deploy/config/api",
        ),
        ("target: /opt/ryframe/config", "target: /opt/ryframe/other"),
        ("create_host_path: false", "create_host_path: true"),
        ("source: ./deploy/tls", "source: ./deploy/other-tls"),
    ] {
        let fixture = Fixture::new();
        fixture.replace("deploy/compose.prod.yml", before, after);
        assert!(
            fixture
                .violations()
                .iter()
                .any(|error| error.contains("volumes") || error.contains("挂载"))
        );
    }
}

#[test]
fn compose_rejects_command_and_healthcheck_directory_drift() {
    for before in [
        "command: [\"--config-dir\", \"/opt/ryframe/config\"]",
        "test: [\"CMD\", \"/usr/local/bin/ryframe\", \"--config-dir\", \"/opt/ryframe/config\", \"--healthcheck\"]",
    ] {
        let fixture = Fixture::new();
        fixture.replace(
            "deploy/compose.prod.yml",
            before,
            &before.replace("/opt/ryframe/config", "/wrong/config"),
        );
        assert!(
            fixture
                .violations()
                .iter()
                .any(|error| error.contains("command") || error.contains("healthcheck"))
        );
    }
}

#[test]
fn compose_preserves_security_dependencies_networks_and_digest() {
    for (before, after) in [
        ("  read_only: true", "  read_only: false"),
        ("    - ALL", "    - NET_ADMIN"),
        ("no-new-privileges:true", "no-new-privileges:false"),
        ("pids_limit: 256", "pids_limit: 0"),
        ("stop_grace_period: 15s", "stop_grace_period: 1s"),
        ("noexec,nosuid", "exec,suid"),
        ("condition: service_healthy", "condition: service_started"),
        ("internal: true", "internal: false"),
        ("127.0.0.1:8080:8080", "0.0.0.0:8080:8080"),
        ("  api:\n", "  api:\n    image: other:latest\n"),
        ("@sha256:00000000", "@invalid:00000000"),
        ("  api:\n", "  api:\n    privileged: true\n"),
    ] {
        let fixture = Fixture::new();
        fixture.replace("deploy/compose.prod.yml", before, after);
        assert!(!fixture.violations().is_empty());
    }
}

#[test]
fn production_templates_reject_weak_credentials_duplicate_ids_and_directory_drift() {
    for (before, after, expected) in [
        (
            "INVALID-EXAMPLE-JWT-Replace-Before-Deployment!123456789",
            "weak",
            "auth.jwt_secret",
        ),
        ("snowflake_worker_id = 2", "snowflake_worker_id = 1", "重复"),
        ("/opt/ryframe/locales", "/wrong/locales", "路径"),
        (
            "/opt/ryframe/tls/mysql-ca.pem",
            "/wrong/mysql-ca.pem",
            "证书路径",
        ),
        ("mode = \"external\"", "mode = \"embedded\"", "jobs"),
        (
            "tls_mode = \"verify_identity\"",
            "tls_mode = \"disabled\"",
            "tls_mode",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.replace("deploy/templates/worker/app.prod.toml", before, after);
        let errors = fixture.violations().join("\n");
        assert!(errors.contains(expected), "{errors}");
        assert!(!errors.contains("INVALID-EXAMPLE-JWT-Replace-Before-Deployment!123456789"));
    }
}

#[test]
fn snapshots_and_checks_never_read_or_expose_private_deployment_directories() {
    let fixture = Fixture::new();
    let before = deployment_snapshot(&fixture.root);
    for directory in ["deploy/config", "deploy/tls"] {
        let directory = fixture.root.join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("PRIVATE-SENTINEL"), [0xff, 0xfe, 0x00]).unwrap();
    }
    assert_eq!(deployment_snapshot(&fixture.root), before);
    let errors = fixture.violations();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!format!("{before:?}{errors:?}").contains("PRIVATE-SENTINEL"));
    assert_eq!(
        fs::read(fixture.root.join("deploy/config/PRIVATE-SENTINEL")).unwrap(),
        [0xff, 0xfe, 0x00]
    );
}

#[test]
fn malformed_compose_fails_without_echoing_values() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("deploy/compose.prod.yml"),
        "services:\n  api: [PRIVATE-SENTINEL",
    )
    .unwrap();
    let errors = fixture.violations().join("\n");
    assert!(errors.contains("YAML"));
    assert!(!errors.contains("PRIVATE-SENTINEL"));
}
