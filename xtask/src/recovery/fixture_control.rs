use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{FIXTURE_CONTROL_PROTOCOL_KIND, FIXTURE_CONTROL_USAGE, FixtureControlCommand},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

#[path = "fixture_control/environment.rs"]
mod environment;
#[path = "fixture_control/request.rs"]
mod request;
#[path = "fixture_control/review.rs"]
mod review;
#[path = "fixture_control/services.rs"]
mod services;
#[path = "fixture_control/source_pair.rs"]
mod source_pair;
#[path = "fixture_control/successor.rs"]
mod successor;

const PROTOCOL_KEY: &str = "RYFRAME_REFERENCE_FIXTURE_CONTROL_PROTOCOL";

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
    let invocation = private_invocation_at(command, root)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY],
    )
}

pub(crate) fn private_invocation_at(
    command: &FixtureControlCommand,
    root: &Path,
) -> Result<PrivateFixtureControlInvocation> {
    if command.is_help() {
        return Err("fixture 控制帮助不启动私有阶段程序".into());
    }
    let (script, fields) = match command {
        FixtureControlCommand::Environment(command) => (
            "scripts/reference_fixture_environment.py",
            environment::fields(command, root)?,
        ),
        FixtureControlCommand::Review(command) => (
            "scripts/reference_fixture_review.py",
            review::fields(command, root)?,
        ),
        FixtureControlCommand::Request(command) => (
            "scripts/reference_fixture_request.py",
            request::fields(command, root)?,
        ),
        FixtureControlCommand::SourcePair(command) => (
            "scripts/reference_fixture_source_pair.py",
            source_pair::fields(command, root)?,
        ),
        FixtureControlCommand::Successor(command) => (
            "scripts/reference_fixture_successor.py",
            successor::fields(command, root)?,
        ),
        FixtureControlCommand::Services(command) => (
            "scripts/reference_fixture_services.py",
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
