use std::path::{Component, Path};

use crate::Result;

pub(super) fn source_domain(relative: &str) -> Result<&'static str> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || relative.contains('\\')
        || windows_drive(relative)
        || relative != path.to_string_lossy()
    {
        return Err("源码清单路径无效".into());
    }
    let parts = relative.split('/').collect::<Vec<_>>();
    if parts.first() == Some(&"tests")
        || parts.starts_with(&["xtask", "tests"])
        || parts.len() >= 3 && parts[0] == "crates" && parts[2] == "tests"
    {
        return Ok("test_tools");
    }
    if parts.first() == Some(&"tools")
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("py" | "mjs" | "js" | "txt" | "toml" | "json")
        )
    {
        return Ok("test_tools");
    }
    if parts.first() == Some(&".github") {
        return Ok("test_tools");
    }
    if parts.first() == Some(&"docs")
        || matches!(
            relative,
            "README.md" | "README.en.md" | "CHANGELOG.md" | "CHANGELOG.en.md" | "LICENSE"
        )
    {
        return Ok("support");
    }
    Ok("product")
}

pub(super) fn backend_product_roles(relative: &str) -> Result<&'static [&'static str]> {
    let parts = relative.split('/').collect::<Vec<_>>();
    if matches!(
        parts.first(),
        Some(&"tools" | &"xtask" | &"tests" | &".github" | &"architecture" | &"docs" | &"deploy")
    ) || parts.len() >= 3 && parts[0] == "crates" && parts[2] == "tests"
    {
        return Ok(&[]);
    }
    if parts.starts_with(&["crates", "ryframe-api"]) {
        return Ok(&["api"]);
    }
    if parts.starts_with(&["crates", "ryframe-generator"]) {
        return Ok(&[]);
    }
    if parts.starts_with(&["crates", "ryframe", "src"]) {
        let nested = &parts[3..];
        if nested.is_empty() {
            return Ok(&["api", "worker"]);
        }
        if nested[0] == "main.rs" {
            return Ok(&["api"]);
        }
        if nested[0] == "reset" {
            return Ok(&[]);
        }
        if nested[0] == "bin" {
            if nested.len() >= 2 && matches!(nested[1], "ryframe_worker.rs" | "ryframe_worker") {
                return Ok(&["worker"]);
            }
            return Ok(&[]);
        }
        return Ok(&["api", "worker"]);
    }
    if parts.starts_with(&["crates", "ryframe"])
        || parts.first() == Some(&"crates")
        || matches!(
            parts.first(),
            Some(&".cargo" | &"vendor" | &"catalog" | &"config" | &"locales" | &"sql")
        )
    {
        return Ok(&["api", "worker"]);
    }
    if parts.first() == Some(&"openapi") {
        return Ok(&["api"]);
    }
    if matches!(
        relative,
        "Cargo.lock" | "Cargo.toml" | "rust-toolchain.toml"
    ) {
        return Ok(&["api", "worker"]);
    }
    if matches!(
        relative,
        "CHANGELOG.md"
            | "CHANGELOG.en.md"
            | "LICENSE"
            | "README.md"
            | "README.en.md"
            | ".dockerignore"
            | ".editorconfig"
            | ".gitattributes"
            | ".gitignore"
            | "deny.toml"
            | "rustfmt.toml"
    ) {
        return Ok(&[]);
    }
    Ok(&["api", "worker"])
}

pub(super) fn tool_source(relative: &str) -> bool {
    let parts = relative.split('/').collect::<Vec<_>>();
    matches!(
        parts.first(),
        Some(&"tools" | &"xtask" | &"tests" | &".github" | &"architecture")
    ) || relative == "scripts/requirements-ci.txt"
        || parts.len() >= 3 && parts[0] == "crates" && parts[2] == "tests"
        || matches!(relative, "deny.toml" | "rustfmt.toml")
}

fn windows_drive(relative: &str) -> bool {
    let bytes = relative.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}
