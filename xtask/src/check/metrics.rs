use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock},
};

use chrono::Utc;
use serde::Serialize;

use crate::process::{install_step_observer, rustc_cache_label};

#[derive(Debug, Serialize)]
struct VerifyStep {
    command: String,
    duration_seconds: f64,
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct TargetState {
    backend_warm: bool,
    resource_warm: bool,
}

#[derive(Debug, Serialize)]
struct VerifyRun {
    version: u8,
    started_at: String,
    backend_commit: Option<String>,
    scope: String,
    mode: String,
    backend_dirty: Option<bool>,
    frontend_dirty: Option<bool>,
    rustc_cache: String,
    targets: TargetState,
    steps: Vec<VerifyStep>,
    total_seconds: f64,
    status: &'static str,
}

struct ActiveVerify {
    root: std::path::PathBuf,
    run: VerifyRun,
}

static ACTIVE_VERIFY: OnceLock<Mutex<Option<ActiveVerify>>> = OnceLock::new();

pub(crate) fn begin(root: &Path, frontend: &Path, scope: &str, mode: &str) {
    install_step_observer(record_step);
    let active = ActiveVerify {
        root: root.to_path_buf(),
        run: VerifyRun {
            version: 1,
            started_at: Utc::now().to_rfc3339(),
            backend_commit: git_text(root, &["rev-parse", "HEAD"]),
            scope: scope.to_owned(),
            mode: mode.to_owned(),
            backend_dirty: git_dirty(root),
            frontend_dirty: git_dirty(frontend),
            rustc_cache: rustc_cache_label().to_owned(),
            targets: TargetState {
                backend_warm: directory_has_entries(&root.join("target/verify/backend")),
                resource_warm: directory_has_entries(&root.join("target/verify/resource")),
            },
            steps: Vec::new(),
            total_seconds: 0.0,
            status: "running",
        },
    };
    if let Ok(mut slot) = ACTIVE_VERIFY.get_or_init(Default::default).lock() {
        *slot = Some(active);
    } else {
        eprintln!("无法初始化 verify 指标记录；检查继续执行。");
    }
}

pub(crate) fn record_step(command: String, duration_seconds: f64, succeeded: bool) {
    let Some(mutex) = ACTIVE_VERIFY.get() else {
        return;
    };
    let Ok(mut slot) = mutex.lock() else {
        return;
    };
    let Some(active) = slot.as_mut() else {
        return;
    };
    active.run.steps.push(VerifyStep {
        command,
        duration_seconds,
        status: if succeeded { "passed" } else { "failed" },
    });
}

pub(crate) fn finish(mode: &str, total_seconds: f64, succeeded: bool) {
    let Some(mutex) = ACTIVE_VERIFY.get() else {
        return;
    };
    let active = mutex.lock().ok().and_then(|mut slot| slot.take());
    let Some(mut active) = active else {
        return;
    };
    active.run.mode = mode.to_owned();
    active.run.total_seconds = total_seconds;
    active.run.status = if succeeded { "passed" } else { "failed" };
    if let Err(error) = append_metrics(&active.root, &active.run) {
        eprintln!("写入 verify 指标失败：{error}；检查结果不受影响。");
    }
}

fn append_metrics(root: &Path, run: &VerifyRun) -> Result<(), String> {
    let directory = root.join("target/verify");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("无法创建 {}：{error}", directory.display()))?;
    let path = directory.join("metrics.jsonl");
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("无法打开 {}：{error}", path.display()))?;
    serde_json::to_writer(&mut output, run)
        .map_err(|error| format!("无法序列化 {}：{error}", path.display()))?;
    output
        .write_all(b"\n")
        .map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

fn directory_has_entries(path: &Path) -> bool {
    path.is_dir()
        && fs::read_dir(path)
            .ok()
            .and_then(|mut entries| entries.next())
            .is_some()
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_dirty(root: &Path) -> Option<bool> {
    git_text(root, &["status", "--porcelain", "--untracked-files=all"])
        .map(|status| !status.is_empty())
}
