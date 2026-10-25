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
const CONFLICT_TEST_TARGET: &str = "generation_pipeline";
const CONFLICT_TESTS: [&str; 2] = [
    "cross_module_structure_conflicts_reject_before_install_with_or_without_frontend_sync",
    "cross_module_permission_conflict_rejects_before_install_with_or_without_frontend_sync",
];

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
    let (executable, conflicts) = compile_resource_tests(root, target_dir, &jobs, profile)?;
    let executable = executable
        .to_str()
        .ok_or("资源 Workspace 测试可执行文件路径不是有效 UTF-8")?;
    let shared_target = resolve_target_dir(root, target_dir);
    let control = resource_workspace_control(root, &frontend_dir, &shared_target, profile)?;
    if let Some(path) = &conflicts {
        let program = path.to_str().ok_or("冲突测试程序路径不是有效 UTF-8")?;
        let listed = command_output_with_env(
            control.path(),
            program,
            &["--list", "--ignored", "cross_module_"],
            &[("CARGO_BUILD_JOBS", jobs.as_str())],
        )?;
        validate_resource_conflict_tests(&listed)?;
    }
    run_with_env_removed(
        control.path(),
        executable,
        &["--ignored", "--nocapture"],
        &[("CARGO_BUILD_JOBS", jobs.as_str())],
        &[],
    )?;
    if let Some(path) = conflicts {
        run_with_env_removed(
            control.path(),
            path.to_str().ok_or("冲突测试程序路径不是有效 UTF-8")?,
            &["--ignored", "--nocapture", "cross_module_"],
            &[("CARGO_BUILD_JOBS", jobs.as_str())],
            &[],
        )?;
    }
    Ok(())
}

fn compile_resource_tests(
    root: &Path,
    target_dir: &str,
    jobs: &str,
    profile: ResourceWorkspaceProfile,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let mut args = vec![
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
    ];
    if profile == ResourceWorkspaceProfile::Full {
        args.extend(["--test", CONFLICT_TEST_TARGET]);
    }
    let output = command_output_with_env(root, "cargo", &args, &[("CARGO_BUILD_JOBS", jobs)])?;
    let executable = resource_test_executable_from_messages(&output, RESOURCE_WORKSPACE_TEST)?;
    let conflicts = if profile == ResourceWorkspaceProfile::Full {
        Some(resource_test_executable_from_messages(
            &output,
            CONFLICT_TEST_TARGET,
        )?)
    } else {
        None
    };
    Ok((executable, conflicts))
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

pub(crate) fn resource_test_executable_from_messages(
    output: &str,
    target: &str,
) -> Result<PathBuf> {
    let mut matches = output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| {
            message.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
                && message
                    .pointer("/target/name")
                    .and_then(serde_json::Value::as_str)
                    == Some(target)
                && message
                    .pointer("/target/kind")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("test")))
        })
        .filter_map(|message| {
            message
                .get("executable")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
        });
    let executable = matches
        .next()
        .ok_or_else(|| format!("Cargo 输出缺少测试目标 {target} 的可执行文件"))?;
    if matches.next().is_some() {
        return Err(format!("Cargo 输出含多个测试目标 {target} 的可执行文件，拒绝猜测").into());
    }
    Ok(executable)
}

pub(crate) fn validate_resource_conflict_tests(output: &str) -> Result<()> {
    let mut actual = output
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .collect::<Vec<_>>();
    actual.sort_unstable();
    let mut expected = CONFLICT_TESTS;
    expected.sort_unstable();
    if actual != expected {
        return Err(format!(
            "资源冲突测试清单不完整或不明确：预期 {expected:?}，实际 {actual:?}；未运行资源场景"
        )
        .into());
    }
    Ok(())
}
