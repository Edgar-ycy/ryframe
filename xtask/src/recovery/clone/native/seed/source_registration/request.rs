//! 派生seed源请求的纯构造；原始文件、历史及实际产物验证由父调用方负责。
use std::path::Path;

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    recovery::clone::native::{
        digest,
        input::{digest_shape, name},
        json as documents,
        lifecycle::manifest,
    },
};

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/request.rs"]
mod tests;

pub(crate) struct Inputs<'a> {
    pub(crate) manifest: &'a Value,
    pub(crate) original_request: &'a Value,
    pub(crate) target_request: &'a Value,
    pub(crate) post_request: &'a Value,
    pub(crate) runtime: &'a Path,
    pub(crate) runtime_binding: &'a Value,
    pub(crate) handoff: &'a Value,
    pub(crate) registry: &'a Value,
    pub(crate) registry_binding: &'a Value,
    pub(crate) maintenance: &'a Value,
    pub(crate) process_bindings: &'a Value,
}

pub(crate) fn build(input: Inputs<'_>) -> Result<Value> {
    let id = source_id(documents::text(&input.manifest["id"], "原run ID")?)?;
    let target = &input.target_request["target"];
    let mut source =
        documents::exact(target, &["scope_id", "s3", "databases"], "目标source配置")?.clone();
    let scope = name(documents::text(&target["scope_id"], "目标scope")?)?;
    documents::object(&target["s3"], "目标对象配置")?;
    documents::array(&target["databases"], "目标数据库")?;
    let runtime = absolute_path(input.runtime)?;
    let api_url = documents::text(&input.handoff["api_url"], "handoff API地址")?;
    if api_url.is_empty() {
        return Err("handoff缺少API地址".into());
    }
    source.insert("runtime_dir".into(), json!(runtime));
    source.insert("api_url".into(), json!(api_url));
    runtime_bindings(&input)?;
    let backend_build = &input.handoff["backend_build"];
    let maintenance_build = &input.target_request["maintenance_build"];
    for value in [backend_build, maintenance_build, input.registry_binding] {
        binding(value)?;
    }
    let fingerprint = documents::text(
        &input.maintenance["source"]["worktree_fingerprint"],
        "维护构建来源指纹",
    )?;
    if !Regex::new(r"^sha256:[a-f0-9]{64}$")?.is_match(fingerprint) {
        return Err("目标维护构建缺少产品来源指纹".into());
    }
    if input.original_request["max_object_bytes"]
        .as_u64()
        .is_none()
    {
        return Err("原请求对象大小上限必须是非负整数".into());
    }
    if documents::text(&input.registry["scope_id"], "registry scope")? != scope
        || digest_shape(&input.registry["runtime_sha256"])?
            != digest_shape(&input.runtime_binding["sha256"])?
    {
        return Err("producer registry不属于派生seed源运行".into());
    }
    let mysql = tool(&input.target_request["tools"]["mysql"])?;
    let mysqldump = tool(&input.original_request["tools"]["mysqldump"])?;
    let node = tool(&input.post_request["node"])?;
    Ok(
        json!({"format_version":1,"kind":"devex-clone-source-export","id":id,
        "source":source,"tools":{"mysql":mysql,"mysqldump":mysqldump,"node":node},
        "backend_build":backend_build,"maintenance_build":maintenance_build,
        "worktree_fingerprint":fingerprint,"runtime":input.runtime_binding,
        "processes":input.process_bindings,"producers_registry":input.registry_binding,
        "max_object_bytes":input.original_request["max_object_bytes"]}),
    )
}

fn runtime_bindings(input: &Inputs<'_>) -> Result<()> {
    binding(input.runtime_binding)?;
    if Path::new(documents::text(
        &input.runtime_binding["path"],
        "runtime绑定路径",
    )?) != input.runtime.join("runtime.json")
    {
        return Err("runtime绑定不属于派生源运行目录".into());
    }
    documents::exact(input.process_bindings, &["api", "worker"], "派生源进程绑定")?;
    for role in ["api", "worker"] {
        let value = &input.process_bindings[role];
        binding(value)?;
        if Path::new(documents::text(&value["path"], "进程收据路径")?)
            != input.runtime.join(format!("{role}.json"))
        {
            return Err("进程收据不属于派生源运行目录".into());
        }
    }
    Ok(())
}

fn source_id(original: &str) -> Result<String> {
    let original = name(original)?;
    let candidate = format!("{original}-seed");
    let id = if candidate.len() <= 48 {
        candidate
    } else {
        format!(
            "seed-{}",
            &hex::encode(Sha256::digest(original.as_bytes()))[..32]
        )
    };
    name(&id)?;
    Ok(id)
}

fn absolute_path(path: &Path) -> Result<String> {
    let text = digest::path_text(path, "派生源路径")?;
    if !path.is_absolute()
        || text.chars().any(char::is_control)
        || text
            .split(['/', '\\'])
            .any(|part| matches!(part, "." | ".."))
    {
        return Err("派生源绑定必须使用明确绝对路径".into());
    }
    Ok(text)
}

fn binding(value: &Value) -> Result<()> {
    manifest::validate_binding_shape(value, "派生源文件绑定")?;
    absolute_path(Path::new(documents::text(
        &value["path"],
        "派生源绑定路径",
    )?))?;
    Ok(())
}

fn tool(value: &Value) -> Result<&Value> {
    documents::exact(value, &["path", "sha256"], "派生源工具")?;
    absolute_path(Path::new(documents::text(
        &value["path"],
        "派生源工具路径",
    )?))?;
    digest_shape(&value["sha256"])?;
    Ok(value)
}
