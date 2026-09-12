use std::path::PathBuf;

use super::super::super::model::{
    CliError, FrontendSourceEvent, FrontendSourceOptions, valid_frontend_source_ref,
    valid_frontend_source_sha,
};

pub(super) fn parse_frontend_source(args: &[String]) -> Result<FrontendSourceOptions, CliError> {
    let mut input = FrontendSourceInput::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--event-name" => {
                replace_value(
                    &mut input.event_name,
                    args,
                    &mut index,
                    "--event-name",
                    false,
                )?;
            }
            "--event" => replace_path(&mut input.event, args, &mut index, "--event")?,
            "--base-sha" => {
                replace_optional_value(
                    &mut input.base_sha,
                    &mut input.base_sha_seen,
                    args,
                    &mut index,
                    "--base-sha",
                )?;
            }
            "--prefer-marker" => {
                set_flag(&mut input.prefer_marker, "--prefer-marker")?;
                index += 1;
            }
            "--candidate-openapi" => replace_path(
                &mut input.candidate_openapi,
                args,
                &mut index,
                "--candidate-openapi",
            )?,
            "--release-ref" => {
                replace_optional_value(
                    &mut input.release_ref,
                    &mut input.release_ref_seen,
                    args,
                    &mut index,
                    "--release-ref",
                )?;
            }
            "--fallback-main-on-invalid-base" => {
                set_flag(
                    &mut input.fallback_main_on_invalid_base,
                    "--fallback-main-on-invalid-base",
                )?;
                index += 1;
            }
            unknown => return Err(CliError::new(format!("未知参数：{unknown}"))),
        }
    }
    build_options(input)
}

#[derive(Default)]
struct FrontendSourceInput {
    event_name: Option<String>,
    event: Option<PathBuf>,
    base_sha: Option<String>,
    base_sha_seen: bool,
    prefer_marker: bool,
    candidate_openapi: Option<PathBuf>,
    release_ref: Option<String>,
    release_ref_seen: bool,
    fallback_main_on_invalid_base: bool,
}

fn build_options(input: FrontendSourceInput) -> Result<FrontendSourceOptions, CliError> {
    let event_name = parse_event(
        input
            .event_name
            .as_deref()
            .ok_or_else(|| CliError::new("缺少必需参数 --event-name"))?,
    )?;
    validate_paths(&input)?;
    validate_source_values(&input)?;
    if event_name == FrontendSourceEvent::PullRequest && input.event.is_none() {
        return Err(CliError::new("pull_request 事件必须提供 --event"));
    }
    if event_name == FrontendSourceEvent::PullRequest
        && input.base_sha.is_none()
        && !input.fallback_main_on_invalid_base
    {
        return Err(CliError::new(
            "pull_request 事件缺少有效 --base-sha；仅资源回退可显式使用 --fallback-main-on-invalid-base",
        ));
    }
    if input.candidate_openapi.is_some() && input.fallback_main_on_invalid_base {
        return Err(CliError::new(
            "--candidate-openapi 与 --fallback-main-on-invalid-base 不能同时使用",
        ));
    }
    Ok(FrontendSourceOptions {
        event_name,
        event: input.event,
        base_sha: input.base_sha,
        prefer_marker: input.prefer_marker,
        candidate_openapi: input.candidate_openapi,
        release_ref: input.release_ref,
        fallback_main_on_invalid_base: input.fallback_main_on_invalid_base,
    })
}

fn validate_source_values(input: &FrontendSourceInput) -> Result<(), CliError> {
    if input
        .base_sha
        .as_deref()
        .is_some_and(|sha| !valid_frontend_source_sha(sha))
    {
        return Err(CliError::new(
            "--base-sha 必须是非零的 40 位小写十六进制 SHA",
        ));
    }
    if input
        .release_ref
        .as_deref()
        .is_some_and(|reference| !valid_frontend_source_ref(reference))
    {
        return Err(CliError::new("--release-ref 不是安全、完整的 Git ref"));
    }
    Ok(())
}

fn parse_event(value: &str) -> Result<FrontendSourceEvent, CliError> {
    match value {
        "pull_request" => Ok(FrontendSourceEvent::PullRequest),
        "push" => Ok(FrontendSourceEvent::Push),
        "schedule" => Ok(FrontendSourceEvent::Schedule),
        "workflow_dispatch" => Ok(FrontendSourceEvent::WorkflowDispatch),
        _ => Err(CliError::new(
            "--event-name 只允许 pull_request、push、schedule 或 workflow_dispatch",
        )),
    }
}

fn validate_paths(input: &FrontendSourceInput) -> Result<(), CliError> {
    for (option, path) in [
        ("--event", input.event.as_ref()),
        ("--candidate-openapi", input.candidate_openapi.as_ref()),
    ] {
        if path.is_some_and(|path| !path.is_absolute()) {
            return Err(CliError::new(format!("{option} 必须是绝对路径")));
        }
    }
    Ok(())
}

fn replace_path(
    target: &mut Option<PathBuf>,
    args: &[String],
    index: &mut usize,
    option: &str,
) -> Result<(), CliError> {
    let mut value = None;
    replace_value(&mut value, args, index, option, false)?;
    if target
        .replace(PathBuf::from(value.expect("选项解析成功后必须有值")))
        .is_some()
    {
        return Err(CliError::new(format!("{option} 不能重复")));
    }
    Ok(())
}

fn replace_optional_value(
    target: &mut Option<String>,
    seen: &mut bool,
    args: &[String],
    index: &mut usize,
    option: &str,
) -> Result<(), CliError> {
    if *seen {
        return Err(CliError::new(format!("{option} 不能重复")));
    }
    let value = raw_option_value(args, *index, option, true)?;
    *target = (!value.is_empty()).then(|| value.to_owned());
    *seen = true;
    *index += 2;
    Ok(())
}

fn replace_value(
    target: &mut Option<String>,
    args: &[String],
    index: &mut usize,
    option: &str,
    allow_empty: bool,
) -> Result<(), CliError> {
    let value = raw_option_value(args, *index, option, allow_empty)?.to_owned();
    if target.replace(value).is_some() {
        return Err(CliError::new(format!("{option} 不能重复")));
    }
    *index += 2;
    Ok(())
}

fn raw_option_value<'a>(
    args: &'a [String],
    index: usize,
    option: &str,
    allow_empty: bool,
) -> Result<&'a str, CliError> {
    args.get(index + 1)
        .filter(|value| (allow_empty || !value.trim().is_empty()) && !value.starts_with('-'))
        .filter(|value| {
            !value
                .chars()
                .any(|character| matches!(character, '\n' | '\r' | '\0'))
        })
        .map(String::as_str)
        .ok_or_else(|| CliError::new(format!("{option} 缺少或包含非法取值")))
}

fn set_flag(target: &mut bool, option: &str) -> Result<(), CliError> {
    if *target {
        return Err(CliError::new(format!("{option} 不能重复")));
    }
    *target = true;
    Ok(())
}
