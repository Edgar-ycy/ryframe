use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use crate::{Result, cli::ApiGenerateCommand, process::run as run_process, workspace::root_dir};

use super::{
    atomic::write_atomically,
    formal::sync_commit,
    model::{
        CANDIDATE_MARKER, CRUD_RESOURCE_ARTIFACT, ContractLock, Snapshot, StagingFrontend, nonce,
        sha256_hex,
    },
    ownership::contract_managed_paths,
    recovery::reject_contract_recovery_artifacts,
    transaction::install_snapshots,
};

pub(crate) const CANDIDATE_GENERATION_ARGS: &[&str] =
    &["scripts/generate-api-artifacts.mjs", "--write"];

pub(crate) fn generate_api(command: &ApiGenerateCommand, frontend_dir: &Path) -> Result<()> {
    match (&command.reference, command.write) {
        (_, false) => super::readonly::check_current(frontend_dir),
        (None, true) => sync_candidate(frontend_dir),
        (Some(reference), true) => sync_commit(reference, frontend_dir),
    }
}

fn sync_candidate(frontend_dir: &Path) -> Result<()> {
    if !frontend_dir.is_dir() {
        return Err(format!("前端目录不存在：{}", frontend_dir.display()).into());
    }
    let root = root_dir();
    let temporary_dir = root.join("target/xtask");
    fs::create_dir_all(&temporary_dir)?;
    let candidate_path = temporary_dir.join(format!(
        "candidate-openapi-{}-{}.json",
        process::id(),
        nonce()?
    ));
    if candidate_path.exists() {
        return Err(format!("候选契约暂存路径冲突：{}", candidate_path.display()).into());
    }
    let candidate = candidate_path
        .to_str()
        .ok_or("候选 OpenAPI 路径不是有效 UTF-8")?;
    let target_dir = root.join("target/xtask-contract");
    let target_dir = target_dir
        .to_str()
        .ok_or("契约导出 target 路径不是有效 UTF-8")?;
    let export_result = run_process(
        &root,
        "cargo",
        &[
            "run",
            "--locked",
            "--target-dir",
            target_dir,
            "-p",
            "ryframe-api",
            "--bin",
            "export_openapi",
            "--",
            candidate,
        ],
    );
    if let Err(error) = export_result {
        let _ = fs::remove_file(&candidate_path);
        return Err(error);
    }

    let result = (|| {
        let bytes = canonical_contract(&fs::read(&candidate_path)?)?;
        apply_candidate(&root, frontend_dir, &bytes, |staging| {
            run_process(staging, "node", CANDIDATE_GENERATION_ARGS)
        })
    })();
    let cleanup_result = fs::remove_file(&candidate_path);
    match (result, cleanup_result) {
        (Ok(()), Ok(())) => {
            println!(
                "已同步当前后端工作树的候选 OpenAPI 与前端派生文件；openapi/source.json 未改动。"
            );
            Ok(())
        }
        (Ok(()), Err(error)) => Err(format!(
            "候选契约已同步，但暂存文件 {} 清理失败：{error}",
            candidate_path.display()
        )
        .into()),
        (Err(error), _) => Err(error),
    }
}

pub(crate) fn validate_candidate_contract(bytes: &[u8]) -> Result<()> {
    let document: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("后端导出的候选 OpenAPI 不是有效 JSON：{error}"))?;
    let openapi = document
        .get("openapi")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let title = document
        .pointer("/info/title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !openapi.starts_with("3.") || title != "RyFrame API" {
        return Err("后端导出的文件不是受支持的 RyFrame OpenAPI 3 契约".into());
    }
    Ok(())
}

pub(super) fn canonical_contract(bytes: &[u8]) -> Result<Vec<u8>> {
    validate_candidate_contract(bytes)?;
    let document: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut canonical = serde_json::to_string_pretty(&document)?.into_bytes();
    canonical.push(b'\n');
    Ok(canonical)
}

pub(crate) fn apply_candidate<F>(
    backend_dir: &Path,
    frontend_dir: &Path,
    candidate: &[u8],
    generate: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    apply_candidate_with_staging_hook(backend_dir, frontend_dir, candidate, |_| Ok(()), generate)
}

