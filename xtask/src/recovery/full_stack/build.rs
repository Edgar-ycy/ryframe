//! 按 Cargo 实际产物构建四个角色，不猜测 target 目录。
use super::provenance;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{digest, json as documents},
};
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
    let jobs = crate::recovery::clone::native::bridge::build_jobs(root)?;
    let source = provenance::current(root, directory)?;
    let artifacts = directory.join("artifacts");
    validate_absolute_path(&artifacts, LocalTestPathKind::NewDirectory)?;
    fs::create_dir(&artifacts)?;
    let mut binaries = serde_json::Map::new();
    for (feature, name) in [
        ("bin-reset", "ryframe-reset"),
        ("bin-migrate", "ryframe-migrate"),
        ("bin-api", "ryframe"),
        ("bin-worker", "ryframe-worker"),
    ] {
        let jobs_text = jobs.to_string();
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
            "--jobs",
            &jobs_text,
            "--message-format=json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let output = command("cargo", &args)?;
        let cargo = artifact(root, name, &output)?;
        binaries.insert(name.into(), json!(freeze(&cargo, &artifacts)?));
    }
    let binaries = Value::Object(binaries);
    if crate::recovery::clone::native::bridge::build_jobs(root)? != jobs {
        return Err("构建期间 verification.build_jobs 变化；保留产物和原收据，不自动重放".into());
    }
    let evidence = provenance::build(root, directory, &binaries, &source, jobs)?;
    if crate::recovery::clone::native::bridge::build_jobs(root)? != jobs {
        return Err("发布构建收据前 verification.build_jobs 变化；保留产物，不自动重放".into());
    }
    documents::write_new(&directory.join("binaries.json"), &binaries)?;
    documents::write_new(&directory.join("build-evidence.json"), &evidence)?;
    Ok(binaries)
}

/// 每个 Cargo 角色返回后立即独立复制；共享缓存后续覆盖不得改变本代产物。
pub(in crate::recovery) fn freeze(source: &Path, directory: &Path) -> Result<PathBuf> {
    validate_absolute_path(source, LocalTestPathKind::ExistingFile)?;
    let before = digest::file(source)?;
    let saved = directory.join(source.file_name().ok_or("Cargo 产物没有文件名")?);
    let mut input = fs::File::open(source)?;
    #[cfg(unix)]
    let permissions = input.metadata()?.permissions();
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&saved)?;
    std::io::copy(&mut input, &mut output)?;
    #[cfg(unix)]
    fs::set_permissions(&saved, permissions)?;
    output.sync_all()?;
    if digest::file(source)? != before || digest::file(&saved)? != before {
        return Err("冻结构建产物期间源或副本发生变化；保留现场，禁止覆盖或重放".into());
    }
    Ok(crate::recovery::fixture_control::portable_path(
        &fs::canonicalize(saved)?,
    ))
}

pub(in crate::recovery) fn artifact(root: &Path, name: &str, output: &str) -> Result<PathBuf> {
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
