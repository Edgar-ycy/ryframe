use std::{collections::BTreeMap, path::PathBuf};

use super::super::model::{
    CliError, GitSha, ReleaseCiEvidenceOptions, ReleaseCiOperation, ReleaseCiOptions,
    ReleaseCommand, ReleaseSourceOptions, ReleaseTag, ReleaseTagOids, RepositorySlug,
};

const SOURCE_OPTIONS: &[&str] = &[
    "--tag",
    "--backend-repository",
    "--backend-commit",
    "--frontend-repository",
    "--frontend-commit",
    "--manifest-path",
];
const CI_OPTIONS: &[&str] = &[
    "--backend-repository",
    "--frontend-repository",
    "--backend-sha",
    "--frontend-sha",
    "--backend-tag-oid",
    "--frontend-tag-oid",
    "--tag",
    "--timeout",
    "--output",
];

pub(super) fn parse_release(args: &[String]) -> Result<ReleaseCommand, CliError> {
    let Some((mode, rest)) = args.split_first() else {
        return Err(CliError::new(release_usage()));
    };
    match mode.as_str() {
        "source" => parse_source(rest).map(ReleaseCommand::Source),
        "ci" => parse_ci(rest).map(ReleaseCommand::Ci),
        _ => Err(CliError::new(release_usage())),
    }
}

fn parse_source(args: &[String]) -> Result<ReleaseSourceOptions, CliError> {
    let (values, plan) = named_options_with_plan(args, SOURCE_OPTIONS)?;
    let manifest_path = absolute_path(&values, "--manifest-path")?;
    Ok(ReleaseSourceOptions {
        tag: ReleaseTag::parse(required(&values, "--tag")?)?,
        backend_repository: RepositorySlug::parse(
            required(&values, "--backend-repository")?,
            "--backend-repository",
        )?,
        backend_commit: GitSha::parse(required(&values, "--backend-commit")?, "--backend-commit")?,
        frontend_repository: RepositorySlug::parse(
            required(&values, "--frontend-repository")?,
            "--frontend-repository",
        )?,
        frontend_commit: GitSha::parse(
            required(&values, "--frontend-commit")?,
            "--frontend-commit",
        )?,
        manifest_path,
        plan,
    })
}

fn parse_ci(args: &[String]) -> Result<ReleaseCiOptions, CliError> {
    match args.split_first() {
        Some((operation, rest)) if operation == "record-pair" => parse_pair(rest, true),
        Some((operation, rest)) if operation == "verify-pair" => parse_pair(rest, false),
        _ => parse_ci_evidence(args),
    }
}

fn parse_pair(args: &[String], record: bool) -> Result<ReleaseCiOptions, CliError> {
    let option = if record { "--output" } else { "--input" };
    let (values, plan) = named_options_with_plan(args, &[option])?;
    let path = absolute_path(&values, option)?;
    let operation = if record {
        ReleaseCiOperation::RecordPair { output: path }
    } else {
        ReleaseCiOperation::VerifyPair { input: path }
    };
    Ok(ReleaseCiOptions { operation, plan })
}

fn parse_ci_evidence(args: &[String]) -> Result<ReleaseCiOptions, CliError> {
    let (values, plan) = named_options_with_plan(args, CI_OPTIONS)?;
    let backend_tag_oid = optional_sha(&values, "--backend-tag-oid")?;
    let frontend_tag_oid = optional_sha(&values, "--frontend-tag-oid")?;
    let tag_oids = match (backend_tag_oid, frontend_tag_oid) {
        (None, None) => None,
        (Some(backend), Some(frontend)) => Some(ReleaseTagOids { backend, frontend }),
        _ => {
            return Err(CliError::new(
                "--backend-tag-oid 与 --frontend-tag-oid 必须同时提供",
            ));
        }
    };
    let timeout_seconds = required(&values, "--timeout")?
        .parse::<u16>()
        .ok()
        .filter(|timeout| (1..=14_400).contains(timeout))
        .ok_or_else(|| CliError::new("--timeout 必须是 1..14400 的十进制秒数"))?;
    let options = ReleaseCiEvidenceOptions {
        backend_repository: RepositorySlug::parse(
            required(&values, "--backend-repository")?,
            "--backend-repository",
        )?,
        frontend_repository: RepositorySlug::parse(
            required(&values, "--frontend-repository")?,
            "--frontend-repository",
        )?,
        backend_sha: GitSha::parse(required(&values, "--backend-sha")?, "--backend-sha")?,
        frontend_sha: GitSha::parse(required(&values, "--frontend-sha")?, "--frontend-sha")?,
        tag: ReleaseTag::parse(required(&values, "--tag")?)?,
        timeout_seconds,
        output: absolute_path(&values, "--output")?,
        tag_oids,
    };
    Ok(ReleaseCiOptions {
        operation: ReleaseCiOperation::Evidence(options),
        plan,
    })
}

fn optional_sha(
    values: &BTreeMap<String, String>,
    option: &str,
) -> Result<Option<GitSha>, CliError> {
    values
        .get(option)
        .map(|value| GitSha::parse(value, option))
        .transpose()
}

fn absolute_path(values: &BTreeMap<String, String>, option: &str) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(required(values, option)?);
    if !path.is_absolute() {
        return Err(CliError::new(format!("{option} 必须是绝对路径")));
    }
    Ok(path)
}

fn named_options_with_plan(
    args: &[String],
    allowed: &[&str],
) -> Result<(BTreeMap<String, String>, bool), CliError> {
    let mut values = BTreeMap::new();
    let mut plan = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--plan" {
            if plan {
                return Err(CliError::new("--plan 不能重复"));
            }
            plan = true;
            index += 1;
            continue;
        }
        if !allowed.contains(&option) {
            return Err(CliError::new(format!("未知参数：{option}")));
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if values.insert(option.to_owned(), value.clone()).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    Ok((values, plan))
}

fn required<'a>(values: &'a BTreeMap<String, String>, option: &str) -> Result<&'a str, CliError> {
    values
        .get(option)
        .map(String::as_str)
        .ok_or_else(|| CliError::new(format!("缺少必需参数 {option}")))
}

fn release_usage() -> &'static str {
    "用法：cargo xtask check release source ... | cargo xtask check release ci ..."
}
