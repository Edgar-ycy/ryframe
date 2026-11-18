use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

use super::super::export;
use super::{
    lock::{ProcessGuard, RunLock},
    manifest, source, state,
};
use crate::{
    Result,
    cli::CloneStage,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

#[path = "stage/marker_diagnostic.rs"]
mod marker_diagnostic;
pub(in crate::recovery::clone::native) use marker_diagnostic::MarkerCheckpoint;

pub(crate) struct StageRequest<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) run_dir: &'a Path,
    pub(crate) stage: &'a str,
    pub(crate) mode: &'a str,
    pub(crate) attempt: Option<u64>,
}

pub(crate) trait StageRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()>;
    fn guarded_preflight(&self, _request: &StageRequest<'_>) -> Result<()> {
        Ok(())
    }
    fn execute(&self, request: &StageRequest<'_>) -> Result<Value>;
}

pub(crate) struct ExportRunner {
    backend: PathBuf,
}

impl ExportRunner {
    pub(crate) fn new(backend: &Path) -> Self {
        Self {
            backend: backend.to_path_buf(),
        }
    }
}

impl StageRunner for ExportRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        export::preflight(&self.backend, request)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        export::execute(&self.backend, request)
    }
}

pub(crate) fn run(
    backend: &Path,
    run_dir: &Path,
    stage: CloneStage,
    runner: &dyn StageRunner,
) -> Result<Value> {
    run_named(
        backend,
        run_dir,
        stage.as_str(),
        stage.mode().as_str(),
        runner,
    )
}

pub(crate) fn run_named(
    backend: &Path,
    run_dir: &Path,
    stage_name: &str,
    mode: &str,
    runner: &dyn StageRunner,
) -> Result<Value> {
    validate_local_test_path(run_dir, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("复制运行目录无效：{error}"))?;
    let request = StageRequest {
        backend,
        run_dir,
        stage: stage_name,
        mode,
        attempt: None,
    };
    let initial_manifest = stage_manifest(backend, run_dir, stage_name, mode)?;
    let _ = state::load(run_dir, !state::resource_cleanup(stage_name, mode))?;
    runner.preflight(&request)?;

    let guard = ProcessGuard::acquire(run_dir)?;
    runner.guarded_preflight(&request)?;
    let run_lock = RunLock::claim(run_dir)?;
    let execution = (|| {
        if stage_name == "cache-target"
            || stage_name == "post-copy" && matches!(mode, "register" | "prepare" | "amend")
            || stage_name == "seed-runtime"
                && matches!(
                    mode,
                    "source-export" | "source-export-reconcile" | "source-generation-reboot-close"
                )
        {
            runner.preflight(&request)?;
        }
        execute_claimed(
            backend,
            run_dir,
            stage_name,
            mode,
            runner,
            &initial_manifest.value,
            &run_lock,
        )
    })();
    let release = run_lock.release();
    let finish = match execution {
        Ok(mut outcome) => {
            if let Err(error) = release {
                outcome.failure = Some(failure_type(&*error));
            }
            finalize(run_dir, stage_name, mode, outcome)
        }
        Err(error) => {
            if let Err(release) = release {
                return Err(format!(
                    "阶段未完成（{}），恢复运行锁失败（{}）",
                    failure_type(&*error),
                    failure_type(&*release)
                )
                .into());
            }
            finish_unstarted_failure(run_dir, stage_name, mode, &*error)?;
            Err(error)
        }
    };
    guard.check()?;
    match finish {
        Ok(value) => Ok(value),
        Err(error) => Err(error),
    }
}

struct LockedOutcome {
    number: u64,
    result: Option<Value>,
    failure: Option<String>,
    diagnostic: Option<String>,
}

fn execute_claimed(
    backend: &Path,
    run_dir: &Path,
    stage: &str,
    mode: &str,
    runner: &dyn StageRunner,
    initial_manifest: &Value,
    run_lock: &RunLock,
) -> Result<LockedOutcome> {
    let cleanup = state::resource_cleanup(stage, mode);
    let registered = stage_manifest(backend, run_dir, stage, mode)?;
    if registered.value != *initial_manifest {
        return Err("取得控制锁前验收清单发生变化".into());
    }
    let _ = state::load(run_dir, !cleanup)?;
    let sources = match source::current(backend) {
        Ok(value) => value,
        Err(_) if cleanup => json!({"source_capture_error_type":"SourceCaptureError"}),
        Err(error) => return Err(error),
    };
    let (number, attempt) = state::begin(run_dir, stage, mode, &sources)?;
    let mut failure = state::bind_controller(run_dir, number, &run_lock.owner, &attempt)
        .err()
        .map(|error| failure_type(&*error));
    let mut diagnostic = None;
    let result = if failure.is_none() {
        let request = StageRequest {
            backend,
            run_dir,
            stage,
            mode,
            attempt: Some(number),
        };
        match runner.execute(&request) {
            Ok(value) => Some(value),
            Err(error) => {
                failure = Some(failure_type(&*error));
                diagnostic = safe_diagnostic(error.as_ref());
                None
            }
        }
    } else {
        None
    };
    if failure.is_none()
        && result
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            == Some("needs_reconciliation")
    {
        failure = Some("NeedsReconciliation".into());
        if stage == "target-cache-markers"
            && let Some(result) = result.as_ref()
            && let Some(
                classification @ ("before"
                | "mismatch"
                | "unknown"
                | "blocked_before_write"
                | "after"),
            ) = result["classification"].as_str()
            && let Some(path) = result["observation"]["path"].as_str()
        {
            diagnostic = Some(format!("Redis 标记观察为 {classification}，收据 {path}"));
        }
    }
    match stage_manifest(backend, run_dir, stage, mode) {
        Ok(value) if value.value == *initial_manifest => {}
        Ok(_) => failure = Some("ManifestChanged".into()),
        Err(error) => failure = Some(failure_type(&*error)),
    }
    if !cleanup {
        match source::current(backend) {
            Ok(value) if value == sources => {}
            Ok(_) => failure = Some("SourceChanged".into()),
            Err(error) => failure = Some(failure_type(&*error)),
        }
    }
    Ok(LockedOutcome {
        number,
        result,
        failure,
        diagnostic,
    })
}

