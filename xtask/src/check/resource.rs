use std::path::PathBuf;

use crate::{
    Result,
    process::{command_output_with_env, run_with_env},
};

use super::context::resolve_target_dir;

const RESOURCE_WORKSPACE_TEST: &str = "resource_workspace_compilation";

pub(crate) fn resource_workspace_compilation(
    root: &std::path::Path,
    target_dir: &str,
    jobs: usize,
) -> Result<()> {
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
    let shared_target = shared_target.to_string_lossy().into_owned();
    run_with_env(
        root,
        executable,
        &["--ignored", "--nocapture"],
        &[
            ("CARGO_BUILD_JOBS", jobs.as_str()),
            (
                "RYFRAME_RESOURCE_WORKSPACE_TARGET_DIR",
                shared_target.as_str(),
            ),
        ],
    )
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
