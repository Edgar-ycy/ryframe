//! 正式运行入口的只读提前校验；只创建隔离文件，不启动服务或发出请求。

use crate::devex::runtime_input;

use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::devex::DevexSuite;

const MAX_OPENAPI_BYTES: u64 = 16 * 1024 * 1024;
const SECRET: &str = "runtime-input-private-marker";
const APP: &str = r#"[rate_limit]
enabled = true
capacity = 100
window_secs = 60
enable_user_rate_limit = false
user_capacity = 500
user_window_secs = 60
api_window_secs = 60
[rate_limit.api_limits]
"POST /api/v1/auth/login" = 5
"GET /api/v1/auth/csrf" = 30
"GET /api/v1/system/posts" = 100
"#;

struct Fixture {
    directory: TempDir,
    config: Value,
    environment: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("runtime-input-")
            .tempdir_in(local)
            .unwrap();
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::create_dir(directory.path().join("openapi")).unwrap();
        fs::write(directory.path().join("config/app.toml"), APP).unwrap();
        fs::write(directory.path().join("config/app.test.toml"), "").unwrap();
        fs::write(
            directory.path().join("openapi/openapi.json"),
            serde_json::to_vec(&openapi()).unwrap(),
        )
        .unwrap();
        let environment = BTreeMap::from([
            ("APP_ENV".into(), "test".into()),
            ("RYFRAME_DEVEX_CONCURRENCY".into(), "10".into()),
            ("RYFRAME_RUNTIME_INPUT_PASSWORD".into(), SECRET.into()),
        ]);
        let mut fixture = Self {
            directory,
            config: config(),
            environment,
        };
        fixture.refresh_digest();
        fixture
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn check(&self, suite: DevexSuite) -> crate::Result<()> {
        runtime_input::validate(self.root(), &self.config, suite, &self.environment)
    }

    fn error(&self, suite: DevexSuite) -> String {
        let error = self.check(suite).unwrap_err().to_string();
        assert!(!error.contains(SECRET));
        assert!(!error.contains("http://"));
        if !error.contains("config_document_invalid：") {
            assert!(!error.contains(self.root().to_str().unwrap()));
        }
        error
    }

    fn refresh_digest(&mut self) {
        let authority = crate::rate_limit::authority(
            self.root(),
            None,
            self.environment
                .get("APP_ENV")
                .map(String::as_str)
                .unwrap_or(""),
        )
        .unwrap();
        let authority = serde_json::to_value(authority).unwrap();
        let bytes = serde_json::to_vec(&authority).unwrap();
        self.config["contract"]["pacing"]["authority_sha256"] =
            json!(hex::encode(Sha256::digest(bytes)));
    }

    fn write_openapi(&self, document: &Value) {
        fs::write(
            self.root().join("openapi/openapi.json"),
            serde_json::to_vec(document).unwrap(),
        )
        .unwrap();
    }
}

fn openapi() -> Value {
    json!({
        "openapi": "3.1.0", "info": {"title": "Runtime input fixture", "version": "1"},
        "x-ryframe-api-prefix": {"value": "/api/v1"},
        "paths": {
            "/api/v1/auth/csrf": {"get": {"operationId": "get_auth_csrf"}},
            "/api/v1/auth/login": {"post": {"operationId": "post_auth_login"}},
            "/api/v1/system/posts": {"get": {"operationId": "get_system_posts"}}
        }
    })
}

