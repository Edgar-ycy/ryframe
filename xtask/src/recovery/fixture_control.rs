use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    Result,
    cli::{FIXTURE_CONTROL_PROTOCOL_KIND, FIXTURE_CONTROL_USAGE, FixtureControlCommand},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_stdin,
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
mod dataset;
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
mod successor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateFixtureControlInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &FixtureControlCommand, root: &Path) -> Result<()> {
    if command.is_help() {
        println!("{FIXTURE_CONTROL_USAGE}");
        return Ok(());
    }
    match command {
        FixtureControlCommand::Successor(
            crate::cli::FixtureSuccessorCommand::GenerationRequest(options),
        ) => {
            return print_json(&crate::recovery::source::evidence::generation_request::run(
                root, options,
            )?);
        }
        FixtureControlCommand::SourcePair(command) => return source_pair::run(command, root),
        FixtureControlCommand::Review(command) => return review::run(command, root),
        FixtureControlCommand::Artifact(command) => return artifact::run(command, root),
        FixtureControlCommand::Services(crate::cli::FixtureServicesCommand::Run(options))
            if matches!(
                options.operation,
                crate::cli::FixtureServiceOperation::Status
                    | crate::cli::FixtureServiceOperation::Close
                    | crate::cli::FixtureServiceOperation::Recover
                    | crate::cli::FixtureServiceOperation::Reconcile
                    | crate::cli::FixtureServiceOperation::Restart
            ) =>
        {
            services::fields(
                &crate::cli::FixtureServicesCommand::Run(options.clone()),
                root,
            )?;
            let value = services::lifecycle::execute(
                root,
                options.operation.as_str(),
                &options.review,
                &options.environment,
                options.owner_binding.as_deref(),
                !matches!(
                    options.operation,
                    crate::cli::FixtureServiceOperation::Status
                ),
            )?;
            return print_json(&value);
        }
        FixtureControlCommand::Retention(command) => return retention::run(command, root),
        _ => {}
    }
    let invocation = private_invocation_at(command, root)?;
    let mut input = tempfile::tempfile()?;
    input.write_all(invocation.protocol.as_bytes())?;
    input.seek(SeekFrom::Start(0))?;
    run_with_stdin(
        root,
        "python",
        &["-B", invocation.script],
        Stdio::from(input),
    )
}

pub(crate) fn private_invocation_at(
    command: &FixtureControlCommand,
    root: &Path,
) -> Result<PrivateFixtureControlInvocation> {
    if command.is_help() {
        return Err("fixture 控制帮助不启动私有阶段程序".into());
    }
    if matches!(
        command,
        FixtureControlCommand::SourcePair(_)
            | FixtureControlCommand::Review(_)
            | FixtureControlCommand::Artifact(_)
            | FixtureControlCommand::Retention(_)
    ) {
        return Err("该夹具操作由 Rust 原生实现".into());
    }
    if matches!(command, FixtureControlCommand::Services(crate::cli::FixtureServicesCommand::Run(options))
        if matches!(options.operation, crate::cli::FixtureServiceOperation::Status
            | crate::cli::FixtureServiceOperation::Close
            | crate::cli::FixtureServiceOperation::Recover | crate::cli::FixtureServiceOperation::Reconcile
            | crate::cli::FixtureServiceOperation::Restart))
    {
        return Err("该服务生命周期由 Rust 原生执行，不生成 Python 编排请求".into());
    }
    let (script, fields) = match command {
        FixtureControlCommand::Artifact(_) => unreachable!("原生导出物理验收已提前返回"),
        FixtureControlCommand::Dataset(command) => (
            "tools/python/reference_fixture_dataset.py",
            dataset::fields(command, root)?,
        ),
        FixtureControlCommand::Environment(command) => (
            "tools/python/reference_fixture_environment.py",
            environment::fields(command, root)?,
        ),
        FixtureControlCommand::Retention(_) => unreachable!("原生保留期验收已提前返回"),
        FixtureControlCommand::Review(_) => unreachable!("原生审阅续签已提前返回"),
        FixtureControlCommand::Request(command) => (
            "tools/python/reference_fixture_request.py",
            request::fields(command, root)?,
        ),
        FixtureControlCommand::SourcePair(_) => unreachable!("原生来源组合已提前返回"),
        FixtureControlCommand::Successor(command) => (
            "tools/python/reference_fixture_successor.py",
            successor::fields(command, root)?,
        ),
        FixtureControlCommand::Services(command) => (
            "tools/python/reference_fixture_services.py",
            services::fields(command, root)?,
        ),
    };
    let mut protocol = Map::from_iter([
        (
            "backend_dir".to_owned(),
            Value::String(path_text(root, "后端目录")?.to_owned()),
        ),
        (
            "domain".to_owned(),
            Value::String(command.domain().to_owned()),
        ),
        ("format_version".to_owned(), Value::from(1)),
        (
            "kind".to_owned(),
            Value::String(FIXTURE_CONTROL_PROTOCOL_KIND.to_owned()),
        ),
        (
            "operation".to_owned(),
            Value::String(command.operation().to_owned()),
        ),
        ("write".to_owned(), Value::Bool(command.writes())),
    ]);
    for (name, value) in fields {
        if protocol.insert(name.clone(), value).is_some() {
            return Err(format!("fixture 控制私有协议字段重复：{name}").into());
        }
    }
    if matches!(
        command,
        FixtureControlCommand::Services(_)
            | FixtureControlCommand::Dataset(_)
            | FixtureControlCommand::Request(_)
            | FixtureControlCommand::Environment(_)
            | FixtureControlCommand::Successor(_)
    ) {
        let binding = crate::check::python_suite::native_tool_binding()?;
        protocol.insert(
            "native_tool".to_owned(),
            Value::String(serde_json::to_string(&binding)?),
        );
    }
    Ok(PrivateFixtureControlInvocation {
        script,
        protocol: serialize_protocol(Value::Object(protocol))?,
    })
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

fn serialize_protocol(protocol: Value) -> Result<String> {
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.contains(['\n', '\r', '\0']) {
        return Err("fixture 控制私有协议不能包含换行符或 NUL".into());
    }
    Ok(serialized)
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
