//! 来源运行纯策略。通过现有生产者证据入口分派，不引入新的环境或命令入口。
use super::super::documents;
use crate::Result;
use serde_json::Value;
#[cfg(test)]
#[path = "policy/evidence_tests.rs"]
mod evidence_tests;
#[path = "policy/flow.rs"]
mod flow;
#[path = "policy/model.rs"]
mod model;
#[path = "policy/producer.rs"]
mod producer;
#[path = "policy/receipts.rs"]
mod receipts;
#[path = "policy/recovery.rs"]
mod recovery;
#[cfg(test)]
#[path = "policy/tests.rs"]
mod tests;

pub(super) fn request(args: &Value) -> Result<Value> {
    documents::exact(args, &["action", "arguments"], "来源运行策略")?;
    let input = &args["arguments"];
    match args["action"].as_str() {
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
        Some("flow") => flow::plan(input),
        _ => Err("来源运行策略操作无效".into()),
    }
}
