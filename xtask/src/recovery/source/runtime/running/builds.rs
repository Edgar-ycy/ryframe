//! 读取已登记的干净构建来源；历史 B0 只核对原 Git 对象和补丁，不重建或执行旧代码。
use super::super::staging;
use crate::{
    Result,
    recovery::clone::native::{bridge, json as document, maintenance, resources::common},
};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "builds/held.rs"]
pub(in crate::recovery::source::evidence) mod held;

pub(in crate::recovery::source::evidence) fn verify(
    backend: &Path,
    request: &Value,
) -> Result<Value> {
    super::super::generation_request::configuration::verify(backend, request)?;
    super::super::generation_request::configuration::execution_source(backend, request)?;
    let (execution, inventory) = registered(backend, request)?;
    let build_path = common::bound_file(backend, &request["backend_build"], "来源产品构建")?;
    let build = document::read(&build_path)?;
    bridge::verify_build(&execution, &build_path, &build)?;
    if build["sources"]["full"] != inventory {
        return Err("来源 API/Worker 构建没有绑定当前完整注册来源".into());
    }
    let maintenance_path =
        common::bound_file(&execution, &request["maintenance_build"], "来源维护构建")?;
    let maintenance = maintenance::verify_evidence(&execution, &maintenance_path, None)?;
    if maintenance["source"] != inventory["source"] {
        return Err("来源维护构建和产品构建不属于同一完整执行源码".into());
    }
    let private = common::read_bound(backend, &request["source_environment"], "来源私有环境")?;
    document::exact(&private, &["environment"], "来源私有环境")?;
    let variables = serde_json::from_value(private["environment"].clone())
        .map_err(|_| "来源私有运行环境必须为字符串映射；检查已绑定环境文件，不回显字段值")?;
    super::super::environment::configured(&variables)?;
    if registered(backend, request)? != (execution.clone(), inventory) {
        return Err("来源构建核验期间当前注册源码变化".into());
    }
    common::bound_file(backend, &request["backend_build"], "来源产品构建")?;
    common::bound_file(&execution, &request["maintenance_build"], "来源维护构建")?;
    common::bound_file(backend, &request["source_environment"], "来源私有环境")?;
    super::super::generation_request::configuration::verify(backend, request)?;
    super::super::generation_request::configuration::execution_source(backend, request)?;
    Ok(build)
}

fn registered(backend: &Path, request: &Value) -> Result<(PathBuf, Value)> {
    let execution = Path::new(document::text(&request["execution_backend"], "来源执行根")?);
    let value = crate::recovery::runtime::build::source::registered(
        backend,
        execution,
        document::text(&request["expected_backend_sha"], "来源精确SHA")?,
        &request["adapter_contract"],
        &request["product_backend"],
    )?;
    Ok((execution.to_owned(), value["inventory"].clone()))
}

pub(in crate::recovery::source::evidence::running) fn git_tree(
    root: &Path,
    head: &str,
) -> Result<String> {
    if head.len() != 40
        || !head
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("历史来源必须是完整小写 SHA".into());
    }
    let output = staging::git(root, &["rev-parse", &format!("{head}^{{tree}}")])?;
    let text = std::str::from_utf8(&output)
        .map_err(|_| "历史 Git 树响应不是 UTF-8")?
        .trim();
    if text.len() != 40
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("历史 Git 树响应不是完整小写 SHA".into());
    }
    Ok(text.to_owned())
}
