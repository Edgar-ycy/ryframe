use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    cli::{CliError, FixtureControlCommand, FixtureSide},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

#[path = "fixture_control/artifact.rs"]
mod artifact;
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

pub(super) fn parse_fixture_control(
    domain: &str,
    args: &[String],
) -> Result<FixtureControlCommand, CliError> {
    match domain {
        "artifact" => artifact::parse(args).map(FixtureControlCommand::Artifact),
        "environment" => environment::parse(args).map(FixtureControlCommand::Environment),
        "review" => review::parse(args).map(FixtureControlCommand::Review),
        "request" => request::parse(args).map(FixtureControlCommand::Request),
        "source-pair" => source_pair::parse(args).map(FixtureControlCommand::SourcePair),
        "successor" => successor::parse(args).map(FixtureControlCommand::Successor),
        "services" => services::parse(args).map(FixtureControlCommand::Services),
        _ => Err(CliError::new(format!("未知 fixture 控制子域：{domain}"))),
    }
}

#[derive(Debug)]
pub(super) struct ParsedOptions {
    values: BTreeMap<String, String>,
    write: bool,
}

impl ParsedOptions {
    pub(super) fn parse(args: &[String], allowed: &[&str]) -> Result<Self, CliError> {
        let mut values = BTreeMap::new();
        let mut write = false;
        let mut index = 0;
        while index < args.len() {
            let name = args[index].as_str();
            if name == "--write" {
                if write {
                    return Err(CliError::new("--write 不能重复"));
                }
                write = true;
                index += 1;
                continue;
            }
            if !allowed.contains(&name) {
                return Err(CliError::new(format!("未知 fixture 控制参数：{name}")));
            }
            let value = args
                .get(index + 1)
                .filter(|value| valid_value(value))
                .ok_or_else(|| CliError::new(format!("{name} 缺少有效取值")))?;
            if values.insert(name.to_owned(), value.to_owned()).is_some() {
                return Err(CliError::new(format!("{name} 不能重复")));
            }
            index += 2;
        }
        Ok(Self { values, write })
    }

    pub(super) fn require(&self, name: &str) -> Result<&str, CliError> {
        self.values
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| CliError::new(format!("缺少必需参数 {name}")))
    }

    pub(super) fn optional(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn path(&self, name: &str, kind: LocalTestPathKind) -> Result<PathBuf, CliError> {
        controlled_path(self.require(name)?, kind, name)
    }

    fn optional_path(
        &self,
        name: &str,
        kind: LocalTestPathKind,
    ) -> Result<Option<PathBuf>, CliError> {
        self.optional(name)
            .map(|value| controlled_path(value, kind, name))
            .transpose()
    }
}

fn controlled_path(value: &str, kind: LocalTestPathKind, label: &str) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    validate_local_test_path(&path, &root_dir(), kind)
        .map(|_| path)
        .map_err(|error| CliError::new(format!("{label} 无效：{error}")))
}

pub(super) fn output_file(value: &str, label: &str) -> Result<PathBuf, CliError> {
    let path = controlled_path(value, LocalTestPathKind::OutputFile, label)?;
    validate_existing_parent(&path, label)?;
    Ok(path)
}

pub(super) fn new_directory(value: &str, label: &str) -> Result<PathBuf, CliError> {
    controlled_path(value, LocalTestPathKind::NewDirectory, label)
}

fn validate_existing_parent(path: &Path, label: &str) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::new(format!("{label} 缺少父目录")))?;
    validate_local_test_path(parent, &root_dir(), LocalTestPathKind::ExistingDirectory)
        .map(|_| ())
        .map_err(|error| CliError::new(format!("{label} 的父目录无效：{error}")))
}

pub(super) fn require_write(options: &ParsedOptions, operation: &str) -> Result<(), CliError> {
    if options.write {
        Ok(())
    } else {
        Err(CliError::new(format!(
            "fixture {operation} 会发布或修改本地阶段状态，必须显式传入 --write"
        )))
    }
}

fn reject_write(options: &ParsedOptions, operation: &str) -> Result<(), CliError> {
    if options.write {
        Err(CliError::new(format!(
            "fixture {operation} 是只读操作，不接受 --write"
        )))
    } else {
        Ok(())
    }
}

fn parse_side(value: &str, allow_seed: bool) -> Result<FixtureSide, CliError> {
    match value {
        "seed" if allow_seed => Ok(FixtureSide::Seed),
        "base" => Ok(FixtureSide::Base),
        "candidate" => Ok(FixtureSide::Candidate),
        _ if allow_seed => Err(CliError::new("--side 只允许 seed、base 或 candidate")),
        _ => Err(CliError::new("--side 只允许 base 或 candidate")),
    }
}

fn valid_identifier(value: &str, label: &str) -> Result<String, CliError> {
    if value.len() > 64
        || value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(CliError::new(format!(
            "{label} 只允许 ASCII 字母、数字、点、下划线或连字符，最长 64 字节"
        )))
    } else {
        Ok(value.to_owned())
    }
}

fn valid_copy_name(value: &str, label: &str) -> Result<String, CliError> {
    let bytes = value.as_bytes();
    if !(3..=48).contains(&bytes.len())
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        Err(CliError::new(format!(
            "{label} 必须为 3 至 48 字节，以小写字母或数字开头，且只包含小写字母、数字、下划线或连字符"
        )))
    } else {
        Ok(value.to_owned())
    }
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r', '\0'])
}
