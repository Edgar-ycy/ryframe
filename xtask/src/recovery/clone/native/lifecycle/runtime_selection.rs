//! 旧复制运行按本侧冻结收据选择原目录，不读取对侧。
use crate::{
    Result,
    cli::CloneSide,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{clone::native::resources::common, fixture_control::portable_path},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) fn fields(side: CloneSide) -> (&'static str, &'static str) {
    match side {
        CloneSide::Source => ("source_environment", "source_request"),
        CloneSide::Target => ("target_environment", "initialized"),
    }
}

pub(super) fn environment(
    backend: &Path,
    manifest: &Value,
    side: CloneSide,
) -> Result<BTreeMap<String, String>> {
    let environment = common::environment(backend, &manifest[fields(side).0])?;
    let mut names = BTreeSet::new();
    if environment.iter().any(|(name, value)| {
        name.is_empty()
            || name.contains(['=', '\0'])
            || value.contains('\0')
            || !names.insert(name.to_uppercase())
    }) {
        return Err("原登记环境包含无效或重复字段".into());
    }
    Ok(environment)
}

pub(super) fn selected(backend: &Path, manifest: &Value, side: CloneSide) -> Result<Value> {
    let receipt = common::read_bound(backend, &manifest[fields(side).1], "已登记侧运行选择")?;
    Ok(match side {
        CloneSide::Source => receipt["source"].clone(),
        CloneSide::Target => receipt["generation"]["selected"].clone(),
    })
}

pub(super) fn directory(path: &Path) -> Result<PathBuf> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    Ok(portable_path(&path.canonicalize()?))
}
