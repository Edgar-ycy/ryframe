use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use crate::{
    Result,
    process::{command_output, run as run_process},
    workspace::root_dir,
};

use super::{
    candidate::{
        CANDIDATE_GENERATION_ARGS, artifact_paths_from_inputs, canonical_contract,
        prepare_staging_frontend, read_optional, snapshot_managed_files, snapshot_staging_inputs,
        verify_input_snapshots,
    },
    formal::github_repository_identifier,
    model::{CANDIDATE_MARKER, Snapshot, StagingFrontend, nonce},
    ownership::contract_managed_paths,
    recovery::reject_contract_recovery_artifacts,
    source::verify_contract_source,
};

pub(super) fn check_current(frontend_dir: &Path) -> Result<()> {
    let backend_dir = root_dir();
    let source_before = crate::devex::source_fingerprints(&backend_dir, frontend_dir)?;
    let result = check_current_with(
        &backend_dir,
        frontend_dir,
        |output| export_current_openapi(&backend_dir, output),
        |staging| run_process(staging, "node", CANDIDATE_GENERATION_ARGS),
    );
    let source_after = crate::devex::source_fingerprints(&backend_dir, frontend_dir)?;
    if source_after != source_before {
        let changed = "前后端源码输入在 API 只读核验期间发生变化，已拒绝本次结果";
        return match result {
            Ok(()) => Err(changed.into()),
            Err(error) => Err(format!("{error}；{changed}").into()),
        };
    }
    result?;
    println!("当前后端 OpenAPI、前端正式契约及全部派生文件一致。");
    Ok(())
}

pub(crate) fn check_current_with<E, G>(
    backend_dir: &Path,
    frontend_dir: &Path,
    export: E,
    generate: G,
) -> Result<()>
where
    E: FnOnce(&Path) -> Result<()>,
    G: FnOnce(&Path) -> Result<()>,
{
    require_workspace(backend_dir, "后端")?;
    require_workspace(frontend_dir, "前端")?;
    let backend_openapi = snapshot_required(&backend_dir.join("openapi/openapi.json"))?;
    let inputs = snapshot_staging_inputs(frontend_dir)?;
    let artifact_paths = artifact_paths_from_inputs(&inputs, frontend_dir)?;
    let managed_paths = contract_managed_paths(frontend_dir, &artifact_paths)?;
    reject_contract_recovery_artifacts(&managed_paths)?;
    let frontend_before = snapshot_managed_files(&managed_paths)?;

    let mut export_dir = ExportDirectory::create(backend_dir)?;
    let result = (|| {
        export(&export_dir.output)?;
        let exported = fs::read(&export_dir.output).map_err(|error| {
            format!(
                "无法读取当前后端导出的 OpenAPI {}：{error}",
                export_dir.output.display()
            )
        })?;
        let candidate = canonical_contract(&exported)?;
        validate_formal_frontend(frontend_dir, &frontend_before, &candidate)?;
        compare_snapshot(
            &backend_openapi,
            &candidate,
            "后端提交的 openapi/openapi.json",
        )?;
        fs::write(&export_dir.output, &candidate)?;
        verify_current_source(backend_dir, frontend_dir, &export_dir.output)?;
        verify_readonly_candidate(
            frontend_dir,
            &backend_openapi,
            &frontend_before,
            &inputs,
            &artifact_paths,
            &candidate,
            generate,
        )
    })();
    finish_with_cleanup(result, export_dir.cleanup(), "OpenAPI 导出临时目录")
}

fn verify_readonly_candidate<G>(
    frontend_dir: &Path,
    backend_openapi: &Snapshot,
    frontend_before: &[Snapshot],
    inputs: &[Snapshot],
    artifact_paths: &[String],
    candidate: &[u8],
    generate: G,
) -> Result<()>
where
    G: FnOnce(&Path) -> Result<()>,
{
    let staging = prepare_staging_frontend(frontend_dir, frontend_before, inputs)?;
    let result = (|| {
        super::atomic::write_atomically(&staging.path.join("openapi/openapi.json"), candidate)?;
        generate(&staging.path)?;
        require_generated_artifacts(&staging.path, artifact_paths)?;
        verify_input_snapshots(inputs)?;
        verify_unchanged(backend_openapi, "后端契约输入")?;
        verify_unchanged_all(frontend_before, "前端受控契约文件")?;
        compare_staging(frontend_dir, &staging.path, frontend_before)
    })();
    cleanup_staging(staging, result)
}

fn export_current_openapi(backend_dir: &Path, output: &Path) -> Result<()> {
    let output = output.to_str().ok_or("候选 OpenAPI 路径不是有效 UTF-8")?;
    run_process(
        backend_dir,
        "cargo",
        &[
            "run",
            "--locked",
            "-p",
            "ryframe-api",
            "--bin",
            "export_openapi",
            "--",
            output,
        ],
    )
}

fn require_workspace(root: &Path, label: &str) -> Result<()> {
    if !root.is_dir() {
        return Err(format!("{label}目录不存在：{}", root.display()).into());
    }
    Ok(())
}

