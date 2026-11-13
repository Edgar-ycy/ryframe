//! strict v1 完整形状先核验，不能靠重新采集时忽略未知字段。
use super::{digest, document, path, source};
use crate::Result;
use serde_json::{Value, json};

fn sha(value: &Value) -> Result<()> {
    let value = document::text(value, "比较来源摘要")?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("比较来源摘要必须是完整小写 SHA256".into());
    }
    Ok(())
}

fn binding(value: &Value) -> Result<()> {
    document::exact(value, &["path", "bytes", "sha256"], "比较来源文件绑定")?;
    path(&value["path"])?;
    if value["bytes"].as_u64().is_none_or(|n| n == 0) {
        return Err("比较来源绑定字节数必须为正整数".into());
    }
    sha(&value["sha256"])
}

pub(super) fn validate(value: &Value) -> Result<()> {
    document::exact(
        value,
        &["format_version", "kind", "source_export", "arms"],
        "比较来源清单",
    )?;
    if value["format_version"].as_u64() != Some(2) || value["kind"] != "restore-comparison-sources"
    {
        return Err("比较来源清单版本或类型无效".into());
    }
    document::exact(&value["arms"], &["b0", "b1"], "比较来源两侧")?;
    export(&value["source_export"])?;
    for name in ["b0", "b1"] {
        arm(name, &value["arms"][name], &value["source_export"])?;
    }
    Ok(())
}

fn export(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "result",
            "origin_attempt",
            "source_registration",
            "source_rebind",
            "source_generation",
            "review_successor",
            "source_request",
            "export",
            "export_sha256",
            "generation_sha256",
            "logical_inventory_sha256",
            "identity_sha256",
        ],
        "共享导出安全身份",
    )?;
    for field in [
        "result",
        "source_registration",
        "source_rebind",
        "source_generation",
        "review_successor",
        "source_request",
        "export",
    ] {
        binding(&value[field])?;
    }
    if value["origin_attempt"].as_u64().is_none_or(|n| n == 0) {
        return Err("共享导出原 attempt 无效".into());
    }
    for field in [
        "export_sha256",
        "generation_sha256",
        "logical_inventory_sha256",
        "identity_sha256",
    ] {
        sha(&value[field])?;
    }
    let mut unsigned = value.clone();
    unsigned
        .as_object_mut()
        .ok_or("共享导出身份必须为对象")?
        .remove("identity_sha256");
    if value["identity_sha256"] != json!(digest::plan_hash(&unsigned)?) {
        return Err("共享导出安全身份摘要无效".into());
    }
    Ok(())
}

fn arm(name: &str, value: &Value, export: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "roots",
            "sources",
            "execution_sources",
            "builds",
            "backend_environment",
            "adapter",
            "source_export_result_sha256",
            "source_export_identity_sha256",
        ],
        "比较来源单侧",
    )?;
    document::exact(
        &value["roots"],
        &["source_backend", "execution_backend", "frontend"],
        "比较来源根目录",
    )?;
    for key in ["source_backend", "execution_backend", "frontend"] {
        path(&value["roots"][key])?;
    }
    for field in ["sources", "execution_sources"] {
        document::exact(&value[field], &["backend", "frontend"], "比较完整双端来源")?;
        for role in ["backend", "frontend"] {
            source::validate_inventory(&value[field][role])?;
        }
    }
    binding(&value["backend_environment"])?;
    document::exact(&value["builds"], &["backend", "frontend"], "比较构建绑定")?;
    for role in ["backend", "frontend"] {
        binding(&value["builds"][role])?;
    }
    if value["source_export_result_sha256"] != export["result"]["sha256"]
        || value["source_export_identity_sha256"] != export["identity_sha256"]
    {
        return Err("B0/B1 未共同绑定同一已发布导出".into());
    }
    if name == "b0" {
        // 直接复用固定证据的纯字段投影；补丁字节与 Git 基线随后实际核对。
        let adapter = &value["adapter"];
        document::exact(
            adapter,
            &[
                "contract",
                "base_backend_sha",
                "base_frontend_sha",
                "reference_adapter_sha",
                "adapter_tree",
                "reconstructed_tree",
                "adapter_paths",
                "patch",
            ],
            "B0 适配身份",
        )?;
        document::exact(
            &adapter["patch"],
            &["path", "bytes", "sha256"],
            "B0 相对补丁绑定",
        )?;
        if adapter["patch"]["bytes"].as_u64().is_none_or(|n| n == 0) {
            return Err("B0 补丁字节数无效".into());
        }
        if *adapter != crate::recovery::b0::evidence()? {
            return Err("B0 工具适配身份无效".into());
        }
    } else if !value["adapter"].is_null() || value["sources"] != value["execution_sources"] {
        return Err("B1 不得携带 B0 适配或不同执行来源".into());
    }
    Ok(())
}
