use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

use super::super::export;
use super::{
    super::json as json_io,
    lock::{ProcessGuard, RunLock},
    manifest, source, state,
};
use crate::{
    Result,
    cli::CloneStage,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process,
};

pub(crate) struct StageRequest<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) run_dir: &'a Path,
    pub(crate) stage: &'a str,
    pub(crate) mode: &'a str,
    pub(crate) attempt: Option<u64>,
}

pub(crate) trait StageRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()>;
    fn execute(&self, request: &StageRequest<'_>) -> Result<Value>;
}

pub(crate) struct PythonRunner {
    backend: PathBuf,
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

impl PythonRunner {
    pub(crate) fn new(backend: &Path) -> Self {
        Self {
            backend: backend.to_path_buf(),
        }
    }
}

impl StageRunner for PythonRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        let value = invoke(&self.backend, request, "preflight")?;
        if value.get("status").and_then(Value::as_str) != Some("stage_operation_preflight_ready") {
            return Err("阶段预检未返回受控就绪结果".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        invoke(&self.backend, request, "run")
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
    let initial_manifest = manifest::load(backend, &run_dir.join("manifest.json"))?;
    let _ = state::load(run_dir, true)?;
    runner.preflight(&request)?;

    let guard = ProcessGuard::acquire(run_dir)?;
    let run_lock = RunLock::claim(run_dir)?;
    let execution = execute_claimed(
        backend,
        run_dir,
        stage_name,
        mode,
        runner,
        &initial_manifest.value,
        &run_lock,
    );
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
            finish_unstarted_failure(run_dir, &*error)?;
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
    let registered = manifest::load(backend, &run_dir.join("manifest.json"))?;
    if registered.value != *initial_manifest {
        return Err("取得控制锁前验收清单发生变化".into());
    }
    let _ = state::load(run_dir, true)?;
    let sources = source::current(backend)?;
    let (number, attempt) = state::begin(run_dir, stage, mode, &sources)?;
    let mut failure = state::bind_controller(run_dir, number, &run_lock.owner, &attempt)
        .err()
        .map(|error| failure_type(&*error));
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
    }
    match manifest::load(backend, &run_dir.join("manifest.json")) {
        Ok(value) if value.value == *initial_manifest => {}
        Ok(_) => failure = Some("ManifestChanged".into()),
        Err(error) => failure = Some(failure_type(&*error)),
    }
    match source::current(backend) {
        Ok(value) if value == sources => {}
        Ok(_) => failure = Some("SourceChanged".into()),
        Err(error) => failure = Some(failure_type(&*error)),
    }
    Ok(LockedOutcome {
        number,
        result,
        failure,
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
        return Err(format!("阶段 {stage} 未完成：{failure}").into());
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
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    let value = state::load(run_dir, true)?;
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

pub(crate) fn invoke(backend: &Path, request: &StageRequest<'_>, operation: &str) -> Result<Value> {
    let payload = protocol(request, operation)?;
    let output = process::command_output_with_input(
        backend,
        "python",
        &["-B", "tools/python/devex_clone.py", "--private-stdin"],
        payload.as_bytes(),
    )
    .map_err(|error| -> String { format!("阶段资源编排子进程失败：{}", error) })?;
    let value = json_io::parse_fragment(output.trim(), "阶段资源编排输出")?;
    if !value.is_object() {
        return Err("阶段资源编排输出必须为对象".into());
    }
    Ok(value)
}

pub(crate) fn protocol(request: &StageRequest<'_>, operation: &str) -> Result<String> {
    let attempt = request.attempt;
    if !matches!(operation, "preflight" | "target-authority" | "run")
        || (operation == "target-authority"
            && (request.stage != "target-verify" || request.mode != "run"))
        || (operation == "run") != attempt.is_some()
        || attempt == Some(0)
    {
        return Err("阶段资源编排 operation 与 attempt 不匹配".into());
    }
    let value = json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-clone-stage",
        "request": {
            "backend_dir": path_text(request.backend, "后端目录")?,
            "run_dir": path_text(request.run_dir, "运行目录")?,
            "stage": request.stage,
            "mode": request.mode,
            "operation": operation,
            "attempt": attempt,
            "write": operation == "run",
        },
    });
    let payload = serde_json::to_string(&value)?;
    if payload.len() > 32 * 1024 || payload.contains(['\r', '\n', '\0']) {
        return Err("阶段资源编排私有协议过长或包含换行/NUL".into());
    }
    Ok(payload)
}

pub(crate) fn failure_type(error: &(dyn std::error::Error + 'static)) -> String {
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

fn path_text<'a>(path: &'a Path, label: &str) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| format!("{label}必须为 UTF-8").into())
}
