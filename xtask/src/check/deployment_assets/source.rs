use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use super::{
    regex,
    rules::{toolchain_violations, validate_runbook_url},
};
use crate::Result;

const DOCKERFILE: &str = "deploy/Dockerfile";
const COMPOSE_FILE: &str = "deploy/compose.prod.yml";
const ALERT_RULES: &str = "deploy/prometheus/ryframe-alerts.yml";
const FIXTURE_ENV: &str = "tools/fixtures/deployment/deploy.env";
const EXPECTED_BASE_IMAGES: [(&str, &str); 2] = [
    (
        "RUST_IMAGE",
        "rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e",
    ),
    (
        "RUNTIME_IMAGE",
        "gcr.io/distroless/base-nossl-debian13:nonroot@sha256:8c563c1fb5e120606f0d85733049775faed6192e2bd2223ef283a5393eec22b9",
    ),
];

pub(super) fn violations(root: &Path) -> Result<Vec<String>> {
    let mut violations = Vec::new();
    check_dockerfile(root, &mut violations)?;
    check_online_generator(root, &mut violations)?;
    check_compose_fixture(root, &mut violations)?;
    check_alert_runbooks(root, &mut violations)?;
    check_pinned_actions(root, &mut violations)?;
    Ok(violations)
}

fn check_dockerfile(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let source = read(root, DOCKERFILE)?;
    check_rust_toolchain(root, &source, violations)?;
    let normalized = regex(r"\\\r?\n\s*").replace_all(&source, " ");
    for (argument, expected) in EXPECTED_BASE_IMAGES {
        let pattern = format!(r"(?m)^ARG\s+{argument}=([^\s]+)\s*$");
        let actual = regex::Regex::new(&pattern)?
            .captures_iter(&source)
            .filter_map(|capture| capture.get(1).map(|value| value.as_str()))
            .collect::<Vec<_>>();
        if actual != [expected] {
            violations.push(format!(
                "生产 Dockerfile 的 {argument} 必须精确固定为 {expected}"
            ));
        } else if !regex(r"^[A-Za-z0-9._/-]+:[A-Za-z0-9][A-Za-z0-9._-]*@sha256:[0-9a-f]{64}$")
            .is_match(actual[0])
        {
            violations.push(format!(
                "生产 Dockerfile 的 {argument} 必须同时保留清晰标签和完整 sha256 摘要"
            ));
        }
    }
    let built = captures(r"--bin\s+([a-zA-Z0-9_-]+)", &normalized);
    let copied = captures(
        r"(?m)^COPY\s+--from=builder\s+\S+\s+/usr/local/bin/([a-zA-Z0-9_-]+)\s*$",
        &source,
    );
    let expected = super::EXPECTED_BINARIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if built != expected {
        violations.push(format!(
            "生产构建二进制必须精确为 {:?}，当前为 {:?}",
            expected, built
        ));
    }
    if copied != expected {
        violations.push(format!(
            "运行镜像二进制必须精确为 {:?}，当前为 {:?}",
            expected, copied
        ));
    }
    if !normalized.contains("--no-default-features") {
        violations.push("生产构建必须禁用默认 feature".to_owned());
    }
    for forbidden in ["ryframe-generator", "ryframe-reset", "ryframe-db-reset"] {
        if source.contains(forbidden) {
            violations.push(format!("生产 Dockerfile 不得包含 {forbidden}"));
        }
    }
    for forbidden in ["apt-get", "curl", "useradd"] {
        if source.contains(forbidden) {
            violations.push(format!("最小运行镜像不得包含 {forbidden}"));
        }
    }
    if !source.contains(r#"CMD ["/usr/local/bin/ryframe", "--healthcheck"]"#) {
        violations.push("生产镜像必须使用 API 内置健康检查".to_owned());
    }
    Ok(())
}

fn check_rust_toolchain(root: &Path, source: &str, violations: &mut Vec<String>) -> Result<()> {
    let toolchain = read(root, "rust-toolchain.toml")?;
    let manifest = read(root, "Cargo.toml")?;
    let channel = toml::from_str::<toml::Value>(&toolchain)?
        .get("toolchain")
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("channel"))
        .and_then(toml::Value::as_str)
        .ok_or("rust-toolchain.toml 缺少 toolchain.channel")?
        .to_owned();
    let minimum = toml::from_str::<toml::Value>(&manifest)?
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("rust-version"))
        .and_then(toml::Value::as_str)
        .ok_or("Cargo.toml 缺少 workspace.package.rust-version")?
        .to_owned();
    violations.extend(toolchain_violations(&channel, &minimum, source));
    Ok(())
}