pub(crate) fn apply_candidate_with_staging_hook<F, H>(
    backend_dir: &Path,
    frontend_dir: &Path,
    candidate: &[u8],
    after_staging: H,
    generate: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
    H: FnOnce(&Path) -> Result<()>,
{
    let _lock = ContractLock::acquire(frontend_dir)?;
    let candidate = canonical_contract(candidate)?;
    let has_crud_resources = serde_json::from_slice::<serde_json::Value>(&candidate)?
        .get("x-ryframe-crud-resources")
        .is_some();
    let inputs = snapshot_staging_inputs(frontend_dir)?;
    let artifact_paths = artifact_paths_from_inputs(&inputs, frontend_dir)?;
    let mut managed_paths = contract_managed_paths(frontend_dir, &artifact_paths)?;
    managed_paths.insert(0, backend_dir.join("openapi/openapi.json"));
    reject_contract_recovery_artifacts(&managed_paths)?;
    let before = snapshot_managed_files(&managed_paths)?;
    let metadata_path = frontend_dir.join("openapi/source.json");
    let metadata_before = before
        .iter()
        .find(|snapshot| snapshot.path == metadata_path)
        .and_then(|snapshot| snapshot.content.as_deref())
        .ok_or_else(|| {
            format!(
                "无法从同步快照读取正式契约来源元数据：{}",
                metadata_path.display()
            )
        })?;
    let staging = prepare_staging_frontend(frontend_dir, &before, &inputs)?;
    after_staging(&staging.path)?;
    let openapi_path = staging.path.join("openapi/openapi.json");
    let marker_path = staging.path.join(CANDIDATE_MARKER);
    let marker = candidate_marker(&candidate, metadata_before)?;
    (|| {
        write_atomically(&openapi_path, &candidate)?;
        write_atomically(&marker_path, &marker)?;
        generate(&staging.path)?;
        let metadata_after = fs::read(staging.path.join("openapi/source.json"))?;
        if metadata_after != metadata_before {
            return Err("候选契约生成过程修改了 openapi/source.json，已拒绝该结果".into());
        }
        for relative in &artifact_paths {
            let path = staging.path.join(relative);
            if !path.is_file() {
                return Err(format!("候选契约缺少派生文件：{}", path.display()).into());
            }
        }
        if has_crud_resources
            && (!artifact_paths
                .iter()
                .any(|path| path == CRUD_RESOURCE_ARTIFACT)
                || !staging.path.join(CRUD_RESOURCE_ARTIFACT).is_file())
        {
            return Err(format!(
                "候选契约包含 CRUD 资源扩展，但缺少派生文件：{CRUD_RESOURCE_ARTIFACT}"
            )
            .into());
        }
        verify_input_snapshots(&inputs)?;
        let desired = desired_candidate_snapshots(
            &before,
            backend_dir,
            frontend_dir,
            &staging.path,
            &candidate,
        )?;
        install_snapshots(&before, &desired)
    })()
}

fn candidate_marker(candidate: &[u8], formal_source: &[u8]) -> Result<Vec<u8>> {
    let document: serde_json::Value = serde_json::from_slice(candidate)?;
    let marker = serde_json::json!({
        "schema_version": 1,
        "mode": "candidate",
        "openapi_version": document
            .get("openapi")
            .and_then(serde_json::Value::as_str)
            .ok_or("候选契约缺少 OpenAPI 版本")?,
        "candidate_sha256": sha256_hex(candidate),
        "formal_source_sha256": sha256_hex(formal_source),
    });
    let mut bytes = serde_json::to_string_pretty(&marker)?.into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}

#[allow(dead_code)]
pub(crate) fn generated_artifact_paths(frontend_dir: &Path) -> Result<Vec<String>> {
    let manifest = frontend_dir.join("scripts/api-artifacts.mjs");
    let source = fs::read(&manifest).map_err(|error| {
        format!(
            "无法读取前端 OpenAPI 派生文件清单 {}：{error}",
            manifest.display()
        )
    })?;
    generated_artifact_paths_from_source(&source)
}

fn generated_artifact_paths_from_source(source: &[u8]) -> Result<Vec<String>> {
    let source = std::str::from_utf8(source)
        .map_err(|error| format!("前端 api-artifacts.mjs 不是 UTF-8：{error}"))?;
    let body = frozen_array_body(source, "generatedArtifactPaths")?;
    let mut paths = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in body.lines() {
        let value = line.trim().trim_end_matches(',').trim();
        if value.is_empty() {
            continue;
        }
        if let Some(name) = value.strip_prefix("...") {
            for nested in frozen_array_body(source, name)?.lines() {
                push_generated_path(nested, &mut paths, &mut seen)?;
            }
        } else {
            push_generated_path(value, &mut paths, &mut seen)?;
        }
    }
    if paths.is_empty() {
        return Err("generatedArtifactPaths 不能为空".into());
    }
    Ok(paths)
}

fn frozen_array_body<'a>(source: &'a str, name: &str) -> Result<&'a str> {
    let marker = format!("export const {name} = Object.freeze([");
    let start = source
        .find(&marker)
        .map(|index| index + marker.len())
        .ok_or_else(|| format!("前端 api-artifacts.mjs 缺少 {name} 清单"))?;
    let end = source[start..]
        .find("])")
        .map(|index| start + index)
        .ok_or_else(|| format!("前端 {name} 清单缺少结束标记"))?;
    Ok(&source[start..end])
}

