//! 核对显式审计文件的完整来源、冻结产物及重复登记身份。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Registered, document, source};
use crate::{
    Result,
    recovery::clone::native::{bridge, digest},
};

pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || !path.is_dir() || digest::linked_path(path)?.iter().any(|v| *v) {
        return Err("来源审计根必须为无链接的绝对目录".into());
    }
    Ok(crate::recovery::fixture_control::portable_path(
        &path.canonicalize()?,
    ))
}

pub(super) fn root(path: &Path, enclosing: &Path) -> Result<PathBuf> {
    let canonical = canonical(path)?;
    let enclosing = crate::recovery::fixture_control::portable_path(&enclosing.canonicalize()?);
    if !canonical.starts_with(enclosing) {
        return Err("嵌套产物来源桥接的根不属于当前运行目录".into());
    }
    Ok(canonical)
}

pub(super) fn load(
    backend: &Path,
    current: &source::Inventory,
    binding: &Value,
    inherited: bool,
) -> Result<Registered> {
    let value = bridge::read_binding(backend, binding)?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "backend_root",
            "build",
            "inventory",
            "original_fingerprints",
            "audited_source",
            "audited_inventory",
            "product_inputs",
            "restore_qualified",
            "compiled",
        ],
        "产物来源桥接",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-artifact-source-bridge"
        || value["restore_qualified"].as_bool() != Some(false)
        || value["compiled"].as_bool() != Some(false)
    {
        return Err("产物来源桥接格式或用途不符".into());
    }
    let declared = root(
        Path::new(document::text(&value["backend_root"], "桥接根")?),
        backend,
    )?;
    let receipt = bridge::read_binding(backend, &value["build"])?;
    let path = Path::new(document::text(&value["build"]["path"], "桥接构建")?);
    bridge::verify_build(backend, path, &receipt)?;
    let inventory_value = bridge::read_binding(backend, &value["inventory"])?;
    let inventory = source::validate_inventory(&inventory_value)?;
    bridge::verify_inventory_source(&inventory, &receipt)?;
    let kind = document::text(&receipt["kind"], "桥接构建类别")?;
    if kind == "restore-backend-build" && receipt["sources"]["full"] != inventory_value {
        return Err("构建收据与来源桥接绑定了不同的构建时 inventory".into());
    }
    let audited = source::validate_inventory(&value["audited_inventory"])?;
    let products = json!({"build":bridge::product_inputs(kind,&inventory)?,
        "audited":bridge::product_inputs(kind,&audited)?});
    if value["original_fingerprints"] != source::fingerprints(&inventory.files)?
        || value["audited_source"] != source::execution_source(&audited)?
        || value["product_inputs"] != products
        || products["build"] != products["audited"]
    {
        return Err("产物来源桥接的产品指纹不匹配".into());
    }
    if !inherited {
        let observed = if declared == backend {
            current.clone()
        } else {
            source::capture(&declared)?
        };
        if audited != observed || value["audited_source"] != source::execution_source(&observed)? {
            return Err("构建审计未绑定本轮完整执行来源".into());
        }
    }
    if bridge::read_binding(backend, binding)? != value {
        return Err("来源桥接核验期间文件变化".into());
    }
    Ok(Registered {
        receipt_hash: bridge::digest::plan_hash(&receipt)?,
        build_identity: build_identity(&receipt)?,
        inventory,
        root: declared,
        inherited,
    })
}

fn build_identity(receipt: &Value) -> Result<String> {
    let mut roles = serde_json::Map::new();
    for (role, artifact) in document::object(&receipt["artifacts"], "来源桥接完整产物")? {
        roles.insert(
            role.clone(),
            json!({"command":artifact["command"],
            "bytes":artifact["bytes"],"sha256":artifact["sha256"]}),
        );
    }
    digest::plan_hash(
        &json!({"kind":receipt["kind"],"source":bridge::build_source(receipt)?,"artifacts":roles}),
    )
}
