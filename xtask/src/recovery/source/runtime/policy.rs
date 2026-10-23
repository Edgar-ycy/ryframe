//! 来源运行纯策略。通过现有生产者证据入口分派，不引入新的环境或命令入口。
use super::super::documents;
use crate::Result;
use serde_json::Value;
#[path = "policy/audit.rs"]
mod audit;
#[path = "policy/cache_io.rs"]
mod cache_io;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/source/runtime/policy/evidence_tests.rs"]
mod evidence_tests;
#[path = "policy/flow.rs"]
mod flow;
#[path = "policy/model.rs"]
mod model;
#[path = "policy/operations.rs"]
mod operations;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/source/runtime/policy/operations_tests.rs"]
mod operations_tests;
#[path = "policy/producer.rs"]
mod producer;
#[path = "policy/receipts.rs"]
mod receipts;
#[path = "policy/recovery.rs"]
mod recovery;
#[path = "policy/recovery_create.rs"]
mod recovery_create;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/source/runtime/policy/tests.rs"]
mod tests;

pub(super) fn request(args: &Value) -> Result<Value> {
    documents::exact(args, &["action", "arguments"], "来源运行策略")?;
    let input = &args["arguments"];
    match args["action"].as_str() {
        Some("cache-read-plans") => cache_io::read_plans(input),
        Some("cache-observe") => cache_io::observation(input),
        Some("cache-command") => cache_io::command(input),
        Some("cache-result") => cache_io::result(input),
        Some("cache-keys") => cache_io::keys(input),
        Some("control-database") => operations::control_database(input),
        Some("paths") => operations::paths(input),
        Some("login-query") => operations::query(input),
        Some("login-row-evidence") => operations::rows(input),
        Some("image-binding") => operations::image(input),
        Some("facts-binding") => operations::facts_binding(input),
        Some("cache-effects") => operations::effects(input),
        Some("login-create") => audit::create(input),
        Some("login-stored") => audit::stored(input),
        Some("login-recheck") => audit::recheck(input),
        Some("failed-entries") => recovery_create::failed_entries(input),
        Some("recovery-prefix") => recovery_create::prefix(input),
        Some("recovery-intent") => recovery_create::intent(input),
        Some("recovery-receipt") => recovery_create::receipt(input),
        Some("published") => recovery_create::published(input),
        Some("login-rows") => model::login_rows(input),
        Some("snapshot") => model::snapshot(input),
        Some("remote-writes") => model::remote_writes(input),
        Some("time-order") => model::time_order(input),
        Some("producer-operation") => producer::validate_operation(input),
        Some("producer-command") => producer::command(input),
        Some("producer-registration") => producer::registration(input),
        Some("producer-completion") => producer::completion(input),
        Some("recovery-pending") => recovery::pending(input),
        Some("recovery-chain") => recovery::chain(input),
        Some("cache-publication") => recovery::cache_publication(input),
        Some("runtime-receipt") => receipts::create(input),
        Some("runtime-final") => receipts::final_check(input),
        Some("runtime-facts") => recovery::facts(input),
        Some("runtime-schema") => receipts::schema(input),
        Some("login-evidence") => receipts::login(input),
        Some("producer-bindings") => {
            documents::exact(input, &["backend", "directory"], "来源生产者文件绑定参数")?;
            let (_, directory) = receipts::directory(input)?;
            receipts::producer_bindings(&directory)
        }
        Some("receipt-fields") => {
            documents::exact(input, &["recovered"], "来源运行字段参数")?;
            let mut fields = receipts::RECEIPT.to_vec();
            if documents::boolean(&input["recovered"], "来源恢复标志")? {
                fields.push("recovery");
            }
            Ok(serde_json::json!(fields))
        }
        Some("flow") => flow::plan(input),
        _ => Err("来源运行策略操作无效".into()),
    }
}
