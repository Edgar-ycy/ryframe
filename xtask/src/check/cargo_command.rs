use std::collections::BTreeSet;

use crate::Result;

use super::context::BACKEND_VERIFY_TARGET_DIR;

pub(crate) const WORKSPACE_CLIPPY_ARGS: &[&str] = &[
    "clippy",
    "--locked",
    "--target-dir",
    BACKEND_VERIFY_TARGET_DIR,
    "--workspace",
    "--all-targets",
    "--all-features",
    "--",
    "-D",
    "warnings",
    "-D",
    "clippy::redundant_clone",
];

pub(crate) fn backend_package_operation_args(
    operation: &str,
    packages: &BTreeSet<String>,
    target_dir: &str,
    jobs: usize,
) -> Vec<String> {
    let mut args = vec![
        operation.to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
    ];
    for package in packages {
        args.push("-p".to_owned());
        args.push(package.clone());
    }
    if operation == "clippy" {
        args.push("--all-targets".to_owned());
    }
    args.extend(["--jobs".to_owned(), jobs.max(1).to_string()]);
    if operation == "clippy" {
        args.extend([
            "--".to_owned(),
            "-D".to_owned(),
            "warnings".to_owned(),
            "-D".to_owned(),
            "clippy::redundant_clone".to_owned(),
        ]);
    }
    args
}

pub(crate) fn workspace_clippy_args(target_dir: &str, jobs: usize) -> Vec<String> {
    let mut args = WORKSPACE_CLIPPY_ARGS
        .iter()
        .map(|argument| {
            if *argument == BACKEND_VERIFY_TARGET_DIR {
                target_dir.to_owned()
            } else {
                (*argument).to_owned()
            }
        })
        .collect::<Vec<_>>();
    let cargo_end = args
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(args.len());
    args.splice(
        cargo_end..cargo_end,
        ["--jobs".to_owned(), jobs.max(1).to_string()],
    );
    args
}

pub(crate) fn workspace_test_args(target_dir: &str, jobs: usize) -> Vec<String> {
    [
        "test",
        "--locked",
        "--target-dir",
        target_dir,
        "--workspace",
        "--all-features",
        "--jobs",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([jobs.max(1).to_string()])
    .collect()
}

/// Windows 链接与测试进程的峰值内存较高，默认限制为四并发。
pub(crate) fn default_test_jobs_from(windows: bool, fallback: usize) -> usize {
    if windows { 4 } else { fallback.max(1) }
}

pub(crate) fn cargo_operation_jobs(operation: &str, windows: bool, fallback: usize) -> usize {
    if operation == "test" {
        default_test_jobs_from(windows, fallback)
    } else {
        fallback.max(1)
    }
}

pub(crate) fn ci_test_jobs_from(
    configured: Option<&str>,
    windows: bool,
    fallback: usize,
) -> Result<usize> {
    match configured {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|jobs| (1..=64).contains(jobs))
            .ok_or_else(|| "RYFRAME_CI_TEST_JOBS 必须是 1 到 64 的整数".into()),
        None => Ok(default_test_jobs_from(windows, fallback)),
    }
}
