use std::{collections::BTreeSet, fs, path::Path};

use ryframe_config::{AppConfig, Environment};

use crate::{
    Result,
    check::supply_chain::yaml::{self, YamlKey, YamlValue},
};

use super::regex;

const ROLES: [(&str, &str, i64); 4] = [
    ("migrate-control", "control", 3),
    ("migrate-tenant-data", "tenant-data", 4),
    ("api", "api", 1),
    ("worker", "worker", 2),
];
const CONFIG: &str = "/opt/ryframe/config";

pub(super) fn check(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let source = fs::read_to_string(root.join("deploy/compose.prod.yml"))?;
    match yaml::parse(&source) {
        Ok(document) => check_document(&document, violations),
        Err(_) => violations.push("生产 Compose YAML 无法解析；请修正结构、重复键或锚点".into()),
    }
    check_templates(root, violations);
    Ok(())
}

fn check_document(document: &YamlValue, violations: &mut Vec<String>) {
    reject_external_inputs(document, violations);
    let image = document.get("x-ryframe-image").and_then(YamlValue::as_str);
    if !image.is_some_and(|value| regex(r"^[^\s@]+@sha256:[0-9a-f]{64}$").is_match(value)) {
        violations.push("Compose 镜像锚点必须为带完整 sha256 摘要的普通字符串".into());
    }
    let Some(services) = document.get("services") else {
        violations.push("Compose 缺少 services".into());
        return;
    };
    let names = mapping_keys(services);
    let expected = ROLES
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<BTreeSet<_>>();
    if names != Some(expected) {
        violations.push("Compose 必须精确保留四个部署服务".into());
    }
    for (name, role, _) in ROLES {
        if let Some(service) = services.get(name) {
            check_service(name, role, service, image, violations);
        }
    }
    expect_yaml(
        document.get("networks"),
        "edge: null\nbackend:\n  internal: true",
        "networks",
        violations,
    );
}

fn reject_external_inputs(value: &YamlValue, violations: &mut Vec<String>) {
    match value {
        YamlValue::Mapping(entries) => {
            for (key, child) in entries {
                if matches!(key, YamlKey::String(name) if name.contains('$')) {
                    violations.push("Compose 禁止环境插值".into());
                }
                if matches!(key, YamlKey::String(name) if name == "env_file" || name == "secrets") {
                    violations.push("Compose 禁止 env_file 与 secrets 注入".into());
                }
                reject_external_inputs(child, violations);
            }
        }
        YamlValue::Sequence(values) => {
            for child in values {
                reject_external_inputs(child, violations);
            }
        }
        YamlValue::String(value) if value.contains('$') => {
            violations.push("Compose 禁止环境插值".into());
        }
        _ => {}
    }
}

fn check_service(
    name: &str,
    role: &str,
    service: &YamlValue,
    image: Option<&str>,
    violations: &mut Vec<String>,
) {
    if service.get("image").and_then(YamlValue::as_str) != image || image.is_none() {
        violations.push(format!("{name}.image 必须与镜像锚点一致"));
    }
    let allowed = [
        "image",
        "restart",
        "read_only",
        "cap_drop",
        "security_opt",
        "pids_limit",
        "stop_grace_period",
        "tmpfs",
        "environment",
        "networks",
        "entrypoint",
        "command",
        "volumes",
        "depends_on",
        "ports",
        "healthcheck",
    ];
    if mapping_keys(service).is_none_or(|keys| keys.iter().any(|key| !allowed.contains(key))) {
        violations.push(format!("{name} 包含未登记的服务设置"));
    }
    for (field, expected) in [
        ("environment", "APP_ENV: prod"),
        ("read_only", "true"),
        ("cap_drop", "[ALL]"),
        ("security_opt", "['no-new-privileges:true']"),
        ("pids_limit", "256"),
        ("stop_grace_period", "15s"),
        ("tmpfs", "['/tmp:mode=1777,noexec,nosuid,size=64m']"),
    ] {
        expect_yaml(
            service.get(field),
            expected,
            &format!("{name}.{field}"),
            violations,
        );
    }
    check_mounts(name, role, service, violations);
    check_process(name, service, violations);
    check_dependencies(name, service, violations);
}

