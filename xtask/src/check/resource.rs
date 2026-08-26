use std::path::{Path, PathBuf};

use crate::{
    Result,
    process::{command_output_with_env, run_with_env},
};

use super::context::resolve_target_dir;

const RESOURCE_WORKSPACE_TEST: &str = "resource_workspace_compilation";
const RESOURCE_WORKSPACE_FRONTEND_DIR: &str = "RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR";

pub(crate) fn resource_workspace_compilation(
    root: &Path,
    frontend_dir: &Path,
    target_dir: &str,
    jobs: usize,
) -> Result<()> {
    let frontend_dir = resolve_frontend_dir(root, frontend_dir)?;
    if !frontend_dir.join("node_modules").is_dir() {
        return Err(format!(
            "资源 Workspace 验证所需的前端依赖不存在：{}",
            frontend_dir.join("node_modules").display()
        )
        .into());
    }
    let jobs = jobs.to_string();
    let output = command_output_with_env(
        root,
        "cargo",
        &[
            "test",
            "--locked",
            "--target-dir",
            target_dir,
            "-p",
            "ryframe-generator",
            "--test",
            RESOURCE_WORKSPACE_TEST,
            "--no-run",
            "--message-format=json-render-diagnostics",
        ],
        &[("CARGO_BUILD_JOBS", jobs.as_str())],
    )?;
    let executable = resource_test_executable_from_messages(&output)?;
    let executable = executable
        .to_str()
        .ok_or("资源 Workspace 测试可执行文件路径不是有效 UTF-8")?;
    let shared_target = resolve_target_dir(root, target_dir);
    let owned_environment =
        resource_workspace_environment(&frontend_dir, &shared_target, jobs.as_str());
    let environment = owned_environment
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    run_with_env(
        root,
        executable,
        &["--ignored", "--nocapture"],
        &environment,
    )
}

fn resolve_frontend_dir(root: &Path, frontend_dir: &Path) -> Result<PathBuf> {
    let candidate = if frontend_dir.is_absolute() {
        frontend_dir.to_path_buf()
    } else {
        root.join(frontend_dir)
    };
    candidate.canonicalize().map_err(|error| {
        format!(
            "无法解析资源 Workspace 使用的前端目录 {}：{error}",
            candidate.display()
        )
        .into()
    })
}

pub(crate) fn resource_workspace_environment(
    frontend_dir: &Path,
    shared_target: &Path,
    jobs: &str,
) -> [(&'static str, String); 3] {
    [
        ("CARGO_BUILD_JOBS", jobs.to_owned()),
        (
            RESOURCE_WORKSPACE_FRONTEND_DIR,
            frontend_dir.to_string_lossy().into_owned(),
        ),
        (
            "RYFRAME_RESOURCE_WORKSPACE_TARGET_DIR",
            shared_target.to_string_lossy().into_owned(),
        ),
    ]
}

pub(crate) fn resource_test_executable_from_messages(output: &str) -> Result<PathBuf> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|message| {
            message.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
                && message
                    .pointer("/target/name")
                    .and_then(serde_json::Value::as_str)
                    == Some(RESOURCE_WORKSPACE_TEST)
        })
        .and_then(|message| {
            message
                .get("executable")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
        })
        .ok_or_else(|| "Cargo 输出缺少资源 Workspace 测试可执行文件".into())
}