fn snapshot_required(path: &Path) -> Result<Snapshot> {
    let content = fs::read(path)
        .map_err(|error| format!("无法读取受控契约文件 {}：{error}", path.display()))?;
    Ok(Snapshot {
        path: path.to_path_buf(),
        content: Some(content),
    })
}

fn validate_formal_frontend(
    frontend_dir: &Path,
    snapshots: &[Snapshot],
    candidate: &[u8],
) -> Result<()> {
    let marker = frontend_dir.join(CANDIDATE_MARKER);
    if snapshot_content(snapshots, &marker)?.is_some() {
        return Err("前端当前处于候选契约状态，缺少可比较的正式契约".into());
    }
    let openapi = frontend_dir.join("openapi/openapi.json");
    compare_bytes(
        snapshot_content(snapshots, &openapi)?.as_deref(),
        Some(candidate),
        "前端正式 openapi/openapi.json",
    )?;
    Ok(())
}

fn verify_current_source(backend: &Path, frontend: &Path, candidate: &Path) -> Result<()> {
    let head = command_output(backend, "git", &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let repository = github_repository_identifier(env!("CARGO_PKG_REPOSITORY"))?;
    verify_contract_source(
        backend,
        head.trim(),
        &repository,
        &frontend.join("openapi/source.json"),
        &frontend.join("openapi/openapi.json"),
        candidate,
    )?;
    Ok(())
}

fn snapshot_content(snapshots: &[Snapshot], path: &Path) -> Result<Option<Vec<u8>>> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.path == path)
        .map(|snapshot| snapshot.content.clone())
        .ok_or_else(|| format!("契约快照缺少 {}", path.display()).into())
}

fn compare_snapshot(snapshot: &Snapshot, expected: &[u8], label: &str) -> Result<()> {
    compare_bytes(snapshot.content.as_deref(), Some(expected), label)
}

fn compare_bytes(actual: Option<&[u8]>, expected: Option<&[u8]>, label: &str) -> Result<()> {
    if actual != expected {
        return Err(format!("{label} 与当前后端实际导出的 OpenAPI 不一致").into());
    }
    Ok(())
}

fn require_generated_artifacts(staging: &Path, paths: &[String]) -> Result<()> {
    for relative in paths {
        if !staging.join(relative).is_file() {
            return Err(format!("OpenAPI 生成器遗漏派生文件：{relative}").into());
        }
    }
    Ok(())
}

fn verify_unchanged(snapshot: &Snapshot, label: &str) -> Result<()> {
    if read_optional(&snapshot.path)? != snapshot.content {
        return Err(format!("{label}在只读核验期间发生变化：{}", snapshot.path.display()).into());
    }
    Ok(())
}

fn verify_unchanged_all(snapshots: &[Snapshot], label: &str) -> Result<()> {
    for snapshot in snapshots {
        verify_unchanged(snapshot, label)?;
    }
    Ok(())
}

fn compare_staging(frontend: &Path, staging: &Path, before: &[Snapshot]) -> Result<()> {
    for snapshot in before {
        let relative = snapshot
            .path
            .strip_prefix(frontend)
            .map_err(|_| format!("前端契约文件不属于前端工作区：{}", snapshot.path.display()))?;
        if read_optional(&staging.join(relative))? != snapshot.content {
            return Err(format!("前端 OpenAPI 派生文件发生漂移：{}", relative.display()).into());
        }
    }
    Ok(())
}

fn cleanup_staging(staging: StagingFrontend, result: Result<()>) -> Result<()> {
    let path = staging.path.clone();
    drop(staging);
    let cleanup = ensure_removed(&path);
    finish_with_cleanup(result, cleanup, "前端契约临时 staging")
}

fn ensure_removed(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法清理 {}：{error}", path.display()).into()),
    }
}

fn finish_with_cleanup<T>(result: Result<T>, cleanup: Result<()>, label: &str) -> Result<T> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(format!("{label}清理失败：{cleanup}").into()),
        (Err(error), Err(cleanup)) => Err(format!("{error}；{label}清理失败：{cleanup}").into()),
    }
}

struct ExportDirectory {
    path: PathBuf,
    output: PathBuf,
    active: bool,
}

impl ExportDirectory {
    fn create(backend_dir: &Path) -> Result<Self> {
        let parent = backend_dir.join("target/xtask");
        fs::create_dir_all(&parent)?;
        let path = parent.join(format!("api-contract-check-{}-{}", process::id(), nonce()?));
        fs::create_dir(&path).map_err(|error| {
            format!("无法创建 OpenAPI 导出临时目录 {}：{error}", path.display())
        })?;
        Ok(Self {
            output: path.join("openapi.json"),
            path,
            active: true,
        })
    }

    fn cleanup(&mut self) -> Result<()> {
        let result = ensure_removed(&self.path);
        if result.is_ok() {
            self.active = false;
        }
        result
    }
}

impl Drop for ExportDirectory {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
