use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::{
    regex,
    rules::{toolchain_violations, validate_runbook_url},
};
use crate::Result;

const DOCKERFILE: &str = "deploy/Dockerfile";
const ALERT_RULES: &str = "deploy/prometheus/ryframe-alerts.yml";
const EXPECTED_BASE_IMAGES: [(&str, &str); 2] = [
    (
        "RUST_IMAGE",
        "rust:1.100.0-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea",
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
    super::compose::check(root, &mut violations)?;
    check_alert_runbooks(root, &mut violations)?;
    check_pinned_actions(root, &mut violations)?;
    Ok(violations)
}

fn check_dockerfile(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    let source = read(root, DOCKERFILE)?;
    check_rust_toolchain(root, &source, violations)?;
    let normalized = regex(r"\\\r?\n\s*").replace_all(&source, " ");
    check_source_commit_binding(&source, &normalized, violations);
    check_runtime_environment(&normalized, violations);
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

fn check_source_commit_binding(source: &str, normalized: &str, violations: &mut Vec<String>) {
    let argument_count = source
        .lines()
        .filter(|line| line.trim() == "ARG SOURCE_COMMIT")
        .count();
    if argument_count != 3 {
        violations.push(
            "生产 Dockerfile 必须在全局、构建阶段和运行阶段各声明一次 ARG SOURCE_COMMIT".to_owned(),
        );
    }
    for required in [
        r#"grep -Eq '^[0-9a-f]{40}$'"#,
        r#"> /workspace/.local-tests/build/source-commit"#,
        r#"rm /workspace/.local-tests/build/source-commit"#,
        r#"LABEL org.opencontainers.image.revision="${SOURCE_COMMIT}""#,
    ] {
        if !normalized.contains(required) {
            violations.push(format!(
                "生产 Dockerfile 的源码提交绑定缺少必要步骤: {required}"
            ));
        }
    }
    for forbidden in ["RYFRAME_BUILD_COMMIT", "ENV SOURCE_COMMIT"] {
        if source.contains(forbidden) {
            violations.push(format!("生产 Dockerfile 不得继续使用 {forbidden}"));
        }
    }
}

fn check_runtime_environment(normalized: &str, violations: &mut Vec<String>) {
    let lines = normalized.lines().collect::<Vec<_>>();
    let instruction = |line: &str, name: &str| {
        line.split_ascii_whitespace()
            .next()
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
    };
    let Some(stage) = lines.iter().rposition(|line| {
        instruction(line, "FROM") && line.split_ascii_whitespace().nth(1).is_some()
    }) else {
        violations.push("生产 Dockerfile 缺少 FROM，无法定位最终运行阶段".to_owned());
        return;
    };
    let environment = lines[stage + 1..]
        .iter()
        .filter(|line| instruction(line, "ENV"))
        .map(|line| line.split_ascii_whitespace().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    if environment.len() != 1 || environment[0].len() != 2 || environment[0][1] != "APP_ENV=prod" {
        violations.push(
            "生产 Dockerfile 运行阶段必须且只能声明一条 ENV APP_ENV=prod，不得缺失、重复、插值或增加其他环境配置"
                .to_owned(),
        );
    }
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

#[cfg(test)]
mod tests {
    use super::{check_runtime_environment, check_source_commit_binding, regex};

    fn runtime_violations(source: &str) -> Vec<String> {
        let normalized = regex(r"\\\r?\n\s*").replace_all(source, " ");
        let mut violations = Vec::new();
        check_runtime_environment(&normalized, &mut violations);
        violations
    }

    #[test]
    fn runtime_accepts_prod_and_preserves_builder_environment() {
        for source in [
            "FROM runtime\nENV APP_ENV=prod\n",
            "FROM builder AS builder\nENV CARGO_HOME=/cargo\nENV PATH=/tools\nFROM runtime\nENV APP_ENV=prod\n",
            "FROM builder\nENV CARGO_NET_OFFLINE=true \\\n                CARGO_INCREMENTAL=0\nFROM runtime\nENV \\\n                APP_ENV=prod\n",
            "from builder\r\nENV CARGO_HOME=/cargo\r\nfrom runtime\r\nenv APP_ENV=prod\r\n",
        ] {
            assert!(runtime_violations(source).is_empty(), "合法运行阶段应通过");
        }
    }

    #[test]
    fn runtime_rejects_missing_extra_duplicate_or_invalid_environment() {
        for declaration in [
            "",
            "ENV",
            "ENV APP_CONFIG_DIR=/opt/ryframe/config",
            "ENV APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads",
            "ENV APP_ENV=prod\nENV APP_CONFIG_DIR=/opt/ryframe/config",
            "ENV APP_ENV=prod\nENV APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads",
            "ENV APP_ENV=prod\nENV TZ=UTC",
            "ENV APP_ENV=prod\nENV APP_ENV=prod",
            "ENV APP_ENV=prod APP_ENV=prod",
            "ENV APP_ENV=dev",
            "ENV APP_ENV=PROD",
            "ENV app_env=prod",
            "ENV APP_ENV prod",
            "ENV APP_ENV=$APP_ENV",
            "ENV APP_ENV=${APP_ENV:-prod}",
            "ENV APP_ENV=prod \\\n                APP_CONFIG_DIR=/opt/ryframe/config \\\n                APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads",
            "ENV APP_ENV=prod \\\r\n                TZ=UTC",
        ] {
            let source = format!(
                "FROM builder\nENV APP_ENV=prod\nFROM intermediate\nENV APP_ENV=prod\nFROM runtime\n{declaration}\n"
            );
            assert!(
                !runtime_violations(&source).is_empty(),
                "非法运行环境声明必须拒绝：{declaration}"
            );
        }
    }

    #[test]
    fn missing_runtime_stage_has_an_explicit_diagnostic() {
        for source in [
            "",
            "ENV APP_ENV=prod\n",
            "# FROM runtime\nENV APP_ENV=prod\n",
        ] {
            let violations = runtime_violations(source);
            assert_eq!(violations.len(), 1);
            assert!(violations[0].contains("缺少 FROM"));
            assert!(violations[0].contains("运行阶段"));
        }
    }

    #[test]
    fn source_commit_binding_rejects_legacy_or_incomplete_delivery() {
        let complete = concat!(
            "ARG SOURCE_COMMIT\n",
            "FROM builder AS builder\n",
            "ARG SOURCE_COMMIT\n",
            "RUN printf '%s\\n' \"${SOURCE_COMMIT}\" | grep -Eq '^[0-9a-f]{40}$' \\\n",
            "    && printf '%s\\n' \"${SOURCE_COMMIT}\" > /workspace/.local-tests/build/source-commit \\\n",
            "    && cargo build --locked \\\n",
            "    && rm /workspace/.local-tests/build/source-commit\n",
            "FROM runtime\n",
            "ARG SOURCE_COMMIT\n",
            "LABEL org.opencontainers.image.revision=\"${SOURCE_COMMIT}\"\n",
        );
        let normalized = regex(r"\\\r?\n\s*").replace_all(complete, " ");
        let mut violations = Vec::new();
        check_source_commit_binding(complete, &normalized, &mut violations);
        assert!(violations.is_empty(), "{violations:#?}");

        for invalid in [
            complete.replacen("ARG SOURCE_COMMIT\n", "", 1),
            complete.replace("^[0-9a-f]{40}$", "^[0-9a-f]+$"),
            complete.replace("rm /workspace/.local-tests/build/source-commit", "true"),
            complete.replace(
                "LABEL org.opencontainers.image.revision=\"${SOURCE_COMMIT}\"",
                "LABEL org.opencontainers.image.revision=development",
            ),
            format!("{complete}ENV SOURCE_COMMIT=unknown\n"),
            format!("{complete}ARG RYFRAME_BUILD_COMMIT\n"),
        ] {
            let normalized = regex(r"\\\r?\n\s*").replace_all(&invalid, " ");
            let mut violations = Vec::new();
            check_source_commit_binding(&invalid, &normalized, &mut violations);
            assert!(!violations.is_empty(), "非法源码提交绑定必须被拒绝");
        }
    }
}
