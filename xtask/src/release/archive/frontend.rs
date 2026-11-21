use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    Result,
    recovery::{clone::native::source, frontend_build::validate_ci_receipt},
};

use super::super::{
    evidence::EMPTY_SHA256,
    support::{commit_shape, exact_object, string_field},
};
use super::{FRONTEND_BUILD, FRONTEND_SOURCE, join_posix, parent, validate_binding};

pub(super) fn validate(
    receipts: &BTreeMap<String, Value>,
    build_raw: &[u8],
    source: &Value,
    backend_root: &str,
    frontend_sha: &str,
    receipt_root: &str,
) -> Result<Value> {
    let receipt = &receipts[FRONTEND_SOURCE];
    exact_object(
        receipt,
        &[
            "format_version",
            "kind",
            "source_pair",
            "fixture",
            "generated",
            "published",
            "build",
        ],
        "归档业务前端来源收据",
    )?;
    if receipt["format_version"].as_u64() != Some(1)
        || receipt["kind"] != "generated-business-frontend-build"
        || receipt["source_pair"] != source["source_pair"]["receipt"]
        || receipt["fixture"] != source["fixture_receipt"]
        || receipt["generated"] != source["generated"]["frontend"]
    {
        return Err("归档业务前端来源收据与原源码组合、生成夹具或快照不匹配".into());
    }
    let build = validate_binding(
        &receipt["build"],
        "归档业务前端构建收据绑定",
        Some(FRONTEND_BUILD),
        Some(build_raw),
    )?;
    if parent(&build.path) != receipt_root {
        return Err("归档业务前端构建收据与运行收据不在同一目录".into());
    }
    let published = &receipt["published"];
    let head = validate_published(published, &receipt["generated"], backend_root, frontend_sha)?;
    validate_ci_receipt(&receipts[FRONTEND_BUILD], &published["inventory"], head)?;
    Ok(receipt.clone())
}

fn validate_published<'a>(
    value: &'a Value,
    generated: &Value,
    backend_root: &str,
    frontend_sha: &str,
) -> Result<&'a str> {
    let label = "归档业务前端发布来源";
    let published = exact_object(
        value,
        &[
            "path",
            "head",
            "parent",
            "tree",
            "inventory",
            "fingerprints",
        ],
        label,
    )?;
    let head = string_field(published, "head", label)?;
    let expected_path = join_posix(backend_root, ".local-tests/full-stack-frontend");
    if value["path"].as_str() != Some(expected_path.as_str())
        || value["parent"].as_str() != Some(frontend_sha)
        || !commit_shape(Some(head))
        || head == frontend_sha
        || !commit_shape(value["tree"].as_str())
    {
        return Err("归档业务前端的发布目录、HEAD、原父提交或树摘要无效".into());
    }
    let inventory = source::validate_inventory(&value["inventory"])?;
    let snapshot = &inventory.source.snapshot;
    if snapshot.head != head
        || !snapshot.clean
        || snapshot.patch_sha256 != EMPTY_SHA256
        || !snapshot.files.is_empty()
        || value["fingerprints"] != source::fingerprints(&inventory.files)?
    {
        return Err("归档业务前端发布库存与干净 HEAD 或分组指纹不符".into());
    }
    let files = generated["files"]
        .as_array()
        .ok_or("归档业务前端生成文件清单无效")?;
    let actual = inventory
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    for file in files {
        let path = file["path"]
            .as_str()
            .ok_or("归档业务前端生成文件路径无效")?;
        let digest = file["sha256"]
            .as_str()
            .ok_or("归档业务前端生成文件摘要无效")?;
        if actual.get(path).copied() != Some(digest) {
            return Err("归档业务前端发布库存缺少生成文件或摘要不匹配".into());
        }
    }
    Ok(head)
}
