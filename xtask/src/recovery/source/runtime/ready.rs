//! ready 只证明预载完成；同代绑定全部成立后才能发送启动授权。
use super::documents;
use crate::{Result, recovery::clone::native::digest};
use serde_json::{Value, json};

pub(super) fn verify(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["raw", "operation", "start", "lineage", "staging"],
        "来源生产者ready参数",
    )?;
    let raw = documents::text(&args["raw"], "来源生产者ready")?;
    if raw.is_empty()
        || raw.len() > 64 * 1024
        || !raw.ends_with('\n')
        || raw[..raw.len() - 1].contains('\n')
    {
        return Err("来源验收原生生产者 ready 不是有界单行证据".into());
    }
    let value = documents::parse_fragment(raw.trim(), "来源验收原生生产者ready")
        .map_err(|_| "来源验收原生生产者ready不是唯一字段JSON")?;
    let staging = &args["staging"];
    let files = documents::array(&staging["manifest"]["files"], "来源staging文件")?;
    let contract = files
        .iter()
        .find(|row| row["path"] == staging["manifest"]["contract"]);
    let expected = json!({
        "format_version":1,"kind":"restore-source-producer-ready","operation_id":args["operation"],
        "source_generation_sha256":args["start"]["sha256"],
        "staging_manifest_sha256":staging["descriptor"]["sha256"],
        "runtime_files_sha256":digest::plan_hash(&staging["manifest"]["runtime_modules"])?,
        "contract_file_sha256":contract.map(|row| &row["sha256"]),
        "lineage_file_sha256":args["lineage"]["sha256"],
    });
    if value != expected || value["format_version"].as_u64() != Some(1) {
        return Err("来源验收原生 ready 未绑定工具、契约、血缘或运行身份".into());
    }
    Ok(value)
}