fn check_online_generator(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let source_root = root.join("crates/ryframe-api/src");
    for path in rust_files(&source_root)? {
        let source = fs::read_to_string(&path)?;
        for (label, pattern) in [
            ("在线生成器 handler", r"\bgenerator_handler\b"),
            ("在线生成器 API", r#"/api/v1/tools/gen(?:/|["'])"#),
            ("在线生成器 crate", r"\bryframe_generator\b"),
        ] {
            if regex(pattern).is_match(&source) {
                violations.push(format!("{}: 生产 API 仍包含{label}", relative(root, &path)));
            }
        }
    }
    Ok(())
}

fn check_compose_fixture(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let compose = read(root, COMPOSE_FILE)?;
    if compose.contains("curl") || compose.contains("CMD-SHELL") {
        violations.push("生产 Compose 健康检查不得依赖 shell 或 curl".to_owned());
    }
    for command in [
        r#"["CMD", "/usr/local/bin/ryframe", "--healthcheck"]"#,
        r#"["CMD", "/usr/local/bin/ryframe-worker", "--healthcheck"]"#,
    ] {
        if !compose.contains(command) {
            violations.push(format!("生产 Compose 缺少内置健康检查: {command}"));
        }
    }
    let values = match parse_env(&root.join(FIXTURE_ENV)) {
        Ok(values) => values,
        Err(error) => {
            violations.push(error);
            return Ok(());
        }
    };
    let required = captures(r"\$\{([A-Z][A-Z0-9_]*):\?[^}]+}", &compose);
    let missing = required
        .difference(&values.keys().cloned().collect())
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        violations.push(format!("Compose 校验环境缺少变量: {}", missing.join(", ")));
    }
    if !regex(r"^[^\s@]+@sha256:[0-9a-f]{64}$")
        .is_match(values.get("RYFRAME_IMAGE").map_or("", String::as_str))
    {
        violations.push("Compose 校验镜像必须使用完整 sha256 摘要".to_owned());
    }
    let forbidden = values
        .keys()
        .filter(|name| regex(r"(?:PASSWORD|SECRET|TOKEN|PRIVATE_KEY)$").is_match(name))
        .cloned()
        .collect::<Vec<_>>();
    if !forbidden.is_empty() {
        violations.push(format!(
            "Compose fixture 只能保存无秘密配置或文件路径: {}",
            forbidden.join(", ")
        ));
    }
    if !regex(r"(?m)^\s*APP_ENV:\s*prod\s*$").is_match(&compose) {
        violations.push("生产 Compose 必须固定 APP_ENV=prod".to_owned());
    }
    Ok(())
}

fn check_alert_runbooks(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let source = read(root, ALERT_RULES)?;
    let alerts = regex(r"(?m)^[ \t]*- alert:[ \t]*([^\s]+)[ \t]*$")
        .captures_iter(&source)
        .filter_map(|capture| Some((capture.get(0)?.start(), capture.get(1)?.as_str().to_owned())))
        .collect::<Vec<_>>();
    if alerts.is_empty() {
        violations.push("Prometheus 规则中没有告警".to_owned());
        return Ok(());
    }
    for (index, (start, name)) in alerts.iter().enumerate() {
        let end = alerts.get(index + 1).map_or(source.len(), |alert| alert.0);
        let block = &source[*start..end];
        let runbook = regex(r#"(?m)^[ \t]*runbook_url:[ \t]*["']([^"']+)["'][ \t]*$"#)
            .captures(block)
            .and_then(|capture| capture.get(1))
            .map(|value| value.as_str());
        if let Some(url) = runbook {
            if let Some(problem) = validate_runbook_url(root, url)? {
                violations.push(format!("告警 {name} 的 runbook_url {problem}"));
            }
        } else {
            violations.push(format!("告警 {name} 缺少 runbook_url"));
        }
    }
    if regex(r"(?m)^[ \t]*runbook:[ \t]*").is_match(&source) {
        violations.push("Prometheus 规则不得继续使用相对 runbook annotation".to_owned());
    }
    Ok(())
}

fn check_pinned_actions(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let workflow_root = root.join(".github/workflows");
    let mut workflows = fs::read_dir(workflow_root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    workflows.retain(|path| {
        path.extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| matches!(value, "yml" | "yaml"))
    });
    workflows.sort();
    for path in workflows {
        let source = fs::read_to_string(&path)?;
        for (index, line) in source.lines().enumerate() {
            let Some(capture) = regex(r"^\s*-?\s*uses:\s*([^\s#]+)").captures(line) else {
                continue;
            };
            let reference = capture
                .get(1)
                .map(|value| value.as_str().trim_matches(['"', '\'']))
                .unwrap_or_default();
            if reference.starts_with("./") {
                continue;
            }
            if !regex(r"^[^\s@]+@[0-9a-f]{40}$").is_match(reference)
                && !regex(r"^docker://[^\s@]+@sha256:[0-9a-f]{64}$").is_match(reference)
            {
                violations.push(format!(
                    "{}:{}: action 必须固定到 commit 或 sha256 digest",
                    relative(root, &path),
                    index + 1
                ));
            }
        }
    }
    Ok(())
}

fn parse_env(path: &Path) -> std::result::Result<BTreeMap<String, String>, String> {
    let source =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut values = BTreeMap::new();
    for (number, raw_line) in source.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "{}:{}: 环境变量格式无效",
                path.display(),
                number + 1
            ));
        };
        if !regex(r"^[A-Z][A-Z0-9_]*$").is_match(name) {
            return Err(format!(
                "{}:{}: 环境变量格式无效",
                path.display(),
                number + 1
            ));
        }
        if values.insert(name.to_owned(), value.to_owned()).is_some() {
            return Err(format!(
                "{}:{}: 环境变量重复: {name}",
                path.display(),
                number + 1
            ));
        }
    }
    Ok(values)
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn captures(pattern: &str, source: &str) -> BTreeSet<String> {
    regex::Regex::new(pattern)
        .expect("调用方固定正则")
        .captures_iter(source)
        .filter_map(|capture| capture.get(1).map(|value| value.as_str().to_owned()))
        .collect()
}

fn read(root: &Path, relative: &str) -> Result<String> {
    Ok(fs::read_to_string(root.join(relative))?)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