fn config() -> Value {
    let slots = (0..100)
        .map(|index| {
            json!({
                "tenant_slot": format!("tenant-{}", index % 10 + 1),
                "user_slot": format!("user-{}", index + 1),
                "client_address": format!("198.18.10.{}", index + 1),
            })
        })
        .collect::<Vec<_>>();
    let bindings = slots
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            json!({
                "tenant_id": format!("actual-{}", slot["tenant_slot"].as_str().unwrap()),
                "username": format!("user-{}", index + 1),
                "password_env": "RYFRAME_RUNTIME_INPUT_PASSWORD",
            })
        })
        .collect::<Vec<_>>();
    let selections = [10, 50, 100]
        .into_iter()
        .map(|count| {
            let distribution = (1..=10)
                .map(|index| (format!("tenant-{index}"), count / 10))
                .collect::<BTreeMap<_, _>>();
            (count.to_string(), json!(distribution))
        })
        .collect::<BTreeMap<_, _>>();
    let workflows = |names: &[&str]| {
        names
            .iter()
            .map(|name| {
                json!({
                    "name": name, "identity_pool": "readers",
                    "steps": [{"operation": "get_system_posts"}],
                })
            })
            .collect::<Vec<_>>()
    };
    json!({
        "schema_version": 1,
        "contract": {
            "dataset_sha256": "a".repeat(64), "environment_sha256": "b".repeat(64),
            "cycles": 10, "timeout_ms": 1000,
            "homepage": {"identity_pool": "readers", "visitor_interval_ms": 250},
            "identity_pools": {"readers": {
                "roles_sha256": "c".repeat(64), "permissions_sha256": "d".repeat(64),
                "client_address_model": "fixed-benchmark-per-user",
                "slots": slots, "selections": selections,
            }},
            "workloads": {
                "api": workflows(&["list", "filter", "page", "login-refresh", "write"]),
                "tenants": workflows(&["list", "write", "export"]),
            },
            "pacing": {
                "authority_sha256": "e".repeat(64), "request_interval_ms": 20001,
                "sample_prepare_wait_ms": 60250, "operation_intervals_ms": {},
            },
        },
        "bindings": {
            "scope_id": "runtime-input-fixture", "api_url": "http://127.0.0.1:19901",
            "frontend_url": "http://127.0.0.1:19902",
            "metrics_urls": {"api": "http://127.0.0.1:19903", "worker": "http://127.0.0.1:19904"},
            "database_metrics_urls": ["http://127.0.0.1:19905"],
            "identity_pools": {"readers": bindings},
        },
    })
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                files.insert(relative, None);
                visit(root, &path, files);
            } else {
                files.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn valid_homepage_api_and_tenants_leave_inputs_and_files_unchanged() {
    let fixture = Fixture::new();
    let files = snapshot(fixture.root());
    let config = fixture.config.clone();
    let environment = fixture.environment.clone();
    for suite in [
        DevexSuite::RuntimeHomepage,
        DevexSuite::RuntimeApi,
        DevexSuite::RuntimeTenants,
    ] {
        fixture.check(suite).unwrap();
    }
    assert_eq!(snapshot(fixture.root()), files);
    assert_eq!(fixture.config, config);
    assert_eq!(fixture.environment, environment);
}

#[test]
fn homepage_uses_cycles_and_ignores_missing_or_invalid_concurrency() {
    let mut fixture = Fixture::new();
    fixture.environment.remove("RYFRAME_DEVEX_CONCURRENCY");
    fixture.check(DevexSuite::RuntimeHomepage).unwrap();
    fixture
        .environment
        .insert("RYFRAME_DEVEX_CONCURRENCY".into(), SECRET.into());
    fixture.check(DevexSuite::RuntimeHomepage).unwrap();
    fixture.config["contract"]["cycles"] = json!(11);
    assert!(
        fixture
            .error(DevexSuite::RuntimeHomepage)
            .contains("实际前 N 身份的租户分布")
    );
}

#[test]
fn concurrency_is_required_unsigned_decimal_u64_and_bounded() {
    let mut fixture = Fixture::new();
    fixture.environment.remove("RYFRAME_DEVEX_CONCURRENCY");
    assert!(fixture.error(DevexSuite::RuntimeApi).contains("10 到 100"));
    for value in [
        "",
        "9",
        "101",
        "+10",
        "-10",
        " 10",
        "10 ",
        "10.0",
        "1e1",
        "１０",
        "10\n",
        "18446744073709551616",
        SECRET,
    ] {
        fixture
            .environment
            .insert("RYFRAME_DEVEX_CONCURRENCY".into(), value.into());
        for suite in [DevexSuite::RuntimeApi, DevexSuite::RuntimeTenants] {
            assert!(fixture.error(suite).contains("无符号十进制整数"));
        }
    }
    for value in ["10", "50", "100", "0010"] {
        fixture
            .environment
            .insert("RYFRAME_DEVEX_CONCURRENCY".into(), value.into());
        fixture.check(DevexSuite::RuntimeApi).unwrap();
        fixture.check(DevexSuite::RuntimeTenants).unwrap();
    }
}

#[test]
fn identity_distribution_and_duplicate_identities_keep_safe_reasons() {
    let mut fixture = Fixture::new();
    fixture.config["contract"]["identity_pools"]["readers"]["selections"]["10"]["tenant-1"] =
        json!(2);
    let error = fixture.error(DevexSuite::RuntimeApi);
    assert!(error.contains("运行时身份池或租户分布校验失败"));
    assert!(error.contains("实际前 N 身份的租户分布必须匹配固定负载声明"));
    fixture.config = config();
    fixture.refresh_digest();
    fixture.config["bindings"]["identity_pools"]["readers"][1] =
        fixture.config["bindings"]["identity_pools"]["readers"][0].clone();
    assert!(
        fixture
            .error(DevexSuite::RuntimeApi)
            .contains("实际选中身份必须为独立用户")
    );
}

#[test]
fn pacing_and_nonexistent_operation_rules_are_rejected_with_reasons() {
    let mut fixture = Fixture::new();
    fixture.config["contract"]["pacing"]["request_interval_ms"] = json!(600);
    let error = fixture.error(DevexSuite::RuntimeApi);
    assert!(error.contains("运行时固定节奏不满足实际限流"));
    assert!(error.contains("实际全局/用户限流"));
    fixture.config["contract"]["pacing"]["request_interval_ms"] = json!(20001);
    fixture.config["contract"]["pacing"]["operation_intervals_ms"]["get_missing"] = json!(1000);
    assert!(
        fixture
            .error(DevexSuite::RuntimeApi)
            .contains("契约中不存在的 operation")
    );
    fixture.config["contract"]["pacing"]["operation_intervals_ms"] = json!({});
    fs::write(
        fixture.root().join("config/app.toml"),
        format!("{APP}\n\"GET /missing\" = 5\n"),
    )
    .unwrap();
    fixture.refresh_digest();
    assert!(
        fixture
            .error(DevexSuite::RuntimeApi)
            .contains("无法用当前契约明确关联操作")
    );
}

#[test]
fn authority_digest_tracks_each_selected_file_configuration() {
    let mut fixture = Fixture::new();
    fixture.config["contract"]["pacing"]["authority_sha256"] = json!("0".repeat(64));
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "实际限流配置与固定节奏摘要不一致"
    );
    fixture.refresh_digest();
    fs::write(
        fixture.root().join("config/app.test.toml"),
        "[rate_limit]\ncapacity = 101\n",
    )
    .unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "实际限流配置与固定节奏摘要不一致"
    );
    fixture.refresh_digest();
    fixture.check(DevexSuite::RuntimeApi).unwrap();
    fs::write(
        fixture.root().join("config/app.test.toml"),
        "[rate_limit]\ncapacity = 102\n",
    )
    .unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "实际限流配置与固定节奏摘要不一致"
    );
    fixture.refresh_digest();
    fixture.check(DevexSuite::RuntimeApi).unwrap();
}

