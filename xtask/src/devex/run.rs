use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::Instant,
};

use chrono::Utc;

use crate::Result;

use super::{
    metadata::{
        MetadataContext, PathNormalizer, collect as collect_metadata, corepack_executable,
        inherited_environment,
    },
    model::{
        CacheState, DevexRunOptions, StepDefinition, SuiteDefinition, SuiteRequirement,
        WorkingDirectory,
    },
    report::{SampleKind, SampleRecord, SampleStatus, append_sample, summarize, write_metadata},
};

pub(super) fn execute(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
) -> Result<PathBuf> {
    let definition = options.suite.definition();
    preflight(backend_root, frontend_root, options, definition)?;
    let devex_root = backend_root.join(".local-tests/devex");
    let (run_id, run_dir) = create_run_directory(&devex_root, options)?;
    let mut environment = effective_environment(definition);
    if definition.steps.iter().any(|step| step.program == "cargo") {
        environment.insert("CARGO_TARGET_DIR".to_owned(), "{target}".to_owned());
    }
    let metadata = collect_metadata(MetadataContext {
        backend_root,
        frontend_root,
        devex_root: &devex_root,
        run_id: &run_id,
        options,
        definition,
        effective_environment: &environment,
    })?;
    write_metadata(&run_dir, &metadata)?;
    fs::write(run_dir.join("samples.jsonl"), [])?;
    let normalizer = PathNormalizer::new(backend_root, frontend_root, &devex_root);

    if options.cache_state == CacheState::Warm {
        let target = run_dir.join("cache/warm");
        let outcome = execute_sample(backend_root, frontend_root, &target, definition, "warmup")?;
        append_sample(
            &run_dir,
            &sample_record(
                &run_id,
                0,
                SampleKind::Warmup,
                options.cache_state,
                &target,
                outcome,
                &normalizer,
            ),
        )?;
        if !outcome.status.success() {
            summarize(&run_dir)?;
            return Err(format!("suite `{}` 预热失败", options.suite.as_str()).into());
        }
    }

    for sequence in 1..=options.runs {
        let target = sample_target(&run_dir, options.cache_state, sequence);
        let label = format!("sample-{sequence:03}");
        println!(
            "DevEx {} {}/{}（{}）",
            options.suite.as_str(),
            sequence,
            options.runs,
            options.cache_state.as_str()
        );
        let outcome = execute_sample(backend_root, frontend_root, &target, definition, &label)?;
        append_sample(
            &run_dir,
            &sample_record(
                &run_id,
                sequence,
                SampleKind::Measurement,
                options.cache_state,
                &target,
                outcome,
                &normalizer,
            ),
        )?;
        if !outcome.status.success() {
            summarize(&run_dir)?;
            return Err(format!(
                "suite `{}` 第 {sequence} 个样本失败，记录保留在 {}",
                options.suite.as_str(),
                normalizer.normalize(&run_dir.to_string_lossy())
            )
            .into());
        }
    }
    let summary = summarize(&run_dir)?;
    println!(
        "DevEx 完成：{}（P50 {}，P95 {}）",
        normalizer.normalize(&run_dir.to_string_lossy()),
        metric(summary.duration_ms.as_ref().map(|value| value.p50)),
        metric(summary.duration_ms.as_ref().map(|value| value.p95)),
    );
    Ok(run_dir)
}

#[derive(Debug, Clone, Copy)]
struct SampleOutcome {
    started_at: chrono::DateTime<Utc>,
    duration_ms: f64,
    status: ExitStatus,
}

fn execute_sample(
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    label: &str,
) -> Result<SampleOutcome> {
    fs::create_dir_all(target)?;
    let started_at = Utc::now();
    let started = Instant::now();
    let mut last_status = success_status()?;
    for (index, step) in definition.steps.iter().enumerate() {
        println!("  → {label} step {:02}: {}", index + 1, display_step(step));
        last_status =
            step_command(step, backend_root, frontend_root, target, definition).status()?;
        if !last_status.success() {
            break;
        }
    }
    Ok(SampleOutcome {
        started_at,
        duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
        status: last_status,
    })
}

fn step_command(
    step: &StepDefinition,
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
) -> Command {
    let program = if step.program == "corepack" {
        corepack_executable()
    } else {
        step.program
    };
    let mut command = Command::new(program);
    let target = target.to_string_lossy();
    let frontend = frontend_root.to_string_lossy();
    let args = step
        .args
        .iter()
        .map(|arg| {
            arg.replace("{target}", &target)
                .replace("{frontend}", &frontend)
        })
        .collect::<Vec<_>>();
    command
        .args(args)
        .current_dir(match step.working_directory {
            WorkingDirectory::Backend => backend_root,
            WorkingDirectory::Frontend => frontend_root,
        })
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for key in definition.remove_environment {
        command.env_remove(key);
    }
    command.envs(definition.environment.iter().copied());
    if step.program == "cargo" {
        command.env("CARGO_TARGET_DIR", target.as_ref());
    }
    command
}

