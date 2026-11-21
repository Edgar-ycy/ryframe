//! 来源验收的原生证据入口；仅从 stdin 接受受限请求，不读取项目私有环境协议。
use std::io::{Read, Write};

use serde_json::{Value, json};

use super::super::clone::native::json as documents;
use crate::Result;

#[path = "evidence/binding.rs"]
pub(crate) mod binding;
#[path = "runtime/cache.rs"]
mod cache;
#[path = "evidence/comparison/mod.rs"]
pub(crate) mod comparison;
#[path = "runtime/control.rs"]
pub(crate) mod control;
#[path = "runtime/environment.rs"]
mod environment;
#[path = "runtime/execution.rs"]
pub(super) mod execution;
#[path = "evidence/generation_request.rs"]
pub(crate) mod generation_request;
#[path = "runtime/images.rs"]
mod images;
#[path = "runtime/inventory.rs"]
mod inventory;
#[path = "evidence/inventory_configuration.rs"]
mod inventory_configuration;
#[path = "evidence/lineage.rs"]
mod lineage;
#[path = "evidence/model.rs"]
mod model;
#[path = "runtime/producer.rs"]
mod producer;
#[path = "runtime/ready.rs"]
mod ready;
#[path = "evidence/reference.rs"]
pub(in crate::recovery) mod reference;
#[path = "evidence/reference_workflow/mod.rs"]
pub(crate) mod reference_workflow;
#[path = "runtime/resources.rs"]
mod resources;
#[path = "runtime/running.rs"]
mod running;
#[path = "runtime/session.rs"]
mod session;
#[path = "evidence/staging.rs"]
mod staging;
#[path = "runtime/stopped.rs"]
mod stopped;
#[cfg(test)]
#[path = "../../../tests/unit/recovery/source/evidence/tests.rs"]
mod tests;

pub(in crate::recovery) use images::saved_object_captures;
pub(in crate::recovery) use images::{inspect_object, object_capture_result};
pub(crate) use ready::policy::cache_transaction::cleanup as cleanup_source_cache;
pub(crate) use running::history::cache_port::dispatch as cache_history;
#[cfg(windows)]
pub(in crate::recovery) use running::history::reboot::{host_boot_proof, verify_host_boot_proof};
pub(in crate::recovery) use running::history::reboot::{
    reboot_close, reboot_process_exited, reboot_status,
};

pub(crate) fn validate_copy_api_environment(args: &Value) -> Result<Value> {
    environment::copy_api(args)
}

pub(crate) fn source_rebind_history(
    controller: &crate::recovery::clone::native::seed::rebind::Controller<'_>,
    descriptor: &Value,
) -> Result<Value> {
    running::history::source_rebind(controller, descriptor)
}

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
        Err(error) => {
            let message = error
                .downcast_ref::<inventory_configuration::PathSyntaxError>()
                .map_or_else(
                    || format!("来源证据校验失败：{error}。检查同代路径、已绑定文件及Git来源；保留本地失败现场，创建操作可能留下部分文件，禁止覆盖或自动重放；未执行远端写入。"),
                    |error| error.0.to_owned(),
                );
            (
                1,
                json!({"ok":false,"error":{
                "category":"SourceEvidenceRejected",
                "message":message,
                "remote_write_may_have_occurred":false,"automatic_retry_permitted":false}}),
            )
        }
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
    let request = parse_request(bytes)?;
    require_write_authorization(&request)?;
    dispatch(&request)
}

fn require_write_authorization(request: &Value) -> Result<()> {
    let writes = if request["operation"]
        .as_str()
        .is_some_and(|v| v.starts_with("comparison-"))
    {
        comparison::writes(
            request["operation"].as_str().ok_or("比较操作无效")?,
            &request["arguments"],
        )?
    } else {
        matches!(
            request["operation"].as_str(),
            Some(
                "runtime-cache-cleanup"
                    | "staging-create"
                    | "database-configuration"
                    | "reference-sql-normalize"
                    | "reference-sql-stream-create"
                    | "reference-sql-stream-close"
                    | "reference-work-directory"
                    | "reference-copy"
                    | "reference-damage"
                    | "reference-receipt-start"
                    | "reference-receipt-finish"
                    | "copy-side-inventory-capture"
            )
        )
    };
    if request["write"].as_bool() != Some(writes) {
        return Err("来源证据写入授权与操作不一致".into());
    }
    Ok(())
}