#[test]
fn authority_uses_the_explicit_file_directory_and_ignores_legacy_overrides() {
    let mut fixture = Fixture::new();
    let relative = "中文 隔离配置";
    let directory = fixture.root().join(relative);
    fs::rename(fixture.root().join("config"), &directory).unwrap();
    fixture.config["bindings"]["app_config_dir"] = json!(relative);
    fixture
        .environment
        .insert("APP_CONFIG_DIR".into(), "missing-config".into());
    fixture
        .environment
        .insert("APP_RATE_LIMIT_CAPACITY".into(), "991".into());
    let before = snapshot(fixture.root());
    fixture.check(DevexSuite::RuntimeApi).unwrap();
    fixture.config["bindings"]["app_config_dir"] = json!(directory);
    fixture.check(DevexSuite::RuntimeApi).unwrap();
    assert_eq!(snapshot(fixture.root()), before);
    fs::write(
        directory.join("app.test.toml"),
        "[rate_limit]\ncapacity=137\n",
    )
    .unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "实际限流配置与固定节奏摘要不一致"
    );
}

#[test]
fn authority_rejects_invalid_explicit_configuration_paths_without_echoing_values() {
    let mut fixture = Fixture::new();
    for value in [
        json!(false),
        json!(137),
        json!([]),
        json!({"password": SECRET}),
    ] {
        fixture.config["bindings"]["app_config_dir"] = value;
        assert_eq!(
            fixture.error(DevexSuite::RuntimeApi),
            "bindings.app_config_dir 必须是文本路径；请修正运行测量配置文件"
        );
    }
    for path in ["", "bad\npath", "nul\0path"] {
        fixture.config["bindings"]["app_config_dir"] = json!(path);
        assert!(
            fixture
                .error(DevexSuite::RuntimeApi)
                .contains("config_directory_invalid")
        );
    }
}

