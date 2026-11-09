use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    Result,
    cli::{
        FIXTURE_CONTROL_USAGE, FixtureControlCommand, FixtureEnvironmentCommand,
        FixtureServiceOperation, FixtureServicesCommand, FixtureSuccessorCommand,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        digest,
        json::{self as native_json, exact, text},
    },
};

#[path = "fixture_control/artifact.rs"]
pub(in crate::recovery) mod artifact;

/// 全栈进程控制复用物理验收的文件配置、产物和源码权威核验。
pub(crate) fn verify_full_stack_runtime(root: &Path, directory: &Path) -> Result<Value> {
    artifact::runtime::verify(root, directory).map(|binding| binding.receipt)
}

pub(crate) fn full_stack_runtime_contract(
    root: &Path,
    directory: &Path,
    api: &Path,
    worker: &Path,
) -> Result<Value> {
    artifact::runtime::contract(root, directory, api, worker)
}
#[path = "fixture_control/dataset.rs"]
pub(crate) mod dataset;
#[path = "fixture_control/environment.rs"]
pub(crate) mod environment;
#[path = "fixture_control/request.rs"]
mod request;
#[path = "fixture_control/retention.rs"]
mod retention;
#[path = "fixture_control/review.rs"]
pub(crate) mod review;
#[path = "fixture_control/services.rs"]
pub(crate) mod services;
#[path = "fixture_control/source_pair.rs"]
pub(crate) mod source_pair;
#[path = "fixture_control/successor.rs"]
pub(crate) mod successor;

pub(super) fn run(command: &FixtureControlCommand, root: &Path) -> Result<()> {
    if command.operation() == "help" {
        println!("{FIXTURE_CONTROL_USAGE}");
        return Ok(());
    }
    match command {
        FixtureControlCommand::Environment(command) => match command {
            FixtureEnvironmentCommand::Review { review, output } => {
                print_json(&environment::review::run(root, review, &output.output)?)
            }
            FixtureEnvironmentCommand::Plan(inputs) => {
                print_json(&environment::run_plan(root, inputs)?)
            }
            FixtureEnvironmentCommand::Prepare { .. } => {
                print_json(&environment::prepare::run(root, command)?)
            }
            FixtureEnvironmentCommand::RotateSecrets { .. }
            | FixtureEnvironmentCommand::BootstrapSecrets { .. } => {
                print_json(&environment::secrets::run(root, command)?)
            }
            FixtureEnvironmentCommand::Help => unreachable!("已输出夹具帮助"),
        },
        FixtureControlCommand::Successor(command) => match command {
            FixtureSuccessorCommand::Relationship(options) => {
                print_json(&successor::publish_relationship(root, options)?)
            }
            FixtureSuccessorCommand::GenerationRequest(options) => print_json(
                &crate::recovery::source::evidence::generation_request::run(root, options)?,
            ),
            FixtureSuccessorCommand::ArmRequest(options) => {
                print_json(&successor::arm::run(root, options)?)
            }
            FixtureSuccessorCommand::Help => unreachable!("已输出夹具帮助"),
        },
        FixtureControlCommand::Services(services) => {
            let FixtureServicesCommand::Run(options) = services else {
                unreachable!("已输出夹具帮助");
            };
            services::fields(services, root)?;
            let operation = options.operation.as_str();
            let result = match options.operation {
                FixtureServiceOperation::Rustfs
                | FixtureServiceOperation::Redis
                | FixtureServiceOperation::Buckets => {
                    services::initial::run(root, &options.review, &options.environment, operation)?
                }
                FixtureServiceOperation::Status
                | FixtureServiceOperation::Close
                | FixtureServiceOperation::Reconcile
                | FixtureServiceOperation::Recover
                | FixtureServiceOperation::Restart => services::lifecycle::execute(
                    root,
                    operation,
                    &options.review,
                    &options.environment,
                    options.owner_binding.as_deref(),
                    command.writes(),
                )?,
            };
            print_json(&result)
        }
        FixtureControlCommand::Artifact(command) => artifact::run(command, root),
        FixtureControlCommand::Dataset(command) => dataset::run(command, root),
        FixtureControlCommand::Request(command) => print_json(&request::run(root, command)?),
        FixtureControlCommand::Retention(command) => retention::run(command, root),
        FixtureControlCommand::Review(command) => review::run(command, root),
        FixtureControlCommand::SourcePair(command) => source_pair::run(command, root),
    }
}

fn validate_path(value: &Path, root: &Path, kind: LocalTestPathKind, label: &str) -> Result<()> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(|error| format!("{label}无效：{error}").into())
}

