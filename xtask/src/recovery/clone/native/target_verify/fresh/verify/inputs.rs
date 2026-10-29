//! 原 T、原 E、私有连接文件和固定程序在完整观察期间持有保护句柄。
use super::super::super::super::{json as document, resources::common};
use super::super::super::{
    guard::Inputs, input::request_configuration, storage::StorageInputs, tree,
};
use crate::Result;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) fn protect(inputs: &StorageInputs<'_>, target: &Path, before: &Value) -> Result<Inputs> {
    let root = inputs.execution_backend()?;
    let (directory, configuration) =
        request_configuration::fresh_files(&root, inputs.environment, inputs.request)?;
    let mut paths = tree::files(target, before)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    paths.extend(configuration.keys().map(|name| directory.join(name)));
    let prepared = document::read(&target.join("prepare.json"))?;
    paths.insert(common::bound_file(
        inputs.backend,
        &prepared["request"],
        "fresh 固定请求",
    )?);
    for db in document::array(&inputs.request["target"]["databases"], "四目标")? {
        paths.insert(PathBuf::from(document::text(
            &db["defaults_file"],
            "MySQL 配置",
        )?));
    }
    let binaries = super::super::super::binding_request::binaries(inputs.backend, inputs.request)?;
    for descriptor in document::array(&binaries["bindings"], "fresh 二进制")? {
        paths.insert(super::super::super::input::request::external_file(
            descriptor,
        )?);
    }
    let protected = Inputs::open(paths)?;
    if request_configuration::fresh_files(&root, inputs.environment, inputs.request)?
        != (directory, configuration)
    {
        return Err("取得复验保护句柄期间配置集合漂移；保留现场，禁止重放".into());
    }
    protected.check()?;
    Ok(protected)
}
