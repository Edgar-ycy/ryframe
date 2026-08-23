use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    Result,
    process::{command_output, run_pnpm},
    workspace::root_dir,
};

use super::{
    execution::{BACKEND_VERIFY_TARGET_DIR, run_owned},
    model::{BackendSnapshotProfile, ConsumerContractPlan},
};

static NEXT_VERIFY_ARTIFACT: AtomicU64 = AtomicU64::new(1);

pub(super) struct BackendSnapshots {
    openapi: Option<PathBuf>,
    mysql: Option<PathBuf>,
}

impl Drop for BackendSnapshots {
    fn drop(&mut self) {
        if let Some(path) = &self.openapi {
            let _ = fs::remove_file(path);
        }
        if let Some(path) = &self.mysql {
            let _ = fs::remove_file(path);
        }
    }
}

pub(super) fn export_and_verify_backend_snapshots(
    profiles: &BTreeSet<BackendSnapshotProfile>,
) -> Result<BackendSnapshots> {
    let root = root_dir();
    let artifact_dir = root.join("target").join("xtask");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "无法创建完整门禁临时目录 {}：{error}",
            artifact_dir.display()
        )
    })?;
    let suffix = format!(
        "{}-{}",
        process::id(),
        NEXT_VERIFY_ARTIFACT.fetch_add(1, Ordering::Relaxed)
    );
    let openapi = profiles
        .contains(&BackendSnapshotProfile::OpenApiContract)
        .then(|| artifact_dir.join(format!("verify-{suffix}-openapi.json")));
    let mysql = profiles
        .contains(&BackendSnapshotProfile::Mysql)
        .then(|| artifact_dir.join(format!("verify-{suffix}-mysql.sql")));
    let snapshots = BackendSnapshots { openapi, mysql };

    if let Some(openapi) = &snapshots.openapi {
        run_owned(
            &root,
            "cargo",
            &backend_snapshot_export_args("ryframe-api", "export_openapi", openapi),
        )?;
        verify_snapshot(
            "OpenAPI",
            &root.join("openapi").join("openapi.json"),
            openapi,
            "cargo api-sync",
        )?;
    }
    if let Some(mysql) = &snapshots.mysql {
        run_owned(
            &root,
            "cargo",
            &backend_snapshot_export_args("ryframe-db", "export_mysql_snapshot", mysql),
        )?;
        verify_snapshot(
            "MySQL 基线",
            &root.join("sql").join("ryframe_config.sql"),
            mysql,
            "cargo run --locked -p ryframe-db --bin export_mysql_snapshot -- sql/ryframe_config.sql",
        )?;
    }
    Ok(snapshots)
}

pub(crate) fn backend_snapshot_export_args(
    package: &str,
    binary: &str,
    output: &Path,
) -> Vec<String> {
    vec![
        "run".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        BACKEND_VERIFY_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
        "--bin".to_owned(),
        binary.to_owned(),
        "--".to_owned(),
        output.to_string_lossy().into_owned(),
    ]
}

fn verify_snapshot(
    label: &str,
    committed_path: &Path,
    generated_path: &Path,
    refresh_command: &str,
) -> Result<()> {
    let committed = fs::read(committed_path).map_err(|error| {
        format!(
            "无法读取已提交的{label}快照 {}：{error}",
            committed_path.display()
        )
    })?;
    let generated = fs::read(generated_path).map_err(|error| {
        format!(
            "无法读取本次生成的{label}快照 {}：{error}",
            generated_path.display()
        )
    })?;
    if committed != generated {
        return Err(format!(
            "{label}快照已过期：{} 与当前代码生成结果不同；确认变更后运行 `{refresh_command}`",
            committed_path.display()
        )
        .into());
    }
    println!("{label}快照与当前代码一致。");
    Ok(())
}

pub(super) fn run_consumer_contract(
    frontend_dir: &Path,
    backend_snapshots: &BackendSnapshots,
) -> Result<()> {
    let openapi = backend_snapshots
        .openapi
        .as_deref()
        .ok_or("消费契约检查缺少本次生成的 OpenAPI 快照")?;
    let candidate = frontend_dir.join("openapi/candidate.json").is_file();
    let candidate_commit = if candidate {
        Some(
            command_output(&root_dir(), "git", &["rev-parse", "HEAD"])?
                .trim()
                .to_owned(),
        )
    } else {
        None
    };
    let plan = load_consumer_contract_plan(frontend_dir, candidate_commit.as_deref())?;
    let arguments = consumer_contract_arguments(&plan, openapi);
    let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    run_pnpm(frontend_dir, &arguments)
}

pub(crate) fn load_consumer_contract_plan(
    frontend_dir: &Path,
    candidate_commit: Option<&str>,
) -> Result<ConsumerContractPlan> {
    let source_path = frontend_dir.join("openapi").join("source.json");
    let source: serde_json::Value = serde_json::from_slice(
        &fs::read(&source_path)
            .map_err(|error| format!("无法读取契约来源 {}：{error}", source_path.display()))?,
    )
    .map_err(|error| format!("契约来源 {} 不是有效 JSON：{error}", source_path.display()))?;
    let candidate = frontend_dir.join("openapi/candidate.json").is_file();
    consumer_contract_plan(&source, candidate, candidate_commit)
}

pub(crate) fn consumer_contract_plan(
    source: &serde_json::Value,
    candidate: bool,
    candidate_commit: Option<&str>,
) -> Result<ConsumerContractPlan> {
    let backend_repository = source
        .get("backend_repository")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("契约来源缺少 backend_repository")?
        .to_owned();
    let backend_commit = if candidate {
        candidate_commit
            .filter(|value| !value.trim().is_empty())
            .ok_or("候选契约检查缺少当前后端提交")?
            .to_owned()
    } else {
        source
            .get("backend_commit")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("正式契约来源缺少 backend_commit")?
            .to_owned()
    };
    Ok(ConsumerContractPlan {
        mode: if candidate { "candidate" } else { "formal" },
        backend_commit,
        backend_repository,
        require_pin: !candidate,
    })
}

pub(crate) fn consumer_contract_arguments(
    plan: &ConsumerContractPlan,
    openapi: &Path,
) -> Vec<String> {
    vec![
        "consumer:check".to_owned(),
        "--".to_owned(),
        "--mode".to_owned(),
        plan.mode.to_owned(),
        "--openapi".to_owned(),
        openapi.to_string_lossy().into_owned(),
        "--backend-commit".to_owned(),
        plan.backend_commit.clone(),
        "--backend-repository".to_owned(),
        plan.backend_repository.clone(),
        "--require-pin".to_owned(),
        plan.require_pin.to_string(),
    ]
}