fn validate_output_file(value: &Path, root: &Path, label: &str) -> Result<()> {
    validate_path(value, root, LocalTestPathKind::OutputFile, label)?;
    let parent = value.parent().ok_or_else(|| format!("{label}缺少父目录"))?;
    validate_path(parent, root, LocalTestPathKind::ExistingDirectory, label)
}

fn path_value(value: &Path, root: &Path, kind: LocalTestPathKind, label: &str) -> Result<Value> {
    validate_path(value, root, kind, label)?;
    Ok(Value::String(path_text(value, label)?.to_owned()))
}

fn output_value(value: &Path, root: &Path, label: &str) -> Result<Value> {
    validate_output_file(value, root, label)?;
    Ok(Value::String(path_text(value, label)?.to_owned()))
}

fn path_text<'a>(value: &'a Path, label: &str) -> Result<&'a str> {
    value
        .to_str()
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}

fn insert_path(
    fields: &mut Map<String, Value>,
    name: &str,
    value: &Path,
    root: &Path,
    kind: LocalTestPathKind,
) -> Result<()> {
    fields.insert(name.to_owned(), path_value(value, root, kind, name)?);
    Ok(())
}

fn insert_optional_path(
    fields: &mut Map<String, Value>,
    name: &str,
    value: Option<&PathBuf>,
    root: &Path,
    kind: LocalTestPathKind,
) -> Result<()> {
    if let Some(value) = value {
        insert_path(fields, name, value, root, kind)?;
    }
    Ok(())
}

pub(super) fn read_json(path: &Path, label: &str, limit: u64) -> Result<Value> {
    crate::recovery::runtime::dataset_authority::reads::file(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法读取{label} {}：{error}", path.display()))?;
    if !metadata.file_type().is_file() || digest::linked(&metadata) {
        return Err(format!("{label}必须是普通文件，不能经过链接或 junction").into());
    }
    if metadata.len() > limit {
        return Err(format!("{label}超过允许大小").into());
    }
    let bytes = fs::read(path).map_err(|error| format!("无法读取{label}：{error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label}超过允许大小").into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{label}必须是 UTF-8 JSON"))?;
    let value = native_json::parse_fragment(text, label)?;
    if !value.is_object() {
        return Err(format!("{label}必须是 JSON 对象").into());
    }
    Ok(value)
}

pub(super) fn file_binding(path: &Path, label: &str) -> Result<Value> {
    let digest = digest::file(path)?;
    Ok(json!({
        "path": path_text_canonical(path, label)?,
        "bytes": digest.bytes,
        "sha256": digest.sha256,
    }))
}

pub(super) fn path_text_canonical(path: &Path, label: &str) -> Result<String> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| format!("无法解析{label} {}：{error}", path.display()))?;
    let path = portable_path(path);
    let resolved = portable_path(&resolved);
    if resolved != path {
        return Err(format!("{label}必须使用规范绝对路径").into());
    }
    digest::path_text(&path, label)
}

pub(super) fn write_new(path: &Path, value: &Value) -> Result<()> {
    native_json::write_new(path, value)
}

pub(super) fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

pub(super) fn exact_object<'a>(
    value: &'a Value,
    fields: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>> {
    exact(value, fields, label)
}

pub(super) fn text_field<'a>(value: &'a Value, field: &str, label: &str) -> Result<&'a str> {
    text(
        value
            .get(field)
            .ok_or_else(|| format!("{label}缺少 {field}"))?,
        &format!("{label} {field}"),
    )
}

pub(super) fn valid_sha256(value: &str, label: &str) -> Result<()> {
    if Regex::new(r"^[a-f0-9]{64}$")?.is_match(value) {
        Ok(())
    } else {
        Err(format!("{label}必须是 64 位小写十六进制摘要").into())
    }
}

pub(super) fn git_root(path: &Path, label: &str) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("无法核验{label} Git 根目录：{error}"))?;
    if !output.status.success() {
        return Err(format!("{label}不是 Git 工作树").into());
    }
    let text =
        String::from_utf8(output.stdout).map_err(|_| format!("{label} Git 根目录不是 UTF-8"))?;
    let root = PathBuf::from(text.trim());
    fs::canonicalize(&root)
        .map(|path| portable_path(&path))
        .map_err(|error| format!("无法解析{label} Git 根目录：{error}").into())
}

pub(super) fn portable_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else {
        value
            .strip_prefix(r"\\?\")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf())
    }
}