#[test]
fn configuration_errors_precede_io_and_keep_constant_details() {
    let mut fixture = Fixture::new();
    fs::remove_file(fixture.root().join("openapi/openapi.json")).unwrap();
    fixture.environment.remove("RYFRAME_DEVEX_CONCURRENCY");
    fixture.config["schema_version"] = json!(2);
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时测量配置无效：运行时配置结构无效"
    );
    fixture.config["schema_version"] = json!(1);
    fixture.config["bindings"]["api_url"] = json!(format!("http://{SECRET}@127.0.0.1/"));
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时测量配置无效：测量端点必须为不含凭据、query 和 fragment 的 HTTP URL"
    );
    fixture.config["bindings"]["api_url"] = json!("http://127.0.0.1:19901");
    fixture.config["contract"]["cycles"] = json!(9);
    assert_eq!(
        fixture.error(DevexSuite::RuntimeHomepage),
        "运行时测量配置无效：每个场景的请求周期 超出范围"
    );
}

#[test]
fn authority_failures_keep_safe_document_details_and_hide_values() {
    let mut fixture = Fixture::new();
    fixture.environment.remove("APP_ENV");
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        concat!(
            "无法读取运行时测量的实际限流配置：config_environment_invalid：",
            "app_env 必须显式设置为 dev 或 test；请修正请求字段"
        )
    );
    fixture.environment.insert("APP_ENV".into(), "test".into());
    fs::write(fixture.root().join("config/app.toml"), format!("[{SECRET}")).unwrap();
    let error = fixture.error(DevexSuite::RuntimeApi);
    assert!(error.starts_with("无法读取运行时测量的实际限流配置：config_document_invalid："));
    assert!(
        error.contains(
            &fixture
                .root()
                .join("config")
                .join("app.toml")
                .display()
                .to_string()
        ),
        "{error}"
    );
    assert!(error.contains("TOML 语法错误"));
    assert!(error.contains("行第"));
    assert!(error.contains("请检查"));
}

#[test]
fn missing_directory_and_oversized_openapi_are_rejected() {
    let fixture = Fixture::new();
    let path = fixture.root().join("openapi/openapi.json");
    fs::remove_file(&path).unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时 OpenAPI 必须是可读取的普通文件"
    );
    fs::create_dir(&path).unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时 OpenAPI 必须是可读取的普通文件"
    );
    fs::remove_dir(&path).unwrap();
    File::create(path)
        .unwrap()
        .set_len(MAX_OPENAPI_BYTES + 1)
        .unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时 OpenAPI 超过 16 MiB"
    );
}

#[test]
fn openapi_exactly_at_limit_is_accepted() {
    let fixture = Fixture::new();
    let mut bytes = serde_json::to_vec(&openapi()).unwrap();
    bytes.resize(MAX_OPENAPI_BYTES as usize, b' ');
    fs::write(fixture.root().join("openapi/openapi.json"), bytes).unwrap();
    fixture.check(DevexSuite::RuntimeApi).unwrap();
}

#[test]
fn openapi_syntax_reports_only_location_and_category() {
    let fixture = Fixture::new();
    for bytes in [
        format!("{{\n\"{SECRET}\": invalid}}").into_bytes(),
        format!("{{\"{SECRET}\":").into_bytes(),
        vec![0xff],
    ] {
        fs::write(fixture.root().join("openapi/openapi.json"), bytes).unwrap();
        let error = fixture.error(DevexSuite::RuntimeApi);
        assert!(error.starts_with("运行时 OpenAPI 不是有效 JSON：第 "));
        assert!(error.contains(" 行第 "));
        assert!(error.ends_with("列，语法错误") || error.ends_with("列，输入不完整"));
    }
}

#[test]
fn catalog_error_keeps_its_constant_reason() {
    let fixture = Fixture::new();
    let mut document = openapi();
    document["x-ryframe-api-prefix"] = Value::Null;
    fixture.write_openapi(&document);
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时 OpenAPI 操作目录无效：契约缺少明确的 API 前缀"
    );
}

#[test]
fn non_runtime_suite_is_rejected() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.error(DevexSuite::RustGate),
        "只有运行时场景可以核验测量输入"
    );
}

#[cfg(unix)]
#[test]
fn symbolic_openapi_is_rejected_even_when_target_is_valid() {
    let fixture = Fixture::new();
    let path = fixture.root().join("openapi/openapi.json");
    let target = fixture.root().join("openapi/target.json");
    fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(target, path).unwrap();
    assert_eq!(
        fixture.error(DevexSuite::RuntimeApi),
        "运行时 OpenAPI 必须是可读取的普通文件"
    );
}
