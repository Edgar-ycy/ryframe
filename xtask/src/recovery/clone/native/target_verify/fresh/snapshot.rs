//! status 与正式 verify 共用原阶段释放快照合同，不授予直接目标工作区要求。
use super::super::super::{json as document, resources::common};
use super::super::tree;
use crate::Result;
use serde_json::Value;
use std::path::Path;

pub(super) fn verify(
    backend: &Path,
    workspace: &Path,
    target: &Path,
    registration: &Value,
    stage: &str,
) -> Result<Value> {
    let path = workspace.join(format!("{stage}-files.json"));
    let descriptor = common::binding(&path)?;
    let value = common::read_bound(backend, &descriptor, "fresh 阶段快照")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "fresh 阶段快照",
    )?;
    let predecessor = if stage == "initialized" {
        common::binding(&workspace.join("prepared-files.json"))?
    } else {
        Value::Null
    };
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != format!("devex-clone-fresh-target-{stage}-files")
        || value["registration"] != *registration
        || value["predecessor"] != predecessor
        || value["files"] != tree::snapshot(backend, target, false)?
        || common::binding(&path)? != descriptor
    {
        return Err("fresh 阶段文件集合、内容或登记发生变化".into());
    }
    Ok(descriptor)
}
