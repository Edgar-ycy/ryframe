use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    Result,
    process::{command_output_with_env, run_with_env_removed},
};

use super::context::resolve_target_dir;

const RESOURCE_WORKSPACE_TEST: &str = "resource_workspace_compilation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceWorkspaceProfile {
    Full,
    Targeted,
}

pub(crate) fn resource_workspace_compilation(
    root: &Path,
    frontend_dir: &Path,
    target_dir: &str,
    jobs: usize,
) -> Result<()> {
    resource_workspace_compilation_with_profile(
        root,
        frontend_dir,
        target_dir,
        jobs,
        ResourceWorkspaceProfile::Full,
    )
}

pub(crate) fn targeted_resource_workspace_compilation(
    root: &Path,
    frontend_dir: &Path,
    target_dir: &str,
    jobs: usize,
) -> Result<()> {
    resource_workspace_compilation_with_profile(
        root,
        frontend_dir,
        target_dir,
        jobs,
        ResourceWorkspaceProfile::Targeted,
    )
}

fn resource_workspace_compilation_with_profile(
    root: &Path,
    frontend_dir: &Path,
    target_dir: &str,
    jobs: usize,
    profile: ResourceWorkspaceProfile,
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
    let control = resource_workspace_control(root, &frontend_dir, &shared_target, profile)?;
    run_with_env_removed(
        control.path(),
        executable,
        &["--ignored", "--nocapture"],
        &[("CARGO_BUILD_JOBS", jobs.as_str())],
        &[],
    )
}

pub(crate) fn resolve_frontend_dir(root: &Path, frontend_dir: &Path) -> Result<PathBuf> {
    let candidate = if frontend_dir.is_absolute() {
        frontend_dir.to_path_buf()
    } else {
        root.join(frontend_dir)
    };
    // Windows 的 canonicalize 会生成 `\\?\` 路径；Node 24 无法把它作为入口脚本路径。
    // 这里仅需要固定相对路径的基准，不需要解析符号链接或目录联接。
    std::path::absolute(&candidate).map_err(|error| {
        format!(
            "无法解析资源 Workspace 使用的前端目录 {}：{error}",
            candidate.display()
        )
        .into()
    })
}

pub(crate) fn resource_workspace_control(
    root: &Path,
    frontend_dir: &Path,
    shared_target: &Path,
    profile: ResourceWorkspaceProfile,
) -> Result<tempfile::TempDir> {
    let parent = root.join(".local-tests/resource-workspace-control");
    fs::create_dir_all(&parent).map_err(|error| {
        format!(
            "无法创建资源 Workspace 控制目录 {}：{error}",
            parent.display()
        )
    })?;
    let directory = tempfile::Builder::new()
        .prefix("invocation-")
        .tempdir_in(&parent)?;
    let path = directory.path().join("resource-workspace-control.json");
    let value = serde_json::json!({
        "format_version": 1,
        "frontend_dir": absolute_text(frontend_dir, "前端目录")?,
        "profile": match profile {
            ResourceWorkspaceProfile::Full => "full",
            ResourceWorkspaceProfile::Targeted => "targeted",
        },
        "target_dir": absolute_text(shared_target, "资源构建目录")?,
    });
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    Ok(directory)
}

fn absolute_text(path: &Path, label: &str) -> Result<String> {
    let path = std::path::absolute(path)
        .map_err(|error| format!("无法解析资源 Workspace {label} {}：{error}", path.display()))?;
    path.to_str()
        .filter(|value| !value.contains(['\r', '\n', '\0']))
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("资源 Workspace {label} 必须是无控制字符的 UTF-8 路径").into())
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
