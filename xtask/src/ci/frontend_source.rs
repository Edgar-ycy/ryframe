use std::{fs, path::Path};

use serde::Serialize;

use crate::{
    Result,
    check::{TaskExecutor, TaskPlan},
    cli::{
        FrontendSourceEvent, FrontendSourceOptions, valid_frontend_source_ref,
        valid_frontend_source_sha,
    },
    process::run_owned_with_env,
    workspace::root_dir,
};

const REQUEST_ENV: &str = "RYFRAME_CI_FRONTEND_SOURCE_REQUEST";
const SCRIPT: &str = "tools/python/select_frontend_commit.py";

pub(super) fn plan(options: &FrontendSourceOptions) -> Result<TaskPlan> {
    validate_options(options)?;
    let executor = if uses_candidate(options) {
        TaskExecutor::CiFrontendCandidateSource
    } else {
        TaskExecutor::CiFrontendSource
    };
    TaskPlan::sequence(&[executor])
}

fn uses_candidate(options: &FrontendSourceOptions) -> bool {
    options.event_name == FrontendSourceEvent::PullRequest && options.candidate_openapi.is_some()
}

pub(super) fn run(options: &FrontendSourceOptions, task_plan: &TaskPlan) -> Result<()> {
    let expected = plan(options)?;
    if task_plan != &expected {
        return Err("前端来源选择计划与登记的原子任务不一致".into());
    }
    validate_runtime_inputs(options)?;
    let root = root_dir()
        .canonicalize()
        .map_err(|error| format!("无法规范化当前后端工作树：{error}"))?;
    let request = request_json(options, &root)?;
    println!("开始 CI 原子任务：{}", task_plan.tasks[0].id);
    run_owned_with_env(
        &root,
        "python",
        &[SCRIPT.to_owned()],
        &[(REQUEST_ENV, request)],
    )
}

fn validate_options(options: &FrontendSourceOptions) -> Result<()> {
    if options
        .event
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err("前端来源事件文件必须是绝对路径".into());
    }
    if options
        .candidate_openapi
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err("候选 OpenAPI 必须是绝对路径".into());
    }
    if options.event_name == FrontendSourceEvent::PullRequest && options.event.is_none() {
        return Err("pull_request 前端来源选择缺少事件文件".into());
    }
    if options.event_name == FrontendSourceEvent::PullRequest
        && options.base_sha.is_none()
        && !options.fallback_main_on_invalid_base
    {
        return Err("pull_request 前端来源选择缺少基线 SHA".into());
    }
    if options.candidate_openapi.is_some() && options.fallback_main_on_invalid_base {
        return Err("候选 OpenAPI 与无效基线回退不能同时使用".into());
    }
    if options
        .base_sha
        .as_deref()
        .is_some_and(|sha| !valid_frontend_source_sha(sha))
    {
        return Err("前端来源基线必须是非零的 40 位小写十六进制 SHA".into());
    }
    if options
        .release_ref
        .as_deref()
        .is_some_and(|reference| !valid_frontend_source_ref(reference))
    {
        return Err("前端来源 release ref 无效".into());
    }
    Ok(())
}

fn validate_runtime_inputs(options: &FrontendSourceOptions) -> Result<()> {
    if let Some(event) = &options.event {
        let metadata = fs::metadata(event)
            .map_err(|error| format!("无法读取前端来源事件文件 {}：{error}", event.display()))?;
        if !metadata.is_file() {
            return Err(format!("前端来源事件路径不是文件：{}", event.display()).into());
        }
        fs::read_to_string(event).map_err(|error| {
            format!(
                "前端来源事件文件不是可读的 UTF-8 文本 {}：{error}",
                event.display()
            )
        })?;
    }
    if let Some(candidate) = &options.candidate_openapi
        && candidate.is_dir()
    {
        return Err(format!("候选 OpenAPI 路径不能是目录：{}", candidate.display()).into());
    }
    Ok(())
}

#[derive(Serialize)]
struct FrontendSourceRequest<'a> {
    event_name: &'static str,
    event: Option<&'a str>,
    backend_worktree: &'a str,
    base_sha: Option<&'a str>,
    prefer_marker: bool,
    candidate_openapi: Option<&'a str>,
    release_ref: Option<&'a str>,
    fallback_main_on_invalid_base: bool,
}

fn request_json(options: &FrontendSourceOptions, root: &Path) -> Result<String> {
    let request = FrontendSourceRequest {
        event_name: options.event_name.as_str(),
        event: optional_utf8(options.event.as_deref(), "事件文件")?,
        backend_worktree: utf8(root, "后端工作树")?,
        base_sha: options.base_sha.as_deref(),
        prefer_marker: options.prefer_marker,
        candidate_openapi: optional_utf8(options.candidate_openapi.as_deref(), "候选 OpenAPI")?,
        release_ref: options.release_ref.as_deref(),
        fallback_main_on_invalid_base: options.fallback_main_on_invalid_base,
    };
    Ok(serde_json::to_string(&request)?)
}

fn optional_utf8<'a>(path: Option<&'a Path>, label: &str) -> Result<Option<&'a str>> {
    path.map(|path| utf8(path, label)).transpose()
}

fn utf8<'a>(path: &'a Path, label: &str) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| format!("{label}路径必须能表示为 UTF-8").into())
}