fn sample_record(
    run_id: &str,
    sequence: usize,
    kind: SampleKind,
    cache_state: CacheState,
    target: &Path,
    outcome: SampleOutcome,
    normalizer: &PathNormalizer,
) -> SampleRecord {
    SampleRecord {
        schema_version: 1,
        run_id: run_id.to_owned(),
        sequence,
        kind,
        cache_state,
        started_at: outcome.started_at.to_rfc3339(),
        duration_ms: outcome.duration_ms,
        status: if outcome.status.success() {
            SampleStatus::Passed
        } else {
            SampleStatus::Failed
        },
        exit_code: outcome.status.code(),
        target_directory: normalizer.normalize(&target.to_string_lossy()),
    }
}

fn preflight(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
) -> Result<()> {
    match definition.requirement {
        SuiteRequirement::Ready => executable_available(backend_root, "cargo", &["--version"]),
        SuiteRequirement::Executable(executable) => {
            executable_available(backend_root, executable, &["--version"])
        }
        SuiteRequirement::Frontend => {
            if !frontend_root.join("package.json").is_file() {
                return Err(format!(
                    "suite `{}` 需要前端目录 {}",
                    options.suite.as_str(),
                    frontend_root.display()
                )
                .into());
            }
            if !frontend_root.join("node_modules").is_dir() {
                return Err(format!(
                    "suite `{}` 需要已安装的前端依赖；请先在 {} 运行 pnpm install --frozen-lockfile",
                    options.suite.as_str(),
                    frontend_root.display()
                )
                .into());
            }
            executable_available(frontend_root, "node", &["--version"])?;
            executable_available(frontend_root, corepack_executable(), &["pnpm", "--version"])
        }
        SuiteRequirement::FrontendFiles => {
            if !frontend_root.join("package.json").is_file() {
                return Err(format!(
                    "suite `{}` 需要前端工作区 {}",
                    options.suite.as_str(),
                    frontend_root.display()
                )
                .into());
            }
            executable_available(backend_root, "cargo", &["--version"])
        }
        SuiteRequirement::Pending(reason) => Err(format!(
            "suite `{}` 暂不可执行：{reason}\n固定命令映射：{}",
            options.suite.as_str(),
            definition
                .steps
                .iter()
                .map(display_step)
                .collect::<Vec<_>>()
                .join(" && ")
        )
        .into()),
    }
}

fn executable_available(root: &Path, executable: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(executable)
        .args(args)
        .current_dir(root)
        .output();
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "DevEx 前置检查失败：`{executable} {}` 返回 {}",
            args.join(" "),
            output.status
        )
        .into()),
        Err(error) => Err(format!("DevEx 前置检查找不到 `{executable}`：{error}").into()),
    }
}

fn effective_environment(definition: SuiteDefinition) -> BTreeMap<String, String> {
    let mut environment = inherited_environment();
    for key in definition.remove_environment {
        environment.remove(*key);
    }
    environment.extend(
        definition
            .environment
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    environment
}

fn create_run_directory(devex_root: &Path, options: &DevexRunOptions) -> Result<(String, PathBuf)> {
    let now = Utc::now();
    let date = now.format("%Y-%m-%d").to_string();
    let run_id = format!(
        "{}-{}-{}-{}-{}",
        now.format("%Y%m%dT%H%M%S%9fZ"),
        options.suite.as_str(),
        options.variant,
        options.cache_state.as_str(),
        std::process::id()
    );
    let directory = devex_root.join(date).join(&run_id);
    fs::create_dir_all(directory.parent().expect("run 目录必须具有日期父目录"))?;
    fs::create_dir(&directory)?;
    Ok((run_id, directory))
}

fn sample_target(run_dir: &Path, cache_state: CacheState, sequence: usize) -> PathBuf {
    match cache_state {
        CacheState::Cold => run_dir.join(format!("cache/cold-{sequence:03}")),
        CacheState::Warm => run_dir.join("cache/warm"),
    }
}

fn display_step(step: &StepDefinition) -> String {
    format!("{} {}", step.program, step.args.join(" "))
}

fn metric(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.1} ms"))
}

#[cfg(windows)]
fn success_status() -> std::io::Result<ExitStatus> {
    Command::new("cmd").args(["/C", "exit", "0"]).status()
}

#[cfg(not(windows))]
fn success_status() -> std::io::Result<ExitStatus> {
    Command::new("true").status()
}