fn check_mounts(name: &str, role: &str, service: &YamlValue, violations: &mut Vec<String>) {
    let Some(volumes) = service.get("volumes").and_then(YamlValue::as_sequence) else {
        violations.push(format!("{name}.volumes 必须为两个只读 bind mount"));
        return;
    };
    if volumes.len() != 2 {
        violations.push(format!("{name}.volumes 必须精确挂载配置与证书目录"));
    }
    for (source, target) in [
        (format!("./deploy/config/{role}"), CONFIG),
        ("./deploy/tls".into(), "/opt/ryframe/tls"),
    ] {
        let found = volumes
            .iter()
            .filter(|mount| mount.get("target").and_then(YamlValue::as_str) == Some(target))
            .collect::<Vec<_>>();
        let expected = format!(
            "type: bind\nsource: {source}\ntarget: {target}\nread_only: true\nbind:\n  create_host_path: false"
        );
        if found.len() == 1 {
            expect_yaml(
                Some(found[0]),
                &expected,
                &format!("{name}.volumes.{target}"),
                violations,
            );
        } else {
            violations.push(format!("{name} 缺少唯一的 {target} 挂载"));
        }
    }
}

fn check_process(name: &str, service: &YamlValue, violations: &mut Vec<String>) {
    let migration = name.starts_with("migrate-");
    let command = match name {
        "migrate-control" => format!("['--config-dir', '{CONFIG}', control, up]"),
        "migrate-tenant-data" => format!("['--config-dir', '{CONFIG}', tenant-data, up, '--all']"),
        _ => format!("['--config-dir', '{CONFIG}']"),
    };
    expect_yaml(
        service.get("command"),
        &command,
        &format!("{name}.command"),
        violations,
    );
    let restart = if migration { "'no'" } else { "unless-stopped" };
    expect_yaml(
        service.get("restart"),
        restart,
        &format!("{name}.restart"),
        violations,
    );
    let binary = if migration {
        "ryframe-migrate"
    } else if name == "worker" {
        "ryframe-worker"
    } else {
        "ryframe"
    };
    if name == "api" {
        if service.get("entrypoint").is_some() {
            violations.push("api 必须使用镜像登记的入口".into());
        }
    } else {
        expect_yaml(
            service.get("entrypoint"),
            &format!("['/usr/local/bin/{binary}']"),
            &format!("{name}.entrypoint"),
            violations,
        );
    }
    if migration {
        if service.get("healthcheck").is_some() {
            violations.push(format!("{name} 不得覆盖登记的迁移服务边界"));
        }
    } else {
        let health = format!(
            "test: [CMD, '/usr/local/bin/{binary}', '--config-dir', '{CONFIG}', '--healthcheck']\ninterval: 15s\ntimeout: 3s\nretries: 5\nstart_period: 20s"
        );
        expect_yaml(
            service.get("healthcheck"),
            &health,
            &format!("{name}.healthcheck"),
            violations,
        );
    }
}

fn check_dependencies(name: &str, service: &YamlValue, violations: &mut Vec<String>) {
    let dependencies = match name {
        "migrate-control" => None,
        "migrate-tenant-data" => {
            Some("migrate-control:\n  condition: service_completed_successfully")
        }
        "api" => Some(
            "migrate-tenant-data:\n  condition: service_completed_successfully\nworker:\n  condition: service_healthy",
        ),
        _ => Some("migrate-tenant-data:\n  condition: service_completed_successfully"),
    };
    if let Some(expected) = dependencies {
        expect_yaml(
            service.get("depends_on"),
            expected,
            &format!("{name}.depends_on"),
            violations,
        );
    } else if service.get("depends_on").is_some() {
        violations.push("migrate-control 不得增加前置服务".into());
    }
    let networks = if name == "api" {
        "[edge, backend]"
    } else {
        "[backend]"
    };
    expect_yaml(
        service.get("networks"),
        networks,
        &format!("{name}.networks"),
        violations,
    );
    if name == "api" {
        expect_yaml(
            service.get("ports"),
            "['127.0.0.1:8080:8080']",
            "api.ports",
            violations,
        );
    } else if service.get("ports").is_some() {
        violations.push(format!("{name} 不得发布端口"));
    }
}

