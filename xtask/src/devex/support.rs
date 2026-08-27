use std::{
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use super::model::{CacheState, DevexSuite, StepDefinition};

pub(crate) fn sample_target(
    run_dir: &Path,
    suite: DevexSuite,
    cache_state: CacheState,
    sequence: usize,
) -> PathBuf {
    if suite == DevexSuite::RustSccache {
        return run_dir.join(format!("cache/sccache-measure-{sequence:03}"));
    }
    match cache_state {
        CacheState::Cold => run_dir.join(format!("cache/cold-{sequence:03}")),
        CacheState::Warm => run_dir.join("cache/warm"),
    }
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
