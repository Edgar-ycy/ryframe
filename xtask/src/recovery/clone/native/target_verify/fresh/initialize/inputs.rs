//! 初始化子进程只读取登记的文件；持有现有保护句柄直到完整发布结束。
use super::super::super::super::{json as document, resources::common};
use super::super::super::{guard::Inputs, input::request_configuration, storage::StorageInputs};
use super::super::registration::Registration;
use crate::Result;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub(crate) fn protect(
    reg: &Registration,
    request: &Value,
    environment: &BTreeMap<String, String>,
) -> Result<Inputs> {
    let inputs = StorageInputs {
        backend: &reg.backend,
        request,
        environment,
        observation_dir: &reg.target,
    };
    let root = inputs.execution_backend()?;
    let (directory, files) = request_configuration::fresh_files(&root, environment, request)?;
    let mut paths = files
        .keys()
        .map(|name| directory.join(name))
        .collect::<BTreeSet<_>>();
    for descriptor in [&reg.value["request"], &reg.value["environment"]] {
        paths.insert(common::bound_file(
            &reg.backend,
            descriptor,
            "初始化固定文件",
        )?);
    }
    paths.extend([
        reg.workspace.join("prepared-files.json"),
        reg.workspace.join("registration.json"),
    ]);
    for db in document::array(&request["target"]["databases"], "四目标")? {
        paths.insert(PathBuf::from(document::text(
            &db["defaults_file"],
            "MySQL配置",
        )?));
    }
    let protected = Inputs::open(paths)?;
    if request_configuration::fresh_files(&root, environment, request)? != (directory, files) {
        return Err("初始化取得保护句柄期间配置集合漂移；保留现场，禁止写入".into());
    }
    protected.check()?;
    Ok(protected)
}
