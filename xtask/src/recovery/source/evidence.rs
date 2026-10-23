//! 来源验收的原生证据入口；仅从 stdin 接受受限请求，不读取项目私有环境协议。
use std::io::{Read, Write};

use serde_json::{Value, json};

use super::super::clone::native::json as documents;
use crate::Result;

#[path = "evidence/binding.rs"]
mod binding;
#[path = "runtime/cache.rs"]
mod cache;
#[path = "runtime/inventory.rs"]
mod inventory;
#[path = "evidence/lineage.rs"]
mod lineage;
#[path = "evidence/model.rs"]
mod model;
#[path = "runtime/ready.rs"]
mod ready;
#[path = "evidence/reference.rs"]
mod reference;
#[path = "evidence/staging.rs"]
mod staging;
#[cfg(test)]
#[path = "evidence/tests.rs"]
mod tests;

pub(crate) const MARKER: &str = "__source-evidence";
const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_none_or(|arg| arg != MARKER) {
        return None;
    }
    let result = (|| {
        if args.len() != 1 {
            return Err("来源证据只接受标准输入请求".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        execute(&bytes)
    })();
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":{"value":value}})),
        // 原始输入可能包含登录行或意外凭据；解析错误不能带回输入片段。
        Err(error) => (
            1,
            json!({"ok":false,"error":{
            "category":"SourceEvidenceRejected",
            "message":format!("来源证据校验失败：{error}。检查同代路径、已绑定文件及Git来源；保留本地失败现场，创建操作可能留下部分文件，禁止覆盖或自动重放；未执行远端写入。"),
            "remote_write_may_have_occurred":false,"automatic_retry_permitted":false}}),
        ),
    };
    let result = (|| -> Result<()> {
        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        std::io::stdout().lock().write_all(&bytes)?;
        Ok(())
    })();
    Some(if result.is_ok() { code } else { 1 })
}

fn execute(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_BYTES {
        return Err("来源证据请求为空或超过16MiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "来源证据请求必须为UTF-8")?;
    let request = documents::parse_fragment(text, "来源证据请求")
        .map_err(|_| "来源证据请求不是唯一字段的严格JSON")?;
    documents::exact(
        &request,
        &["operation", "arguments", "write"],
        "来源证据请求",
    )?;
    let writes = if request["operation"] == "runtime-generation" {
        crate::recovery::runtime::generation::writes(&request["arguments"])
    } else {
        matches!(
            request["operation"].as_str(),
            Some("staging-create" | "database-configuration" | "reference-sql-normalize")
        )
    };
    if request["write"].as_bool() != Some(writes) {
        return Err("来源证据写入授权与操作不一致".into());
    }
    let args = &request["arguments"];
    match request["operation"].as_str() {
        Some("physical-binding") => binding::physical(args),
        Some("lineage-derive") => lineage::derive(args),
        Some("lineage-verify") => lineage::verify(args),
        Some("redact-database") => binding::redact(args),
        Some("connection-summary") => binding::summary(args),
        Some("database-configuration") => binding::configuration(args),
        Some("result") => model::result(args),
        Some("cache-plan") => model::cache_plan(args),
        Some("login-audit") => model::login_audit(args),
        Some("write-effects") => model::write_effects(args),
        Some("staging-create") => staging::create(args),
        Some("staging-verify") => staging::verify(args),
        Some("runtime-manifest") => inventory::request(args, false),
        Some("producer-inputs") => inventory::request(args, true),
        Some("producer-files") => inventory::files_request(args),
        Some("producer-ready") => ready::verify(args),
        Some("runtime-cache-observation") => cache::observation(args),
        Some("runtime-cache-receipt") => cache::receipt(args),
        Some("runtime-cache-resume") => cache::resume(args),
        Some("runtime-probe") => crate::recovery::runtime::probes::request(args),
        Some("runtime-generation") => crate::recovery::runtime::generation::request(args),
        Some(operation) if operation.starts_with("reference-") => {
            reference::request(operation, args)
        }
        _ => Err("来源证据操作无效".into()),
    }
}
