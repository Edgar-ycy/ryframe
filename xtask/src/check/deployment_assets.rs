//! 生产部署资产与最终镜像的纯 Rust 只读校验。

use std::{path::Path, sync::OnceLock};

use crate::{Result, process::PreservedFailure};

#[path = "deployment_assets/compose.rs"]
mod compose;
#[path = "deployment_assets/image.rs"]
mod image;
#[path = "deployment_assets/rules.rs"]
pub(crate) mod rules;
#[path = "deployment_assets/source.rs"]
mod source;
#[path = "deployment_assets/tar.rs"]
mod tar;

pub(crate) const EXPECTED_BINARIES: &[&str] = &["ryframe", "ryframe-migrate", "ryframe-worker"];

fn regex(pattern: &'static str) -> &'static regex::Regex {
    static REGEXES: OnceLock<Vec<(&'static str, regex::Regex)>> = OnceLock::new();
    REGEXES
        .get_or_init(|| {
            [
                r"^ARG\s+RUST_IMAGE=([^\s]+)\s*$",
                r"^ARG\s+RUNTIME_IMAGE=([^\s]+)\s*$",
                r"\bgenerator_handler\b",
                r#"/api/v1/tools/gen(?:/|["'])"#,
                r"\bryframe_generator\b",
                r"(?m)^[ \t]*- alert:[ \t]*([^\s]+)[ \t]*$",
                r#"(?m)^[ \t]*runbook_url:[ \t]*["']([^"']+)["'][ \t]*$"#,
                r"(?m)^[ \t]*runbook:[ \t]*",
                r"^\s*-?\s*uses:\s*([^\s#]+)",
                r"^1\.\d+\.\d+$",
                r"^[^\s@]+@[0-9a-f]{40}$",
                r"^docker://[^\s@]+@sha256:[0-9a-f]{64}$",
                r"^[^\s@]+@sha256:[0-9a-f]{64}$",
                r"^[A-Za-z0-9._/-]+:[A-Za-z0-9][A-Za-z0-9._-]*@sha256:[0-9a-f]{64}$",
                r"--bin\s+([a-zA-Z0-9_-]+)",
                r"^COPY\s+--from=builder\s+\S+\s+/usr/local/bin/([a-zA-Z0-9_-]+)\s*$",
                r"\\\r?\n\s*",
                r"^[ \t]*(`{3,}|~{3,})",
                r"^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$",
                r"[^\w\- ]",
                r"[ \t]+",
                r"^[A-Za-z0-9._/@:+-]+$",
                r"^[0-9a-f]{12,64}$",
                r"^[0-9a-f]{40}$",
            ]
            .into_iter()
            .map(|pattern| {
                (
                    pattern,
                    regex::Regex::new(pattern).expect("固定部署资产正则必须有效"),
                )
            })
            .collect()
        })
        .iter()
        .find_map(|(source, compiled)| (*source == pattern).then_some(compiled))
        .expect("部署资产正则必须先登记")
}

pub(crate) fn source_violations(root: &Path) -> Result<Vec<String>> {
    source::violations(root)
}

pub(crate) fn archive_violations(path: &Path) -> Result<Vec<String>> {
    tar::violations(path)
}

pub(crate) fn run_source(root: &Path) -> Result<()> {
    finish(source_violations(root)?, "静态")
}

pub(crate) fn run_image(root: &Path, image: &str, expected_commit: Option<&str>) -> Result<()> {
    finish(image::violations(root, image, expected_commit)?, "生产镜像")
}

pub(crate) fn finish(violations: Vec<String>, phase: &str) -> Result<()> {
    if violations.is_empty() {
        println!("部署资产检查通过（{phase}）。");
        return Ok(());
    }
    println!("部署资产检查失败:");
    for violation in &violations {
        println!("  - {violation}");
    }
    Err(PreservedFailure::new(
        format!("部署资产检查发现 {} 个问题", violations.len()),
        Some(1),
    )
    .into())
}

fn execution_error(context: &str, error: impl std::fmt::Display) -> PreservedFailure {
    PreservedFailure::new(format!("部署资产检查执行失败: {context}: {error}"), Some(1))
}
