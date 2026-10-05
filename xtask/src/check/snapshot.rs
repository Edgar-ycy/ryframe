use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    Result,
    process::{command_output, run_owned, run_pnpm_with_env},
};

use super::model::{BackendSnapshotProfile, ConsumerContractPlan};

pub(crate) struct BackendSnapshots {
    openapi: Option<PathBuf>,
    mysql: Option<PathBuf>,
}

impl BackendSnapshots {
    pub(crate) fn workspace_test_environment(&self) -> Vec<(&'static str, String)> {
        let mut environment = Vec::with_capacity(2);
        if let Some(openapi) = &self.openapi {
            environment.push((
                "RYFRAME_VERIFY_OPENAPI_SNAPSHOT_OUTPUT",
                snapshot_environment_path(openapi),
            ));
        }
        if let Some(mysql) = &self.mysql {
            environment.push((
                "RYFRAME_VERIFY_MYSQL_SNAPSHOT_OUTPUT",
                snapshot_environment_path(mysql),
            ));
        }
        environment
    }
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
    root: &Path,
    profiles: &BTreeSet<BackendSnapshotProfile>,
    target_dir: &str,
) -> Result<BackendSnapshots> {
    let snapshots = prepare_backend_snapshots(root, profiles)?;

    if let Some(openapi) = &snapshots.openapi {
        run_owned(
            root,
            "cargo",
            &backend_snapshot_export_args(
                target_dir,
                "ryframe",
                "export_openapi",
                openapi,
            ),
        )?;
    }
    if let Some(mysql) = &snapshots.mysql {
        run_owned(
            root,
            "cargo",
            &backend_snapshot_export_args(target_dir, "ryframe-db", "export_mysql_snapshot", mysql),
        )?;
    }
    verify_backend_snapshots(root, &snapshots)?;
    Ok(snapshots)
}

pub(crate) fn prepare_backend_snapshots(
    root: &Path,
    profiles: &BTreeSet<BackendSnapshotProfile>,
) -> Result<BackendSnapshots> {
    prepare_backend_snapshots_with_prefix(root, profiles, "verify")
}

pub(crate) fn prepare_consumer_backend_snapshots(
    root: &Path,
    profiles: &BTreeSet<BackendSnapshotProfile>,
) -> Result<BackendSnapshots> {
    prepare_backend_snapshots_with_prefix(root, profiles, "consumer")
}

fn prepare_backend_snapshots_with_prefix(
    root: &Path,
    profiles: &BTreeSet<BackendSnapshotProfile>,
    prefix: &str,
) -> Result<BackendSnapshots> {
    let artifact_dir = root.join("target").join("xtask");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "无法创建完整门禁临时目录 {}：{error}",
            artifact_dir.display()
        )
    })?;
    let openapi = profiles
        .contains(&BackendSnapshotProfile::OpenApiContract)
        .then(|| artifact_dir.join(format!("{prefix}-openapi.json")));
    let mysql = profiles
        .contains(&BackendSnapshotProfile::Mysql)
        .then(|| artifact_dir.join(format!("{prefix}-mysql.sql")));
    Ok(BackendSnapshots { openapi, mysql })
}

fn snapshot_environment_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn stage_committed_backend_snapshots(
    root: &Path,
    snapshots: &BackendSnapshots,
) -> Result<()> {
    if let Some(openapi) = &snapshots.openapi {
        copy_committed_snapshot(&root.join("openapi/openapi.json"), openapi, "OpenAPI")?;
    }
    if let Some(mysql) = &snapshots.mysql {
        copy_committed_snapshot(&root.join("sql/ryframe_config.sql"), mysql, "MySQL 基线")?;
    }
    Ok(())
}

fn copy_committed_snapshot(source: &Path, destination: &Path, label: &str) -> Result<()> {
    fs::copy(source, destination).map_err(|error| {
        format!(
            "无法暂存已提交的{label}快照 {} -> {}：{error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

pub(crate) fn verify_backend_snapshots(root: &Path, snapshots: &BackendSnapshots) -> Result<()> {
    if let Some(openapi) = &snapshots.openapi {
        verify_snapshot(
            "OpenAPI",
            &root.join("openapi").join("openapi.json"),
            openapi,
            "cargo xtask generate api --write",
        )?;
    }
    if let Some(mysql) = &snapshots.mysql {
        verify_snapshot(
            "MySQL 基线",
            &root.join("sql").join("ryframe_config.sql"),
            mysql,
            "cargo run --locked -p ryframe-db --features migration --bin export_mysql_snapshot -- sql/ryframe_config.sql",
        )?;
    }
    Ok(())
}

pub(crate) fn backend_snapshot_export_args(
    target_dir: &str,
    package: &str,
    binary: &str,
    output: &Path,
) -> Vec<String> {
    let mut args = vec![
        "--config".to_owned(),
        "profile.dev.debug=0".to_owned(),
        "run".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
    ];
    if package == "ryframe-db" && binary == "export_mysql_snapshot" {
        args.extend(["--features".to_owned(), "migration".to_owned()]);
    }
    args.extend([
        "--bin".to_owned(),
        binary.to_owned(),
        "--".to_owned(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

/// 只有实际运行对应 package 的集成测试时，测试环境变量才能生成所需快照。
pub(crate) fn package_tests_generate_snapshots(
    profiles: &BTreeSet<BackendSnapshotProfile>,
    packages: &BTreeSet<String>,
) -> bool {
    profiles.iter().all(|profile| {
        let producer = match profile {
            BackendSnapshotProfile::OpenApiContract => "ryframe-api",
            BackendSnapshotProfile::Mysql => "ryframe-db",
        };
        packages.contains(producer)
    })
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
    backend_root: &Path,
    frontend_dir: &Path,
    backend_snapshots: &BackendSnapshots,
    full: bool,
) -> Result<()> {
    let openapi = backend_snapshots
        .openapi
        .as_deref()
        .ok_or("消费契约检查缺少本次生成的 OpenAPI 快照")?;
    let candidate = frontend_dir.join("openapi/candidate.json").is_file();
    let candidate_commit = if candidate {
        Some(
            command_output(backend_root, "git", &["rev-parse", "HEAD"])?
                .trim()
                .to_owned(),
        )
    } else {
        None
    };
    let plan = load_consumer_contract_plan(frontend_dir, candidate_commit.as_deref())?;
    let arguments = consumer_contract_arguments(&plan, openapi);
    let context = serde_json::to_string(&arguments)?;
    let source_domain_checker = backend_root.join("tools/python/source_domain_contract.py");
    let source_domain_checker = source_domain_checker
        .to_str()
        .ok_or("来源分域检查器路径不是有效 UTF-8")?;
    let python = std::env::var("RYFRAME_PYTHON")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "python".to_owned());
    run_pnpm_with_env(
        frontend_dir,
        consumer_contract_command(full),
        &[
            ("RYFRAME_CONSUMER_CONTRACT", context.as_str()),
            ("RYFRAME_BACKEND_SOURCE_DOMAIN_CHECK", source_domain_checker),
            ("RYFRAME_PYTHON", python.as_str()),
        ],
    )
}

pub(crate) fn consumer_contract_command(full: bool) -> &'static [&'static str] {
    if full {
        &["check", "--full"]
    } else {
        &["check", "--stage", "contract"]
    }
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
