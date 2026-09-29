use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::Stdio,
    thread,
};

use serde_json::Value;

use crate::Result;

use super::{
    BuildContext, StepResult, WaitResult, build_step_interruption, record_cargo_invocation,
    wait_command,
};
use crate::dev::model::{ArtifactAction, BuildPlan};

pub(crate) const DEV_API_FEATURES: &str = "bin-api,runtime-swagger-ui";

#[derive(Clone, Copy)]
pub(super) struct CargoTarget {
    name: &'static str,
    feature: &'static str,
}

pub(super) fn requested_targets(plan: &BuildPlan) -> Vec<CargoTarget> {
    let mut targets = Vec::new();
    if plan.api == ArtifactAction::Rebuild {
        targets.push(CargoTarget {
            name: "ryframe",
            feature: DEV_API_FEATURES,
        });
    }
    if plan.worker == ArtifactAction::Rebuild {
        targets.push(CargoTarget {
            name: "ryframe-worker",
            feature: "bin-worker",
        });
    }
    if plan.migrate {
        targets.push(CargoTarget {
            name: "ryframe-migrate",
            feature: "bin-migrate",
        });
    }
    targets
}

pub(super) fn build_targets(
    context: &BuildContext<'_>,
    plan: &BuildPlan,
    targets: &[CargoTarget],
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<BTreeMap<String, PathBuf>>> {
    let mut artifacts = BTreeMap::new();
    for target in targets {
        if let Some(interruption) = build_step_interruption(context.shutdown, context.watcher, plan)
        {
            return Ok(interruption);
        }
        match build_target(context, plan, *target, lkg_check)? {
            StepResult::Complete(path) => {
                artifacts.insert(target.name.to_owned(), path);
            }
            StepResult::Failed => return Ok(StepResult::Failed),
            StepResult::Superseded => return Ok(StepResult::Superseded),
            StepResult::Cancelled => return Ok(StepResult::Cancelled),
        }
    }
    Ok(StepResult::Complete(artifacts))
}

fn build_target(
    context: &BuildContext<'_>,
    plan: &BuildPlan,
    target: CargoTarget,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<PathBuf>> {
    let mut build = context.cargo_command();
    build
        .args([
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            target.feature,
            "--bin",
            target.name,
            "--message-format=json-render-diagnostics",
        ])
        .current_dir(context.root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    record_cargo_invocation(context.cargo_invocations);
    let mut child = context.group.spawn(build)?;
    let stdout = child
        .take_stdout_reader()?
        .ok_or("Cargo JSON 输出未建立 stdout pipe")?;
    let reader =
        thread::spawn(move || read_cargo_artifacts(stdout).map_err(|error| error.to_string()));
    let wait = wait_command(
        &mut child,
        context.shutdown,
        context.watcher,
        plan,
        lkg_check,
    )?;
    // Cargo 直接父进程退出后仍可能残留持有 JSON 管道的 rustc/build-script。
    // 先关闭该命令自己的进程树，再等待读取线程，避免 supersede 卡在孤儿管道上。
    drop(child);
    let mut artifacts = reader
        .join()
        .map_err(|_| "Cargo JSON 读取线程异常退出")?
        .map_err(|error| format!("无法读取 Cargo JSON 输出：{error}"))?;
    Ok(match wait {
        WaitResult::Complete(status) if status.success() => {
            let executable = artifacts
                .remove(target.name)
                .ok_or_else(|| format!("Cargo 未报告 {} 的 executable 产物", target.name))?;
            StepResult::Complete(executable)
        }
        WaitResult::Complete(_) => StepResult::Failed,
        WaitResult::Superseded => StepResult::Superseded,
        WaitResult::Cancelled => StepResult::Cancelled,
    })
}

fn read_cargo_artifacts(stdout: impl std::io::Read) -> Result<BTreeMap<String, PathBuf>> {
    let mut artifacts = BTreeMap::new();
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            println!("{line}");
            continue;
        };
        match message.get("reason").and_then(Value::as_str) {
            Some("compiler-artifact") => record_artifact(&message, &mut artifacts),
            Some("compiler-message") => render_compiler_message(&message),
            _ => {}
        }
    }
    Ok(artifacts)
}

fn record_artifact(message: &Value, artifacts: &mut BTreeMap<String, PathBuf>) {
    let name = message
        .pointer("/target/name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let executable = message
        .get("executable")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    if let (Some(name), Some(executable)) = (name, executable) {
        artifacts.insert(name, executable);
    }
}

fn render_compiler_message(message: &Value) {
    if let Some(rendered) = message.pointer("/message/rendered").and_then(Value::as_str) {
        eprint!("{rendered}");
    }
}