fn expect_yaml(
    actual: Option<&YamlValue>,
    expected: &str,
    label: &str,
    violations: &mut Vec<String>,
) {
    let wrapped =
        if !expected.contains('\n') && (expected.starts_with('[') || !expected.contains(':')) {
            format!("value: {expected}")
        } else {
            format!(
                "value:\n{}",
                expected
                    .lines()
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>()
            )
        };
    let expected = yaml::parse(&wrapped).expect("固定部署校验 YAML 必须有效");
    let expected = expected.get("value").expect("固定部署校验字段必须存在");
    if !actual.is_some_and(|actual| equivalent(actual, expected)) {
        violations.push(format!("Compose {label} 与登记的部署边界不一致"));
    }
}

fn equivalent(left: &YamlValue, right: &YamlValue) -> bool {
    match (left, right) {
        (YamlValue::Mapping(left), YamlValue::Mapping(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .iter()
                        .any(|(other, expected)| key == other && equivalent(value, expected))
                })
        }
        (YamlValue::Sequence(left), YamlValue::Sequence(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| equivalent(left, right))
        }
        _ => left == right,
    }
}

fn mapping_keys(value: &YamlValue) -> Option<BTreeSet<&str>> {
    value
        .as_mapping()?
        .iter()
        .map(|(key, _)| match key {
            YamlKey::String(name) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn check_templates(root: &Path, violations: &mut Vec<String>) {
    let mut identifiers = BTreeSet::new();
    for (_, role, expected_id) in ROLES {
        let directory = root.join("deploy/templates").join(role);
        match AppConfig::load(&directory, Environment::Prod) {
            Ok(config) => {
                if role == "control"
                    && !config
                        .bootstrap
                        .admin_password
                        .as_deref()
                        .is_some_and(|password| !password.trim().is_empty())
                {
                    violations.push(
                        "部署 control 模板必须显式提供非空 bootstrap.admin_password 用于初始化"
                            .into(),
                    );
                }
                if !identifiers.insert(config.snowflake_worker_id) {
                    violations.push(format!("部署模板 {role} 的 snowflake_worker_id 重复"));
                }
                if config.snowflake_worker_id != expected_id {
                    violations.push(format!(
                        "部署模板 {role} 的 snowflake_worker_id 与角色不一致"
                    ));
                }
                if config.app.locales_dir != Path::new("/opt/ryframe/locales")
                    || config.app.host != "0.0.0.0"
                    || config.app.port != 8080
                    || config.jobs.health_host != "0.0.0.0"
                    || config.jobs.health_port != 9091
                {
                    violations.push(format!("部署模板 {role} 的路径或端口与容器边界不一致"));
                }
                if config.object_storage.backend != ryframe_config::StorageBackend::S3 {
                    violations.push(format!("部署模板 {role} 必须使用 S3 TLS 存储"));
                }
                let database = &config.database.primary;
                let redis = config.redis.as_ref();
                let paths = [
                    (database.tls_ca.as_deref(), "/opt/ryframe/tls/mysql-ca.pem"),
                    (
                        database.tls_client_cert.as_deref(),
                        "/opt/ryframe/tls/mysql-client-cert.pem",
                    ),
                    (
                        database.tls_client_key.as_deref(),
                        "/opt/ryframe/tls/mysql-client-key.pem",
                    ),
                    (
                        redis.and_then(|redis| redis.tls_ca.as_deref()),
                        "/opt/ryframe/tls/redis-ca.pem",
                    ),
                    (
                        redis.and_then(|redis| redis.tls_client_cert.as_deref()),
                        "/opt/ryframe/tls/redis-client-cert.pem",
                    ),
                    (
                        redis.and_then(|redis| redis.tls_client_key.as_deref()),
                        "/opt/ryframe/tls/redis-client-key.pem",
                    ),
                ];
                if paths
                    .iter()
                    .any(|(actual, expected)| *actual != Some(*expected))
                {
                    violations.push(format!("部署模板 {role} 的证书路径必须绑定只读 TLS 目录"));
                }
            }
            Err(error) => {
                violations.push(format!("部署公开模板 {role} 未通过生产配置校验：{error}"))
            }
        }
    }
}
