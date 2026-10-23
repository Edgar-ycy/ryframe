use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;

use crate::{
    Result,
    check::{TaskExecutor, TaskPlan},
    cli::{
        FrontendSourceEvent, FrontendSourceOptions, RepositorySlug, valid_frontend_source_ref,
        valid_frontend_source_sha,
    },
    process,
    workspace::root_dir,
};

const OPENAPI_PATH: &str = "openapi/openapi.json";

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
    println!("开始 CI 原子任务：{}", task_plan.tasks[0].id);
    let repository =
        select_frontend_repository(&root, || env::var("GITHUB_REPOSITORY_OWNER").ok())?;
    let (reference, changed) = select_ci_frontend_ref(options, &root)?;
    write_outputs(&reference, changed, &repository)
}

/// 文件值优先；标准 GitHub owner 只在字段缺省时参与选择，不读取项目环境覆盖。
pub(crate) fn select_frontend_repository(
    root: &Path,
    github_owner: impl FnOnce() -> Option<String>,
) -> Result<String> {
    const REPAIR: &str = "请在 config/tools.toml 或 .local-tests/tools.toml 的 [workspace] 设置 frontend_repository='owner/repository'；本机文件优先。";
    let settings = ryframe_config::ToolSettings::load(root)?;
    if let Some(repository) = settings.workspace.frontend_repository {
        return RepositorySlug::parse(&repository, "workspace.frontend_repository")
            .map(|slug| slug.as_str().to_owned())
            .map_err(|_| {
                format!("工具配置 workspace.frontend_repository 为空或不是有效仓库标识；{REPAIR}")
                    .into()
            });
    }
    let owner = github_owner().ok_or_else(|| {
        format!("未配置 workspace.frontend_repository，且 GitHub 标准身份 GITHUB_REPOSITORY_OWNER 缺失或不是 UTF-8；{REPAIR}")
    })?;
    RepositorySlug::parse(&format!("{owner}/ryframe-vue3"), "GITHUB_REPOSITORY_OWNER")
        .map(|slug| slug.as_str().to_owned())
        .map_err(|_| format!("GitHub 标准身份 GITHUB_REPOSITORY_OWNER 无效；{REPAIR}").into())
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

pub(crate) fn select_ci_frontend_ref(
    options: &FrontendSourceOptions,
    root: &Path,
) -> Result<(String, bool)> {
    if options.event_name != FrontendSourceEvent::PullRequest {
        return Ok(release_reference(options.release_ref.as_deref()));
    }
    let event = options
        .event
        .as_deref()
        .ok_or("pull_request 选择缺少事件文件")?;
    let body = pull_request_body(event)?;
    let Some(base_sha) = options.base_sha.as_deref() else {
        if options.fallback_main_on_invalid_base {
            return Ok((
                select_frontend_ref(&body, false, options.prefer_marker)?,
                false,
            ));
        }
        return Err("pull_request 选择缺少事件文件或基线提交".into());
    };
    let base_sha = checked_commit(base_sha, "后端基线提交")?;
    if !commit_exists_in_worktree(root, &base_sha) {
        if options.fallback_main_on_invalid_base {
            return Ok((
                select_frontend_ref(&body, false, options.prefer_marker)?,
                false,
            ));
        }
        return Err("pull_request 后端基线提交无法解析".into());
    }
    let changed = match options.candidate_openapi.as_deref() {
        Some(candidate) => generate_and_classify_candidate(root, &base_sha, candidate)?,
        None => contract_changed_from_git(root, &base_sha)?,
    };
    Ok((
        select_frontend_ref(&body, changed, options.prefer_marker)?,
        changed,
    ))
}

fn release_reference(value: Option<&str>) -> (String, bool) {
    let Some(version) = value.and_then(|value| value.strip_prefix("refs/tags/")) else {
        return ("main".to_owned(), false);
    };
    if is_release_version(version) {
        (version.to_owned(), false)
    } else {
        ("main".to_owned(), false)
    }
}

fn is_release_version(value: &str) -> bool {
    let Some((major, rest)) = value
        .strip_prefix('v')
        .and_then(|value| value.split_once('.'))
    else {
        return false;
    };
    let Some((minor, patch)) = rest.split_once('.') else {
        return false;
    };
    [major, minor, patch].iter().all(|part| {
        !part.is_empty() && part.len() <= 9 && part.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub(crate) fn select_frontend_ref(
    body: &str,
    contract_changed: bool,
    prefer_marker: bool,
) -> Result<String> {
    let body = body.replace("\r\n", "\n").replace('\r', "\n");
    if !contract_changed && !prefer_marker {
        return Ok("main".to_owned());
    }
    let mentions = body.to_ascii_lowercase().matches("frontend-commit").count();
    let matches = marker_values(&body);
    if !contract_changed && mentions == 0 {
        return Ok("main".to_owned());
    }
    if matches.len() != 1 || mentions != 1 {
        if contract_changed {
            return Err(
                "OpenAPI 已变化；PR 正文必须且只能包含一行 Frontend-Commit: <40 位 SHA>".into(),
            );
        }
        return Err(
            "Frontend-Commit marker 无效；PR 正文必须且只能包含一行 Frontend-Commit: <40 位 SHA>"
                .into(),
        );
    }
    Ok(matches[0].to_ascii_lowercase())
}

fn marker_values(body: &str) -> Vec<&str> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim_matches([' ', '\t']);
            let value = line.strip_prefix("Frontend-Commit:")?;
            let value = value.trim_matches([' ', '\t']);
            (value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then_some(value)
        })
        .collect()
}

fn checked_commit(value: &str, label: &str) -> Result<String> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label}必须是 40 位 Git SHA").into());
    }
    Ok(value.to_ascii_lowercase())
}

