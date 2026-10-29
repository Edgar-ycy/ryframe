//! 共用原有操作系统白名单与明确绑定的应用环境，不读取应用覆盖变量。
use super::super::storage::StorageInputs;
use super::{receipts, registration::Registration};
use crate::Result;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(super) fn registered(
    registration: &Registration,
    request: &Value,
) -> Result<BTreeMap<String, String>> {
    let private =
        serde_json::from_value::<BTreeMap<String, String>>(registration.environment.clone())?;
    let private = private
        .into_iter()
        .map(|(k, v)| (k.to_ascii_uppercase(), v))
        .collect();
    Ok(merged(
        &registration.backend,
        request,
        &registration.target,
        &private,
    ))
}

pub(super) fn test(
    backend: &Path,
    request: &Value,
    observation: &Path,
    temporary: &Path,
) -> BTreeMap<String, String> {
    let temporary = temporary.to_string_lossy().into_owned();
    let private = BTreeMap::from([
        ("APP_ENV".into(), "test".into()),
        ("TEMP".into(), temporary.clone()),
        ("TMP".into(), temporary),
    ]);
    merged(backend, request, observation, &private)
}

pub(crate) fn temporary_identity(backend: &Path, temporary: &Path) -> Result<Value> {
    use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};
    let path = validate_local_test_path(temporary, backend, LocalTestPathKind::ExistingDirectory)?;
    let (device, inode) = super::super::super::resources::common::device_inode_for(
        &path,
        &std::fs::symlink_metadata(&path)?,
    )?;
    let identity = serde_json::json!({"path":path,"device":device,"inode":inode});
    super::super::super::resources::common::directory_identity(backend, &identity)?;
    Ok(identity)
}

fn merged(
    backend: &Path,
    request: &Value,
    observation: &Path,
    private: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    receipts::command_environment(&StorageInputs {
        backend,
        request,
        environment: private,
        observation_dir: observation,
    })
}
