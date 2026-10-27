//! 按 Cargo 实际产物构建四个角色，不猜测 target 目录。
use super::provenance;
use crate::{Result, recovery::clone::native::json as documents};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(in crate::recovery) fn run(
    root: &Path,
    directory: &Path,
    command: &mut impl FnMut(&str, &[String]) -> Result<String>,
) -> Result<Value> {
    for name in ["binaries.json", "build-evidence.json"] {
        if directory.join(name).exists() {
            return Err("已有全栈构建收据；保留现场，禁止覆盖或自动重放".into());
        }
    }
    let source = provenance::current(root, directory)?;
    let mut binaries = serde_json::Map::new();
    for (feature, name) in [
        ("bin-reset", "ryframe-reset"),
        ("bin-migrate", "ryframe-migrate"),
        ("bin-api", "ryframe"),
        ("bin-worker", "ryframe-worker"),
    ] {
        let args = [
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            feature,
            "--bin",
            name,
            "--message-format=json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let output = command("cargo", &args)?;
        binaries.insert(name.into(), json!(artifact(root, name, &output)?));
    }
    let binaries = Value::Object(binaries);
    let evidence = provenance::build(root, directory, &binaries, &source)?;
    documents::write_new(&directory.join("binaries.json"), &binaries)?;
    documents::write_new(&directory.join("build-evidence.json"), &evidence)?;
    Ok(binaries)
}

fn artifact(root: &Path, name: &str, output: &str) -> Result<PathBuf> {
    let manifest = root.join("crates/ryframe/Cargo.toml");
    let mut paths = Vec::new();
    for line in output.lines() {
        let event = documents::parse_fragment(line, "Cargo 产物事件")?;
        if event["reason"] != "compiler-artifact"
            || event["target"]["name"] != name
            || event["executable"].is_null()
        {
            continue;
        }
        if event["target"]["kind"] != json!(["bin"])
            || fs::canonicalize(Path::new(documents::text(
                &event["manifest_path"],
                "Cargo manifest",
            )?))?
                != fs::canonicalize(&manifest)?
        {
            return Err("Cargo 返回角色相同但不属于当前 Workspace 的二进制".into());
        }
        let path = PathBuf::from(documents::text(&event["executable"], "Cargo executable")?);
        if !path.is_absolute() || !path.is_file() {
            return Err("Cargo 二进制路径必须是已存在的绝对文件".into());
        }
        paths.push(crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(path)?,
        ));
    }
    if paths.len() != 1 {
        return Err("Cargo 必须恰好返回一个当前 Workspace 的角色产物".into());
    }
    Ok(paths.remove(0))
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/full_stack/build_tests.rs"]
pub(in crate::recovery) mod tests;
