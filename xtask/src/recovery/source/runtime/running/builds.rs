//! 读取已登记的干净构建来源；历史 B0 只核对原 Git 对象和补丁，不重建或执行旧代码。
use super::super::staging;
use crate::{
    Result,
    recovery::{
        clone::native::{bridge, digest, json as document, maintenance, resources::common, source},
        fixture_control::git_root,
    },
};
use serde_json::Value;
use std::path::{Path, PathBuf};

const ADAPTER: &str = "legacy-stable-readiness-b0-v1";
const ADAPTER_COMMIT: &str = "c05114bcdf5c369cd74087db6317ce3c8f89bee8";
const PRODUCT_COMMIT: &str = "815c5eafb09d4b493319d255fb7ab88ddd02c8b6";
const PRODUCT_TREE: &str = "aeb1824ee52ea12d53d0cf87c327c7be3d1e12b4";
const PATCH_SHA256: &str = "28ad29f21ca9fd58356d059700bcc871172466e0a62286c5ccdbf68b1ed3fba3";

pub(in crate::recovery::source::evidence) fn verify(
    backend: &Path,
    request: &Value,
) -> Result<Value> {
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
    Ok(build)
}

fn registered(backend: &Path, request: &Value) -> Result<(PathBuf, Value)> {
    let execution = Path::new(document::text(&request["execution_backend"], "来源执行根")?);
    let canonical = git_root(execution, "来源执行根")?;
    if canonical != execution {
        return Err("来源执行树必须是已登记的规范 Git 根目录".into());
    }
    let inventory = source::capture(execution)?;
    if inventory.source.snapshot.head
        != document::text(&request["expected_backend_sha"], "来源完整 SHA")?
        || !inventory.source.snapshot.clean
    {
        return Err("来源构建必须使用声明的精确干净后端源码".into());
    }
    match request["adapter_contract"].as_str() {
        None if request["adapter_contract"].is_null() && request["product_backend"].is_null() => (),
        Some(ADAPTER) => adapter(backend, request, &inventory)?,
        _ => return Err("未知、不完整或混用普通产品根的来源适配声明".into()),
    }
    Ok((execution.to_owned(), source::inventory_value(&inventory)))
}

fn adapter(backend: &Path, request: &Value, inventory: &source::Inventory) -> Result<()> {
    if request["expected_backend_sha"] != ADAPTER_COMMIT {
        return Err("B0 适配必须绑定原精确提交".into());
    }
    let product = Path::new(document::text(&request["product_backend"], "B0 产品根")?);
    if git_root(backend, "B0 协调根")? != backend || git_root(product, "B0 产品根")? != product
    {
        return Err("B0 产品和协调根必须是规范 Git 根".into());
    }
    let original = source::capture(product)?;
    let patch = backend.join("xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch");
    common::common_file(backend, &patch, "B0 已登记补丁")?;
    if inventory.source.snapshot.head != ADAPTER_COMMIT
        || original.source.snapshot.head != PRODUCT_COMMIT
        || !original.source.snapshot.clean
        || digest::file(&patch)?.sha256 != PATCH_SHA256
        || git_tree(backend, PRODUCT_COMMIT)? != PRODUCT_TREE
        || source::backend_domains(inventory)?["product"]
            != source::backend_domains(&original)?["product"]
    {
        return Err("B0 适配改变了已登记补丁、原产品提交或产品输入".into());
    }
    Ok(())
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