fn dispatch(request: &Value) -> Result<Value> {
    let args = &request["arguments"];
    match request["operation"].as_str() {
        Some("restore-business-proof") => {
            crate::recovery::runtime::dataset_authority::business::request(args)
        }
        Some("restore-dataset-authority") => {
            crate::recovery::runtime::dataset_authority::request(args)
        }
        Some("runtime-build-read") => crate::recovery::runtime::build::request(args),
        Some("generation-request-rebuild") => generation_request::request(args),
        Some("generation-request-configuration") => generation_configuration(args),
        Some(
            operation @ ("comparison-verify"
            | "comparison-build-source"
            | "comparison-source-export"),
        ) => comparison::request(operation, args),
        Some("recorded-process-stopped") => recorded_process_stopped(args),
        Some("historical-storage-restart-failure") => running::history::restart_request(args),
        Some("source-generation-reboot-observation") => running::history::reboot::request(args),
        Some("source-runtime-stopped") => stopped::request(args),
        Some("copy-schema-catalog") => schema_catalog(args),
        Some("copy-database-observation") => database_observation(args),
        Some("copy-saved-object-captures") => saved_object_captures(args),
        Some("copy-source-generation") => {
            crate::recovery::clone::native::export::proof::copy::request(args)
        }
        Some("copy-side-inventory-capture") => {
            crate::recovery::clone::native::target_verify::database::protocol::request(args)
        }
        Some("copy-side-inventory-verify") => images::inventory::request(args),
        Some("copy-inventory-configuration") => inventory_configuration::request(args),
        Some("copy-schedule-actions") => crate::recovery::clone::native::schedule::request(args),
        Some(operation @ ("business-catalog" | "business-ledger-columns")) => {
            business_catalog(operation, args)
        }
        Some("copy-administrator-credentials") => {
            crate::recovery::clone::native::session::credentials::validate_request(args)
        }
        Some("copy-pacing-binding") => {
            crate::recovery::clone::native::session::pacing_binding::request(args)
        }
        Some("copy-maintenance-root") => {
            crate::recovery::clone::native::maintenance_root::request(args)
        }
        Some("copy-api-environment") => environment::copy_api(args),
        Some("source-inventory") => super::super::clone::native::source::protocol::request(args),
        Some("full-stack-source-evidence") => full_stack_source(args),
        Some("full-stack-binaries-read") => {
            documents::exact(args, &["directory"], "全栈二进制清单读取参数")?;
            let directory = binding::path(&args["directory"])?;
            crate::recovery::fixture_control::artifact::runtime::read_binaries(&directory)
        }
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
        Some("runtime-cache-cleanup") => ready::policy::cache_transaction::cleanup(args),
        Some("runtime-cache-state") => ready::policy::cache_attempt::state(args),
        Some("runtime-cache-observation") => cache::observation(args),
        Some("runtime-cache-receipt") => ready::policy::cache_attempt::receipt(args),
        Some("runtime-cache-resume") => ready::policy::cache_attempt::resume(args),
        Some(operation)
            if operation.starts_with("reference-")
                || operation == "source-reference-projection" =>
        {
            if reference_workflow::handles(operation) {
                reference_workflow::request(operation, args)
            } else {
                reference::request(operation, args)
            }
        }
        _ => Err("来源证据操作无效".into()),
    }
}

fn full_stack_source(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "directory", "ci"], "全栈来源事实参数")?;
    documents::exact(&args["ci"], &["run_id", "attempt"], "全栈来源CI身份")?;
    for field in ["run_id", "attempt"] {
        if !args["ci"][field].is_null() && !args["ci"][field].is_string() {
            return Err("全栈来源CI身份必须为文本或空值".into());
        }
    }
    let backend = binding::path(&args["backend"])?;
    let directory = binding::path(&args["directory"])?;
    let current = crate::recovery::full_stack::provenance::current(&backend, &directory)?;
    if !current.is_null()
        && !matches!(
            current["source_pair"]["kind"].as_str(),
            Some("reference-fixture-source-pair" | "reference-fixture-execution-source-pair")
        )
    {
        let expected =
            ["run_id", "attempt"].map(|field| args["ci"][field].as_str().map(std::ffi::OsStr::new));
        crate::recovery::full_stack::provenance::validate_ci_identity(
            &current["source_pair"],
            expected,
        )?;
    }
    Ok(current)
}

fn generation_configuration(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "request"], "来源配置读取参数")?;
    let backend = binding::path(&args["backend"])?;
    generation_request::configuration::execution_source(&backend, &args["request"])?;
    generation_request::configuration::verify(&backend, &args["request"])
}

fn recorded_process_stopped(args: &Value) -> Result<Value> {
    documents::exact(args, &["identity"], "历史进程停止核验参数")?;
    let backend = crate::workspace::root_dir();
    let tool = crate::recovery::fixture_control::portable_path(&std::env::current_exe()?);
    super::super::clone::native::seed::source_registration::quiet::require_stopped(
        &backend,
        &backend.join(".local-tests"),
        &args["identity"],
        &tool,
    )?;
    Ok(json!({"stopped":true,"signals_sent":0}))
}

fn database_observation(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "value", "declared", "scope", "ownership"],
        "复制数据库观察参数",
    )?;
    let backend = binding::path(&args["backend"])?;
    let scope = documents::text(&args["scope"], "数据库观察 scope")?;
    let ownership = documents::array(&args["ownership"], "数据库观察 owner")?;
    crate::recovery::clone::native::target_verify::database::model::observation(
        &backend,
        &args["value"],
        &args["declared"],
        scope,
        ownership,
    )
}

fn schema_catalog(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "include_full"], "复制schema目录参数")?;
    let backend = binding::path(&args["backend"])?;
    let include_full = documents::boolean(&args["include_full"], "include_full")?;
    let loaded = crate::recovery::clone::native::catalog::load_ordered(&backend, include_full)?;
    Ok(match loaded.full {
        Some((full_control, full_tenant)) => json!({
            "control": loaded.control, "tenant": loaded.tenant,
            "full_control": full_control, "full_tenant": full_tenant,
        }),
        None => json!({"control": loaded.control, "tenant": loaded.tenant}),
    })
}

fn business_catalog(operation: &str, args: &Value) -> Result<Value> {
    documents::exact(args, &["backend"], "业务目录参数")?;
    let backend = binding::path(&args["backend"])?;
    use crate::recovery::clone::native::business;
    if operation == "business-catalog" {
        let loaded = business::load(&backend)?;
        let columns = loaded.ordered_columns()?;
        Ok(json!({"tables": loaded.tables, "columns": columns}))
    } else {
        Ok(serde_json::to_value(business::ledger_column_pairs(
            &backend,
        )?)?)
    }
}

fn parse_request(bytes: &[u8]) -> Result<Value> {
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
    Ok(request)
}