fn push_generated_path(
    source: &str,
    paths: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    let value = source.trim().trim_end_matches(',').trim();
    if value.is_empty() {
        return Ok(());
    }
    let value = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .ok_or_else(|| format!("generatedArtifactPaths 包含不受支持的条目：{value}"))?;
    validate_frontend_relative_path(value)?;
    if !seen.insert(value.to_owned()) {
        return Err(format!("generatedArtifactPaths 包含重复路径：{value}").into());
    }
    paths.push(value.to_owned());
    Ok(())
}

fn validate_frontend_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || value.contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || !value.starts_with("src/")
    {
        return Err(format!("OpenAPI 派生文件路径不安全：{value}").into());
    }
    Ok(())
}

pub(super) fn snapshot_managed_files(managed_paths: &[PathBuf]) -> Result<Vec<Snapshot>> {
    managed_paths
        .iter()
        .map(|path| {
            let content = match fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            Ok(Snapshot {
                path: path.clone(),
                content,
            })
        })
        .collect()
}

pub(super) fn snapshot_staging_inputs(frontend_dir: &Path) -> Result<Vec<Snapshot>> {
    let scripts = frontend_dir.join("scripts");
    let mut paths = vec![
        frontend_dir.join("package.json"),
        frontend_dir.join("pnpm-lock.yaml"),
    ];
    collect_file_paths(&scripts, &mut paths)?;
    paths.sort();
    paths.dedup();
    snapshot_managed_files(&paths)
}

fn collect_file_paths(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_file_paths(&path, paths)?;
        } else {
            paths.push(path);
        }
    }
    Ok(())
}

pub(super) fn artifact_paths_from_inputs(
    inputs: &[Snapshot],
    frontend_dir: &Path,
) -> Result<Vec<String>> {
    let manifest = frontend_dir.join("scripts/api-artifacts.mjs");
    let source = inputs
        .iter()
        .find(|snapshot| snapshot.path == manifest)
        .and_then(|snapshot| snapshot.content.as_deref())
        .ok_or_else(|| format!("契约输入快照缺少 {}", manifest.display()))?;
    generated_artifact_paths_from_source(source)
}

pub(super) fn verify_input_snapshots(inputs: &[Snapshot]) -> Result<()> {
    for snapshot in inputs {
        if read_optional(&snapshot.path)? != snapshot.content {
            return Err(format!(
                "契约生成输入在事务期间发生变化，已拒绝安装任何结果：{}",
                snapshot.path.display()
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn prepare_staging_frontend(
    frontend_dir: &Path,
    snapshots: &[Snapshot],
    inputs: &[Snapshot],
) -> Result<StagingFrontend> {
    let parent = frontend_dir.join(".local-tests");
    fs::create_dir_all(&parent)?;
    let path = parent.join(format!("contract-stage-{}-{}", process::id(), nonce()?));
    fs::create_dir(&path)?;
    let staging = StagingFrontend { path };
    for input in inputs {
        let relative = input
            .path
            .strip_prefix(frontend_dir)
            .map_err(|_| format!("契约生成输入不属于前端工作区：{}", input.path.display()))?;
        if let Some(content) = &input.content {
            write_atomically(&staging.path.join(relative), content)?;
        }
    }
    for snapshot in snapshots {
        let Ok(relative) = snapshot.path.strip_prefix(frontend_dir) else {
            continue;
        };
        if let Some(content) = &snapshot.content {
            write_atomically(&staging.path.join(relative), content)?;
        }
    }
    Ok(staging)
}

fn desired_candidate_snapshots(
    before: &[Snapshot],
    backend_dir: &Path,
    frontend_dir: &Path,
    staging_dir: &Path,
    candidate: &[u8],
) -> Result<Vec<Snapshot>> {
    let backend_openapi = backend_dir.join("openapi/openapi.json");
    before
        .iter()
        .map(|snapshot| {
            let content = if snapshot.path == backend_openapi {
                Some(candidate.to_vec())
            } else {
                let relative = snapshot.path.strip_prefix(frontend_dir).map_err(|_| {
                    format!("契约托管文件不属于前端工作区：{}", snapshot.path.display())
                })?;
                read_optional(&staging_dir.join(relative))?
            };
            Ok(Snapshot {
                path: snapshot.path.clone(),
                content,
            })
        })
        .collect()
}

pub(super) fn desired_frontend_snapshots(
    before: &[Snapshot],
    frontend_dir: &Path,
    staging_dir: &Path,
) -> Result<Vec<Snapshot>> {
    before
        .iter()
        .map(|snapshot| {
            let relative = snapshot.path.strip_prefix(frontend_dir).map_err(|_| {
                format!("契约托管文件不属于前端工作区：{}", snapshot.path.display())
            })?;
            Ok(Snapshot {
                path: snapshot.path.clone(),
                content: read_optional(&staging_dir.join(relative))?,
            })
        })
        .collect()
}

pub(super) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
