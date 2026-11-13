//! 双端真实构建与完整源码三域核验；不构建、不接受模拟产物。
use super::{adapter, common, document, path, repository, source};
use crate::{Result, recovery::clone::native::bridge};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn inventory(root: &Path, expected: Option<&str>) -> Result<Value> {
    let root = repository(root)?;
    let inventory = source::capture(&root)?;
    if !inventory.source.snapshot.clean
        || expected.is_some_and(|head| inventory.source.snapshot.head != head)
    {
        return Err("比较来源必须使用声明的精确干净 Git 工作树".into());
    }
    Ok(source::inventory_value(&inventory))
}

fn backend(
    root: &Path,
    receipt_path: &Path,
    environment_path: &Path,
    inventory: &Value,
) -> Result<(Value, Value, Value)> {
    common::common_file(root, receipt_path, "比较后端构建收据")?;
    let binding = common::binding(receipt_path)?;
    let value = common::read_bound(root, &binding, "比较后端构建收据")?;
    bridge::verify_build(root, receipt_path, &value)?;
    if value["kind"] != "restore-backend-build"
        || value["sources"] != source::backend_domains(&source::validate_inventory(inventory)?)?
    {
        return Err("比较后端构建与当前完整源码不同".into());
    }
    let environment =
        crate::recovery::inputs::document::read(environment_path, root, "比较后端构建环境")?;
    let head = document::text(&inventory["source"]["snapshot"]["head"], "后端执行提交")?;
    bridge::build_context::verify_registered(
        root,
        environment_path,
        head,
        &binding,
        &value["build"],
    )?;
    environment.assert_unchanged()?;
    bridge::verify_build(root, receipt_path, &value)?;
    if common::binding(receipt_path)? != binding {
        return Err("比较后端构建收据在核验期间变化".into());
    }
    Ok((
        value,
        binding,
        crate::recovery::inputs::document::binding(&environment),
    ))
}

fn frontend(root: &Path, receipt: &Path, inventory: &Value, b0: bool) -> Result<Value> {
    if receipt != root.join("dist/.vite/restore-build.json") {
        return Err("比较前端构建收据必须是同根 dist 内的正式收据".into());
    }
    let binding = common::binding(receipt)?;
    let head = document::text(&inventory["source"]["snapshot"]["head"], "前端精确提交")?;
    use crate::recovery::frontend_build;
    if b0 {
        frontend_build::b0::validate(root, Some(inventory), head)?.verify()?;
    } else {
        frontend_build::validate(root, Some(inventory), Some(head))?.verify()?;
    }
    if common::binding(receipt)? != binding {
        return Err("比较前端构建收据在核验期间变化".into());
    }
    Ok(binding)
}

pub(super) fn capture(
    backend_root: &Path,
    args: &Value,
    export: &Value,
    b0: bool,
    reconstruct: bool,
) -> Result<Value> {
    let prefix = if b0 { "b0" } else { "b1" };
    let product = repository(&path(&args[format!("{prefix}_backend")])?)?;
    let execution = if b0 {
        repository(&path(&args["b0_adapter_backend"])?)?
    } else {
        product.clone()
    };
    let front = repository(&path(&args[format!("{prefix}_frontend")])?)?;
    let product_inventory = inventory(&product, b0.then_some(crate::recovery::b0::BACKEND))?;
    let execution_inventory = inventory(&execution, b0.then_some(crate::recovery::b0::COMMIT))?;
    let frontend_inventory = inventory(&front, b0.then_some(crate::recovery::b0::FRONTEND))?;
    let (build, backend_binding, environment_binding) = backend(
        &execution,
        &path(&args[format!("{prefix}_backend_build")])?,
        &path(&args[format!("{prefix}_backend_environment")])?,
        &execution_inventory,
    )?;
    let frontend_binding = frontend(
        &front,
        &path(&args[format!("{prefix}_frontend_build")])?,
        &frontend_inventory,
        b0,
    )?;
    if b0 {
        let registered = adapter::registered(backend_root, &execution, &product, reconstruct)?;
        if registered["inventory"] != product_inventory
            || registered["execution_inventory"] != execution_inventory
            || build["sources"]
                != source::backend_domains(&source::validate_inventory(&execution_inventory)?)?
        {
            return Err("B0 构建没有绑定原产品及完整适配执行来源".into());
        }
    }
    Ok(
        json!({"roots":{"source_backend":product,"execution_backend":execution,"frontend":front},
        "sources":{"backend":product_inventory,"frontend":frontend_inventory},
        "execution_sources":{"backend":execution_inventory,"frontend":frontend_inventory},
        "builds":{"backend":backend_binding,"frontend":frontend_binding},
        "backend_environment":environment_binding,
        "adapter":if b0 { crate::recovery::b0::evidence()? } else { Value::Null },
        "source_export_result_sha256":export["result"]["sha256"],
        "source_export_identity_sha256":export["identity_sha256"]}),
    )
}