fn finalize(run_dir: &Path, stage: &str, mode: &str, outcome: LockedOutcome) -> Result<Value> {
    if outcome.failure.is_some()
        && state::controller_path(run_dir, outcome.number).exists()
        && let Err(error) = state::record_failure(
            run_dir,
            outcome.number,
            stage,
            mode,
            outcome.failure.as_deref().unwrap_or("StageError"),
        )
    {
        return Err(format!(
            "阶段失败（{}），且阶段位置证据保存失败（{}）",
            outcome.failure.as_deref().unwrap_or("StageError"),
            failure_type(&*error)
        )
        .into());
    }
    state::finish(
        run_dir,
        outcome.number,
        outcome.result.as_ref(),
        outcome.failure.as_deref(),
    )?;
    if let Some(failure) = outcome.failure {
        let diagnostic = outcome
            .diagnostic
            .map(|value| format!("；{value}"))
            .unwrap_or_default();
        let guidance = if stage == "copy" {
            format!(
                "；请核对 {} 及 {} 中 copy_directory 指向目录的 failure.json；保留现场，先核对完整前后像，不自动重试",
                run_dir
                    .join(format!("failure-{:04}.json", outcome.number))
                    .display(),
                run_dir.join("manifest.json").display(),
            )
        } else {
            String::new()
        };
        return Err(format!("阶段 {stage} 未完成：{failure}{diagnostic}{guidance}").into());
    }
    let result = outcome.result.ok_or("成功阶段必须发布实际结果")?;
    Ok(json!({
        "status": "stage_finished",
        "stage": stage,
        "mode": mode,
        "attempt": outcome.number,
        "result": result,
        "restore_qualified": false,
    }))
}

fn finish_unstarted_failure(
    run_dir: &Path,
    stage: &str,
    mode: &str,
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    let value = state::load(run_dir, !state::resource_cleanup(stage, mode))?;
    let Some(attempt) = state::running_attempt(&value)? else {
        return Ok(());
    };
    let number = attempt
        .get("number")
        .and_then(Value::as_u64)
        .ok_or("运行中阶段序号无效")?;
    let error_type = failure_type(error);
    if state::controller_path(run_dir, number).exists() {
        let stage = attempt
            .get("stage")
            .and_then(Value::as_str)
            .ok_or("运行中阶段名称无效")?;
        let mode = attempt
            .get("mode")
            .and_then(Value::as_str)
            .ok_or("运行中阶段模式无效")?;
        state::record_failure(run_dir, number, stage, mode, &error_type)?;
    }
    state::finish(run_dir, number, None, Some(&error_type))
}

fn stage_manifest(
    backend: &Path,
    run: &Path,
    stage: &str,
    mode: &str,
) -> Result<manifest::Manifest> {
    let path = run.join("manifest.json");
    if state::resource_cleanup(stage, mode)
        || stage == "seed-runtime" && mode == "source-generation-reboot-close"
    {
        manifest::registered(backend, &path)
    } else {
        manifest::load(backend, &path)
    }
}

pub(crate) fn failure_type(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(category) = marker_diagnostic::category(error) {
        return category.into();
    }
    if let Some((category, _)) = super::super::post_process::diagnostic::diagnostic(error) {
        return category.into();
    }
    if let Some((category, _)) = super::super::cache::handoff::diagnostic(error) {
        return category;
    }
    if let Some((category, _)) = super::super::target_verify::diagnostic(error) {
        return category;
    }
    if let Some((category, _)) = super::super::seed::rebind::diagnostic(error) {
        return category;
    }
    if safe_diagnostic(error).is_some() {
        return "HistoricalProducerCimUnavailable".into();
    }
    let text = error.to_string();
    Regex::new(r"开发复制未完成：([A-Za-z0-9_]+)")
        .ok()
        .and_then(|pattern| {
            pattern
                .captures(&text)
                .and_then(|capture| capture.get(1))
                .map(|value| value.as_str().to_owned())
        })
        .or_else(|| {
            Regex::new(r"开发复制私有协议无效：[^（\r\n]*")
                .ok()
                .filter(|pattern| pattern.is_match(&text))
                .map(|_| "ProtocolError".to_owned())
        })
        .unwrap_or_else(|| "StageError".into())
}

fn safe_diagnostic(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    if let Some(category) = marker_diagnostic::category(error) {
        return Some(format!("Redis 标记恢复前置检查失败（{category}）"));
    }
    if let Some((_, diagnostic)) = super::super::post_process::diagnostic::diagnostic(error) {
        return Some(diagnostic);
    }
    if let Some((_, diagnostic)) = super::super::cache::handoff::diagnostic(error) {
        return Some(diagnostic);
    }
    if let Some((_, diagnostic)) = super::super::target_verify::diagnostic(error) {
        return Some(diagnostic);
    }
    if let Some((_, diagnostic)) = super::super::seed::rebind::diagnostic(error) {
        return Some(diagnostic);
    }
    if let Some(error) =
        error.downcast_ref::<super::super::seed::source_registration::quiet::CimUnavailable>()
    {
        return Some(error.to_string());
    }
    error.source().and_then(safe_diagnostic)
}
