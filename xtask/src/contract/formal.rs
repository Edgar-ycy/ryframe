use std::{fs, path::Path};

use crate::{
    Result,
    process::{command_output, run_with_env},
    workspace::root_dir,
};

use super::{
    candidate::{
        artifact_paths_from_inputs, desired_frontend_snapshots, prepare_staging_frontend,
        snapshot_managed_files, snapshot_staging_inputs, validate_candidate_contract,
        verify_input_snapshots,
    },
    model::{CANDIDATE_MARKER, ContractLock, sha256_hex},
    ownership::contract_managed_paths,
    recovery::reject_contract_recovery_artifacts,
    transaction::install_snapshots,
};

pub(crate) const FORMAL_SYNC_ARGS: &[&[&str]] = &[
    &["scripts/sync-api-contract.mjs"],
    &["scripts/generate-api-artifacts.mjs", "--write"],
];

pub(super) fn sync_commit(reference: &str, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let object = format!("{reference}^{{commit}}");
    let commit = command_output(&root, "git", &["rev-parse", "--verify", &object])?
        .trim()
        .to_owned();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("Git 引用 {reference} 未解析为完整提交 SHA").into());
    }
    let repository = github_repository_identifier(env!("CARGO_PKG_REPOSITORY"))?;
    let worktree = root
        .to_str()
        .ok_or("后端工作区路径不是有效 UTF-8，无法同步契约")?;
    let environment = [
        ("RYFRAME_BACKEND_REPOSITORY", repository.as_str()),
        ("RYFRAME_BACKEND_COMMIT", commit.as_str()),
        ("RYFRAME_BACKEND_WORKTREE", worktree),
        ("RYFRAME_OPENAPI_PATH", "openapi/openapi.json"),
    ];
    let _lock = ContractLock::acquire(frontend_dir)?;
    let inputs = snapshot_staging_inputs(frontend_dir)?;
    let artifact_paths = artifact_paths_from_inputs(&inputs, frontend_dir)?;
    let managed_paths = contract_managed_paths(frontend_dir, &artifact_paths)?;
    reject_contract_recovery_artifacts(&managed_paths)?;
    let before = snapshot_managed_files(&managed_paths)?;
    let staging = prepare_staging_frontend(frontend_dir, &before, &inputs)?;
    for args in FORMAL_SYNC_ARGS {
        run_with_env(&staging.path, "node", args, &environment)?;
    }
    validate_formal_sync(&staging.path, &repository, &commit, &artifact_paths)?;
    verify_input_snapshots(&inputs)?;
    let desired = desired_frontend_snapshots(&before, frontend_dir, &staging.path)?;
    install_snapshots(&before, &desired)
}

pub(crate) fn validate_formal_sync(
    frontend_dir: &Path,
    repository: &str,
    commit: &str,
    artifact_paths: &[String],
) -> Result<()> {
    if frontend_dir.join(CANDIDATE_MARKER).exists() {
        return Err("正式契约同步完成后仍存在 openapi/candidate.json".into());
    }
    let metadata_path = frontend_dir.join("openapi/source.json");
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(&metadata_path)?)
        .map_err(|error| format!("正式契约来源元数据不是有效 JSON：{error}"))?;
    if metadata
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || metadata
            .get("backend_repository")
            .and_then(serde_json::Value::as_str)
            != Some(repository)
        || metadata
            .get("backend_commit")
            .and_then(serde_json::Value::as_str)
            != Some(commit)
        || metadata
            .get("openapi_path")
            .and_then(serde_json::Value::as_str)
            != Some("openapi/openapi.json")
    {
        return Err("正式契约来源元数据未固定到请求的后端提交".into());
    }
    for relative in artifact_paths {
        let path = frontend_dir.join(relative);
        if !path.is_file() {
            return Err(format!("正式契约缺少派生文件：{}", path.display()).into());
        }
    }
    let openapi = fs::read(frontend_dir.join("openapi/openapi.json"))?;
    validate_candidate_contract(&openapi)?;
    let document: serde_json::Value = serde_json::from_slice(&openapi)?;
    if metadata
        .get("openapi_version")
        .and_then(serde_json::Value::as_str)
        != document.get("openapi").and_then(serde_json::Value::as_str)
        || metadata.get("sha256").and_then(serde_json::Value::as_str)
            != Some(sha256_hex(&openapi).as_str())
    {
        return Err("正式契约来源元数据与 openapi/openapi.json 内容不一致".into());
    }
    Ok(())
}

pub(crate) fn github_repository_identifier(repository: &str) -> Result<String> {
    let path = repository
        .strip_prefix("https://github.com/")
        .or_else(|| repository.strip_prefix("http://github.com/"))
        .or_else(|| repository.strip_prefix("git@github.com:"))
        .ok_or("Cargo repository 必须指向 GitHub 仓库")?
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() != 2
        || segments
            .iter()
            .any(|segment| segment.is_empty() || !is_repository_segment(segment))
    {
        return Err("Cargo repository 不是有效的 GitHub owner/repository".into());
    }
    Ok(path.to_owned())
}

fn is_repository_segment(segment: &str) -> bool {
    segment
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}
