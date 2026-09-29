use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

use super::model::{CANDIDATE_MARKER, CRUD_RESOURCE_ARTIFACT};

pub(super) fn contract_managed_paths(
    frontend_dir: &Path,
    artifact_paths: &[String],
) -> Result<Vec<PathBuf>> {
    let mut paths = vec![
        frontend_dir.join("openapi/openapi.json"),
        frontend_dir.join("openapi/source.json"),
        frontend_dir.join(CANDIDATE_MARKER),
    ];
    paths.extend(artifact_paths.iter().map(|path| frontend_dir.join(path)));
    paths.extend(previous_owned_paths(frontend_dir)?);
    if !artifact_paths
        .iter()
        .any(|path| path == CRUD_RESOURCE_ARTIFACT)
    {
        paths.push(frontend_dir.join(CRUD_RESOURCE_ARTIFACT));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn previous_owned_paths(frontend_dir: &Path) -> Result<Vec<PathBuf>> {
    let ownership = frontend_dir.join("src/api/generated/ownership.json");
    let bytes = match fs::read(&ownership) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let document: serde_json::Value = serde_json::from_slice(&bytes)?;
    if document.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("OpenAPI ownership 版本无效".into());
    }
    document
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or("OpenAPI ownership 缺少文件清单")?
        .iter()
        .map(|value| owned_path(frontend_dir, value))
        .collect()
}

fn owned_path(frontend_dir: &Path, value: &serde_json::Value) -> Result<PathBuf> {
    let relative = value.as_str().ok_or("OpenAPI ownership 路径必须是字符串")?;
    let path = Path::new(relative);
    let safe_relative = !path.is_absolute()
        && !relative.contains('\\')
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    let generated_api = relative.starts_with("src/api/generated/");
    let generated_shared =
        relative.starts_with("src/shared/") && relative.ends_with(".generated.json");
    if !safe_relative || (!generated_api && !generated_shared) {
        return Err(format!("OpenAPI ownership 包含非托管路径：{relative}").into());
    }
    Ok(frontend_dir.join(path))
}