fn contract_changed_from_git(root: &Path, base_sha: &str) -> Result<bool> {
    let output = command_output(
        root,
        "git",
        &["diff", "--quiet", base_sha, "HEAD", "--", OPENAPI_PATH],
    )?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(command_failure("无法比较后端 OpenAPI 变更", &output)),
    }
}

fn commit_exists_in_worktree(root: &Path, commit: &str) -> bool {
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    command_output(
        root,
        "git",
        &["cat-file", "-e", &format!("{commit}^{{commit}}")],
    )
    .is_ok_and(|output| output.status.success())
}

fn generate_and_classify_candidate(
    root: &Path,
    base_sha: &str,
    candidate_path: &Path,
) -> Result<bool> {
    if let Some(parent) = candidate_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("无法创建候选 OpenAPI 父目录 {}：{error}", parent.display())
        })?;
    }
    process::run(
        root,
        "cargo",
        &[
            "run",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "ryframe",
            "--bin",
            "export_openapi",
            "--",
            candidate_path
                .to_str()
                .ok_or("候选 OpenAPI 路径必须能表示为 UTF-8")?,
        ],
    )?;
    let committed_path = root.join(OPENAPI_PATH);
    let committed = fs::read(&committed_path).map_err(|error| {
        format!(
            "无法读取已提交 OpenAPI 快照 {}：{error}",
            committed_path.display()
        )
    })?;
    let candidate = fs::read(candidate_path)
        .map_err(|error| format!("无法读取候选 OpenAPI {}：{error}", candidate_path.display()))?;
    if committed != candidate {
        return Err("后端提交未包含当前代码生成的最新 OpenAPI 快照".into());
    }
    let output = command_output(
        root,
        "git",
        &["show", &format!("{base_sha}:{OPENAPI_PATH}")],
    )?;
    if !output.status.success() {
        return Err(command_failure("无法读取基线 OpenAPI 快照", &output));
    }
    Ok(output.stdout != candidate)
}

fn pull_request_body(event_path: &Path) -> Result<String> {
    let bytes = fs::read(event_path)
        .map_err(|error| format!("无法读取 CI 事件 {}：{error}", event_path.to_string_lossy()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("无法读取 CI 事件 {}：{error}", event_path.to_string_lossy()))?;
    let event: Value = serde_json::from_str(text)
        .map_err(|error| format!("无法读取 CI 事件 {}：{error}", event_path.to_string_lossy()))?;
    let pull_request = event
        .get("pull_request")
        .and_then(Value::as_object)
        .ok_or("CI 事件缺少 pull_request 对象")?;
    match pull_request.get("body") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(body)) => Ok(body.clone()),
        Some(_) => Err("pull_request.body 必须是字符串或 null".into()),
    }
}

fn write_outputs(reference: &str, changed: bool, repository: &str) -> Result<()> {
    let text = format!("ref={reference}\nchanged={changed}\nrepository={repository}\n");
    if let Some(output) = env::var_os("GITHUB_OUTPUT") {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(PathBuf::from(output))
            .map_err(|error| format!("无法写入 GITHUB_OUTPUT：{error}"))?;
        file.write_all(text.as_bytes())
            .map_err(|error| format!("无法写入 GITHUB_OUTPUT：{error}"))?;
    }
    print!("{text}");
    Ok(())
}

fn command_output(
    root: &Path,
    executable: &str,
    arguments: &[&str],
) -> Result<std::process::Output> {
    Command::new(executable)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("无法执行 {executable} {}：{error}", arguments.join(" ")).into())
}

fn command_failure(label: &str, output: &std::process::Output) -> Box<dyn std::error::Error> {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let detail = if stderr.is_empty() {
        output
            .status
            .code()
            .map_or_else(|| "进程被信号终止".to_owned(), |code| code.to_string())
    } else {
        stderr
    };
    format!("{label}：{detail}").into()
}
