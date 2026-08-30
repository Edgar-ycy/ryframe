use std::{
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use crate::{Result, workspace::remove_isolated_directory};

use super::model::{CacheState, DevexSuite, StepDefinition};

pub(crate) fn sample_target(
    run_dir: &Path,
    suite: DevexSuite,
    cache_state: CacheState,
    sequence: usize,
) -> PathBuf {
    if suite == DevexSuite::RustSccache {
        // 每轮成功后都会精确删除该目录。保持同一个目标路径能让 sccache
        // 复用编译键，同时又不会把 Cargo target 的增量产物误当作缓存命中。
        return run_dir.join("cache/sccache-measure");
    }
    if suite.uses_sccache() {
        return run_dir.join(format!("cache/sccache-measure-{sequence:03}"));
    }
    match cache_state {
        CacheState::Cold => run_dir.join(format!("cache/cold-{sequence:03}")),
        CacheState::Warm => run_dir.join("cache/warm"),
    }
}

pub(crate) fn cleanup_successful_sample_target(
    run_dir: &Path,
    target: &Path,
    suite: DevexSuite,
    cache_state: CacheState,
) -> Result<()> {
    if cache_state != CacheState::Cold && !suite.uses_sccache() {
        return Ok(());
    }
    remove_isolated_directory(&run_dir.join("cache"), target)
}

pub(super) fn display_step(step: &StepDefinition) -> String {
    format!("{} {}", step.program, step.args.join(" "))
}

pub(super) fn metric(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.1} ms"))
}

#[cfg(windows)]
pub(super) fn success_status() -> std::io::Result<ExitStatus> {
    Command::new("cmd").args(["/C", "exit", "0"]).status()
}

#[cfg(not(windows))]
pub(super) fn success_status() -> std::io::Result<ExitStatus> {
    Command::new("true").status()
}
