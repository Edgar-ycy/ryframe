use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{
    Result,
    cli::{ApiSyncCommand, ContractOperation},
    process::{command_output, run as run_process, run_pnpm, run_pnpm_with_env},
    workspace::root_dir,
};

const CRUD_RESOURCE_ARTIFACT: &str = "src/api/generated/crudResources.ts";
const CANDIDATE_MARKER: &str = "openapi/candidate.json";

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) path: PathBuf,
    pub(crate) content: Option<Vec<u8>>,
}

struct ContractLock {
    path: PathBuf,
    identity: Vec<u8>,
}

impl ContractLock {
    fn acquire(frontend_dir: &Path) -> Result<Self> {
        let path = frontend_dir.join("openapi/.xtask-contract.lock");
        let parent = path.parent().ok_or("契约锁文件缺少父目录")?;
        fs::create_dir_all(parent)?;
        let identity = format!("pid={}\nnonce={}\n", process::id(), nonce()?).into_bytes();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "无法获取契约同步锁 {}：{error}。若没有其他 cargo api-sync 正在运行，请删除该残留锁文件",
                    path.display()
                )
            })?;
        if let Err(error) = file.write_all(&identity).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(Self { path, identity })
    }
}

impl Drop for ContractLock {
    fn drop(&mut self) {
        if fs::read(&self.path).is_ok_and(|content| content.as_slice() == self.identity.as_slice())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct StagingFrontend {
    path: PathBuf,
}

pub(crate) trait ContractFileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

struct RealContractFileOperations;

impl ContractFileOperations for RealContractFileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::hard_link(source, target)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

impl Drop for StagingFrontend {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn run(operation: ContractOperation, frontend_dir: &Path) -> Result<()> {
    match operation {
        ContractOperation::Check => run_pnpm(frontend_dir, &["api:check"]),
    }
}

pub(crate) fn api_sync(command: &ApiSyncCommand, frontend_dir: &Path) -> Result<()> {
    match command {
        ApiSyncCommand::Candidate => sync_candidate(frontend_dir),
        ApiSyncCommand::Commit(reference) => sync_commit(reference, frontend_dir),
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
    let export_result = run_process(
        &root,
        "cargo",
        &[
            "run",
            "--locked",
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
            run_pnpm(staging, &["api:generate"])
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

fn canonical_contract(bytes: &[u8]) -> Result<Vec<u8>> {
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
    let mut managed_paths = contract_managed_paths(frontend_dir, &artifact_paths);
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

fn contract_managed_paths(frontend_dir: &Path, artifact_paths: &[String]) -> Vec<PathBuf> {
    let mut paths = vec![
        frontend_dir.join("openapi/openapi.json"),
        frontend_dir.join("openapi/source.json"),
        frontend_dir.join(CANDIDATE_MARKER),
    ];
    paths.extend(artifact_paths.iter().map(|path| frontend_dir.join(path)));
    if !artifact_paths
        .iter()
        .any(|path| path == CRUD_RESOURCE_ARTIFACT)
    {
        paths.push(frontend_dir.join(CRUD_RESOURCE_ARTIFACT));
    }
    paths
}

#[cfg(test)]
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
    let marker = "export const generatedArtifactPaths = Object.freeze([";
    let start = source
        .find(marker)
        .map(|index| index + marker.len())
        .ok_or("前端 api-artifacts.mjs 缺少 generatedArtifactPaths 清单")?;
    let end = source[start..]
        .find("])")
        .map(|index| start + index)
        .ok_or("前端 generatedArtifactPaths 清单缺少结束标记")?;
    let mut paths = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in source[start..end].lines() {
        let value = line.trim().trim_end_matches(',').trim();
        if value.is_empty() {
            continue;
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
    }
    if paths.is_empty() {
        return Err("generatedArtifactPaths 不能为空".into());
    }
    Ok(paths)
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

fn snapshot_managed_files(managed_paths: &[PathBuf]) -> Result<Vec<Snapshot>> {
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

fn snapshot_staging_inputs(frontend_dir: &Path) -> Result<Vec<Snapshot>> {
    let scripts = frontend_dir.join("scripts");
    let mut paths = vec![frontend_dir.join("package.json")];
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

fn artifact_paths_from_inputs(inputs: &[Snapshot], frontend_dir: &Path) -> Result<Vec<String>> {
    let manifest = frontend_dir.join("scripts/api-artifacts.mjs");
    let source = inputs
        .iter()
        .find(|snapshot| snapshot.path == manifest)
        .and_then(|snapshot| snapshot.content.as_deref())
        .ok_or_else(|| format!("契约输入快照缺少 {}", manifest.display()))?;
    generated_artifact_paths_from_source(source)
}

fn verify_input_snapshots(inputs: &[Snapshot]) -> Result<()> {
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

fn prepare_staging_frontend(
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

fn desired_frontend_snapshots(
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

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

struct ContractTransactionMarker {
    path: PathBuf,
}

impl ContractTransactionMarker {
    fn begin(paths: &[PathBuf], identity: &str) -> Result<Self> {
        let directory = contract_transaction_directory(paths)?;
        let path = directory.join(format!(".xtask-contract-transaction-{identity}"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        writeln!(file, "pid={}", process::id())?;
        writeln!(file, "targets={}", paths.len())?;
        file.sync_all()?;
        Ok(Self { path })
    }

    fn finish(&mut self, operations: &impl ContractFileOperations) -> Result<()> {
        operations.remove_file(&self.path).map_err(|error| {
            format!(
                "契约事务已完成，但清理事务标记 {} 失败：{error}",
                self.path.display()
            )
            .into()
        })
    }
}

fn install_snapshots(before: &[Snapshot], desired: &[Snapshot]) -> Result<()> {
    install_snapshots_with(before, desired, &RealContractFileOperations)
}

pub(crate) fn install_snapshots_with(
    before: &[Snapshot],
    desired: &[Snapshot],
    operations: &impl ContractFileOperations,
) -> Result<()> {
    if before.len() != desired.len()
        || before
            .iter()
            .zip(desired)
            .any(|(old, new)| old.path != new.path)
    {
        return Err("契约事务的新旧文件集合不一致".into());
    }
    let paths = before
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect::<Vec<_>>();
    reject_contract_recovery_artifacts(&paths)?;
    if before
        .iter()
        .zip(desired)
        .all(|(old, new)| old.content == new.content)
    {
        return Ok(());
    }
    let marker = format!("{}-{}", process::id(), nonce()?);
    let mut transaction = ContractTransactionMarker::begin(&paths, &marker)?;
    let mut staged = Vec::with_capacity(desired.len());
    for (index, (old, snapshot)) in before.iter().zip(desired).enumerate() {
        if old.content == snapshot.content {
            staged.push(None);
            continue;
        }
        let Some(content) = &snapshot.content else {
            staged.push(None);
            continue;
        };
        let stage_result: Result<PathBuf> = (|| {
            let path = contract_sibling_path(&snapshot.path, "new", &marker, index)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            if let Err(error) = output.write_all(content).and_then(|_| output.sync_all()) {
                drop(output);
                let _ = fs::remove_file(&path);
                return Err(error.into());
            }
            Ok(path)
        })();
        match stage_result {
            Ok(path) => staged.push(Some(path)),
            Err(error) => {
                let cleanup_errors =
                    cleanup_contract_files_with(staged.iter().flatten(), operations);
                if cleanup_errors.is_empty() {
                    transaction.finish(operations)?;
                    return Err(error);
                }
                return Err(format!(
                    "{error}；清理已暂存契约文件同时失败：{}；事务标记保留在 {}",
                    cleanup_errors.join("；"),
                    transaction.path.display()
                )
                .into());
            }
        }
    }

    let mut committed = Vec::new();
    for (index, (old, new)) in before.iter().zip(desired).enumerate() {
        let current = match fs::read(&old.path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return rollback_snapshot_install(
                    before,
                    desired,
                    &staged,
                    &committed,
                    error.into(),
                    operations,
                    &mut transaction,
                );
            }
        };
        if current != old.content {
            return rollback_snapshot_install(
                before,
                desired,
                &staged,
                &committed,
                format!("最终替换前文件发生变化，拒绝覆盖：{}", old.path.display()).into(),
                operations,
                &mut transaction,
            );
        }
        if old.content == new.content {
            continue;
        }

        let backup = if old.content.is_some() {
            let backup = match contract_sibling_path(&old.path, "backup", &marker, index) {
                Ok(path) => path,
                Err(error) => {
                    return rollback_snapshot_install(
                        before,
                        desired,
                        &staged,
                        &committed,
                        error,
                        operations,
                        &mut transaction,
                    );
                }
            };
            if let Err(error) = operations.rename(&old.path, &backup) {
                return rollback_snapshot_install(
                    before,
                    desired,
                    &staged,
                    &committed,
                    error.into(),
                    operations,
                    &mut transaction,
                );
            }
            committed.push((index, Some(backup.clone())));
            if fs::read(&backup).ok() != old.content {
                return rollback_snapshot_install(
                    before,
                    desired,
                    &staged,
                    &committed,
                    format!(
                        "原契约移入备份后内容发生变化，拒绝继续安装：{}",
                        backup.display()
                    )
                    .into(),
                    operations,
                    &mut transaction,
                );
            }
            Some(backup)
        } else {
            committed.push((index, None));
            None
        };
        let _ = backup;
        if new.content.is_some()
            && let Err(error) = operations.hard_link(
                staged[index]
                    .as_ref()
                    .expect("有目标内容的契约快照必须已经暂存"),
                &new.path,
            )
        {
            return rollback_snapshot_install(
                before,
                desired,
                &staged,
                &committed,
                error.into(),
                operations,
                &mut transaction,
            );
        }
        if let Some(stage) = &staged[index]
            && let Err(error) = operations.remove_file(stage)
        {
            return rollback_snapshot_install(
                before,
                desired,
                &staged,
                &committed,
                error.into(),
                operations,
                &mut transaction,
            );
        }
    }

    for snapshot in desired {
        if read_optional(&snapshot.path)? != snapshot.content {
            return Err(format!(
                "契约安装后文件被并发修改，已保留当前内容、事务备份和标记 {}：{}",
                transaction.path.display(),
                snapshot.path.display()
            )
            .into());
        }
    }
    for (index, backup) in &committed {
        if let Some(backup) = backup
            && fs::read(backup).ok() != before[*index].content
        {
            return Err(format!(
                "契约已安装，但原文件备份 {} 被并发修改；已保留备份和事务标记 {}，拒绝清理",
                backup.display(),
                transaction.path.display()
            )
            .into());
        }
    }
    let backups = committed
        .iter()
        .filter_map(|(_, backup)| backup.as_ref())
        .collect::<Vec<_>>();
    let cleanup_errors = cleanup_contract_files_with(backups.into_iter(), operations);
    if cleanup_errors.is_empty() {
        transaction.finish(operations)
    } else {
        Err(format!(
            "契约文件已完整写入，但清理事务备份失败：{}；事务标记保留在 {}，下次同步会安全拒绝继续",
            cleanup_errors.join("；"),
            transaction.path.display()
        )
        .into())
    }
}

fn rollback_snapshot_install(
    before: &[Snapshot],
    desired: &[Snapshot],
    staged: &[Option<PathBuf>],
    committed: &[(usize, Option<PathBuf>)],
    cause: Box<dyn std::error::Error>,
    operations: &impl ContractFileOperations,
    transaction: &mut ContractTransactionMarker,
) -> Result<()> {
    let mut errors = Vec::new();
    for (index, backup) in committed.iter().rev() {
        let target = &before[*index].path;
        let current = match read_optional(target) {
            Ok(current) => current,
            Err(error) => {
                errors.push(format!("读取 {} 失败：{error}", target.display()));
                continue;
            }
        };
        if current == desired[*index].content {
            if current.is_some()
                && let Err(error) = operations.remove_file(target)
                && error.kind() != io::ErrorKind::NotFound
            {
                errors.push(format!("删除半写入文件 {} 失败：{error}", target.display()));
                continue;
            }
        } else if current.is_some() {
            errors.push(format!(
                "{} 在替换后被再次修改，已保留当前内容和事务备份",
                target.display()
            ));
            continue;
        }
        if let Some(backup) = backup
            && let Err(error) = operations.rename(backup, target)
        {
            errors.push(format!(
                "恢复 {} 失败，原文件备份保留在 {}：{error}",
                target.display(),
                backup.display()
            ));
        }
    }
    errors.extend(cleanup_contract_files_with(
        staged.iter().flatten(),
        operations,
    ));
    if errors.is_empty() {
        if let Err(error) = transaction.finish(operations) {
            return Err(format!("{cause}；{error}").into());
        }
        Err(cause)
    } else {
        Err(format!(
            "{cause}；契约事务回滚未能安全完成：{}；事务标记保留在 {}",
            errors.join("；"),
            transaction.path.display()
        )
        .into())
    }
}

fn contract_transaction_directory(paths: &[PathBuf]) -> Result<&Path> {
    paths
        .iter()
        .find(|path| {
            path.file_name().and_then(|value| value.to_str()) == Some("candidate.json")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|value| value.to_str())
                    == Some("openapi")
        })
        .or_else(|| paths.first())
        .and_then(|path| path.parent())
        .ok_or_else(|| "契约事务没有可用的标记目录".into())
}

fn contract_sibling_path(path: &Path, role: &str, marker: &str, index: usize) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("契约文件名不是 UTF-8：{}", path.display()))?;
    Ok(path.with_file_name(format!(".{name}.xtask-{role}-{marker}-{index}")))
}

fn reject_contract_recovery_artifacts(paths: &[PathBuf]) -> Result<()> {
    let mut artifacts = Vec::new();
    let mut parents = Vec::new();
    for path in paths {
        let parent = path
            .parent()
            .ok_or_else(|| format!("契约文件没有父目录：{}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("契约文件名不是 UTF-8：{}", path.display()))?;
        let prefix = format!(".{name}.xtask-");
        parents.push(parent.to_path_buf());
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let candidate = entry?.path();
            if candidate
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    value
                        .strip_prefix(&prefix)
                        .is_some_and(is_contract_file_artifact)
                })
            {
                artifacts.push(candidate);
            }
        }
    }
    parents.sort();
    parents.dedup();
    for parent in parents {
        let entries = match fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let candidate = entry?.path();
            if candidate
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| value.strip_prefix(".xtask-contract-transaction-"))
                .is_some_and(is_contract_identity)
            {
                artifacts.push(candidate);
            }
        }
    }
    artifacts.sort();
    artifacts.dedup();
    if artifacts.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "检测到上次契约事务未完整结束，已拒绝继续写入：{}；请根据 backup 恢复或确认目标已完整写入后再清理这些精确文件",
            artifacts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("；")
        )
        .into())
    }
}

fn is_contract_file_artifact(value: &str) -> bool {
    let mut parts = value.split('-');
    matches!(parts.next(), Some("new" | "backup"))
        && parts.next().is_some_and(is_contract_number)
        && parts.next().is_some_and(is_contract_number)
        && parts.all(is_contract_number)
}

fn is_contract_identity(value: &str) -> bool {
    let mut parts = value.split('-');
    parts.next().is_some_and(is_contract_number)
        && parts.next().is_some_and(is_contract_number)
        && parts.next().is_none()
}

fn is_contract_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn cleanup_contract_files_with<'a>(
    paths: impl Iterator<Item = &'a PathBuf>,
    operations: &impl ContractFileOperations,
) -> Vec<String> {
    paths
        .filter_map(|path| match operations.remove_file(path) {
            Ok(()) => None,
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => Some(format!("{}：{error}", path.display())),
        })
        .collect()
}

fn ensure_atomic_write_has_no_recovery_artifacts(path: &Path) -> Result<()> {
    reject_contract_recovery_artifacts(&[path.to_path_buf()])
}

fn write_atomically(path: &Path, content: &[u8]) -> Result<()> {
    write_atomically_with(path, content, &RealContractFileOperations)
}

pub(crate) fn write_atomically_with(
    path: &Path,
    content: &[u8],
    operations: &impl ContractFileOperations,
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("文件没有父目录：{}", path.display()))?;
    fs::create_dir_all(parent)?;
    ensure_atomic_write_has_no_recovery_artifacts(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("文件名不是 UTF-8：{}", path.display()))?;
    let marker = format!("{}-{}", process::id(), nonce()?);
    let staged = path.with_file_name(format!(".{name}.xtask-new-{marker}"));
    let backup = path.with_file_name(format!(".{name}.xtask-backup-{marker}"));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    if let Err(error) = output.write_all(content).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = operations.remove_file(&staged);
        return Err(error.into());
    }
    drop(output);

    let had_original = path.exists();
    if had_original && let Err(error) = operations.rename(path, &backup) {
        let cleanup = operations.remove_file(&staged);
        return match cleanup {
            Ok(()) => Err(error.into()),
            Err(cleanup_error) => Err(format!(
                "{error}；清理暂存文件 {} 同时失败：{cleanup_error}",
                staged.display()
            )
            .into()),
        };
    }
    if let Err(error) = operations.rename(&staged, path) {
        let mut recovery_errors = Vec::new();
        if had_original && let Err(restore_error) = operations.rename(&backup, path) {
            recovery_errors.push(format!(
                "恢复目标失败，原文件备份保留在 {}：{restore_error}",
                backup.display()
            ));
        }
        if let Err(cleanup_error) = operations.remove_file(&staged)
            && cleanup_error.kind() != io::ErrorKind::NotFound
        {
            recovery_errors.push(format!(
                "清理暂存文件 {} 失败：{cleanup_error}",
                staged.display()
            ));
        }
        return if recovery_errors.is_empty() {
            Err(error.into())
        } else {
            Err(format!("{error}；{}", recovery_errors.join("；")).into())
        };
    }
    if had_original && let Err(error) = operations.remove_file(&backup) {
        eprintln!(
            "警告：{} 已写入新内容，但清理备份 {} 失败：{error}；可确认后人工删除备份",
            path.display(),
            backup.display()
        );
    }
    Ok(())
}

fn nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}

fn sync_commit(reference: &str, frontend_dir: &Path) -> Result<()> {
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
    let managed_paths = contract_managed_paths(frontend_dir, &artifact_paths);
    reject_contract_recovery_artifacts(&managed_paths)?;
    let before = snapshot_managed_files(&managed_paths)?;
    let staging = prepare_staging_frontend(frontend_dir, &before, &inputs)?;
    run_pnpm_with_env(&staging.path, &["api:sync"], &environment)?;
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

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(content);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
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
