//! 固定失败采集所用的实际大二进制逐字节摘要核对；文档限额不套到产物。
use super::{builds, data};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn verify(
    backend: &Path,
    output: &Path,
    sources: &Value,
    observed: bool,
) -> Result<(Value, Value)> {
    let binaries = document::read(&output.join("runtime/binaries.json"))?;
    let command = document::read(&output.join(if observed {
        "before/inventory-before.command.json"
    } else {
        "before/migrations-control.command.json"
    }))?;
    let binary = Path::new(document::text(
        if observed {
            &command["command"][0]
        } else {
            &binaries["ryframe-migrate"]
        },
        "原只读工具",
    )?);
    let path = binary
        .parent()
        .ok_or("原工具缺少维护目录")?
        .join("build.json");
    let maintenance = common::binding(&path)?;
    let receipt = common::read_bound(backend, &maintenance, "原只读维护构建")?;
    let artifact = &receipt["artifacts"][if observed { "tenant-data" } else { "migrate" }];
    let actual = digest::file(binary)?;
    if maintenance["sha256"]
        != if observed {
            data::OBSERVATION_MAINTENANCE
        } else {
            data::COLLECTION_MAINTENANCE
        }
        || artifact["executable"] != json!(binary)
        || artifact["bytes"] != actual.bytes
        || artifact["sha256"] != actual.sha256
    {
        return Err("原采集命令未绑定同源已审计的只读维护产物".into());
    }
    if observed {
        observation(backend, output, &receipt, &command, binary)?;
    } else if receipt["source"]
        != json!({"snapshot":sources["snapshot"],"worktree_fingerprint":sources["worktree_fingerprint"]})
        || command
            != json!({"command":[binary,"control","verify"],"cwd":backend,"remote_operations":"read_only"})
    {
        return Err("原 control verify 不属于原来源和固定只读命令".into());
    }
    common::bound_file(backend, &maintenance, "原只读维护构建")?;
    if digest::file(binary)? != actual {
        return Err("原只读二进制在核验期间变化".into());
    }
    Ok((maintenance, artifact.clone()))
}

fn observation(
    backend: &Path,
    output: &Path,
    receipt: &Value,
    command: &Value,
    binary: &Path,
) -> Result<()> {
    let snapshot = &receipt["source"]["snapshot"];
    let arguments = document::array(&command["command"], "原库存失败命令")?;
    let timestamp = arguments.last().ok_or("原库存失败缺少观察时间")?;
    let diagnostic = document::read(&output.join("before/inventory-before.diagnostic.json"))?;
    if snapshot["clean"] != true
        || snapshot["files"] != json!([])
        || snapshot["patch_sha256"] != hex::encode(sha2::Sha256::digest(b""))
        || builds::git_tree(backend, document::text(&snapshot["head"], "原产品 SHA")?)?
            != data::OBSERVATION_PRODUCT_TREE
        || *command
            != json!({"command":[binary,"backup-inventory","--output",output.join("before/inventory-before.json"),
            "--source-sha",snapshot["head"],"--observed-at",timestamp],"cwd":receipt["backend_root"],"remote_operations":"read_only"})
        || diagnostic["error_type"] != "CalledProcessError"
        || diagnostic["returncode"].as_i64() != Some(1)
        || diagnostic["stdout"] != ""
        || !document::text(&diagnostic["stderr"], "原参数拒绝诊断")?
            .starts_with("Error: Validation(\"缺少 --quiesced-at\\n")
    {
        return Err("原库存失败没有证明停止于已审计二进制的参数解析边界".into());
    }
    Ok(())
}

use sha2::Digest;
