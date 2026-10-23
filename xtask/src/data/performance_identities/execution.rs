use super::{
    ledger::Ledger,
    plan::{array, text},
    receipt,
};
use crate::Result;
use serde_json::{Value, json};
use std::{future::Future, path::Path, pin::Pin};

#[path = "execution/apply.rs"]
mod apply;
#[path = "execution/verify.rs"]
mod verify;
pub(super) type IdentityFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + 'a>>;
pub(super) trait IdentitySession {
    fn login(&mut self) -> IdentityFuture<'_, ()>;
    fn request<'a>(&'a mut self, step: &'a Value) -> IdentityFuture<'a, Value>;
    fn template<'a>(&'a mut self, path: &'a Path) -> IdentityFuture<'a, Value>;
    fn template_data(&mut self) -> IdentityFuture<'_, Value>;
    fn token(&self) -> Option<&str>;
    fn close(&mut self) -> IdentityFuture<'_, ()>;
}
pub(super) trait Context {
    fn password<'a>(&'a mut self, slot: &'a str) -> IdentityFuture<'a, String>;
    fn inspect<'a>(&'a mut self, phase: &'a str) -> IdentityFuture<'a, Value>;
    fn session<'a>(
        &'a mut self,
        identity: &'a Value,
    ) -> IdentityFuture<'a, Box<dyn IdentitySession>>;
}
pub(super) async fn execute(
    plan: &Value,
    root: &Path,
    mode: &str,
    token: &str,
    context: &mut dyn Context,
    checkpoint: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Value> {
    let mut ledger = Ledger::open(plan, root, mode, token)?;
    let result = async {
        if mode == "verify" {
            if let Some(receipt) = ledger.resume()? {
                return Ok(summary(plan, &receipt));
            }
            ledger.begin_verify()?;
        }
        let before = context.inspect(mode).await?;
        if before["scope_id"] != plan["environment"]["scope_id"]
            || before["server_uuid"] != plan["environment"]["database"]["server_uuid"]
        {
            return Err("身份检查收据绑定错误".into());
        }
        let result = if mode == "apply" {
            apply::run(plan, &mut ledger, context, &before).await?
        } else {
            verify::run(plan, &mut ledger, context).await?
        };
        let after = context.inspect("verify").await?;
        if after["scope_id"] != before["scope_id"]
            || after["server_uuid"] != before["server_uuid"]
            || after["api_process"] != before["api_process"]
        {
            return Err("身份准备期间 API 进程或资源绑定变化".into());
        }
        if mode == "apply" {
            ledger.prepared()
        } else {
            ledger.publish(&result, checkpoint)?;
            Ok(summary(plan, &result))
        }
    }
    .await;
    let closed = ledger.close();
    match (result, closed) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}
fn summary(plan: &Value, receipt: &Value) -> Value {
    json!({"status":receipt["status"],"plan_sha256":plan["plan_sha256"],"users":200,"tenants":10,"message_audience":10})
}

pub(super) async fn close<T>(session: &mut dyn IdentitySession, result: Result<T>) -> Result<T> {
    let closed = session.close().await;
    match (result, closed) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}
pub(super) async fn request(
    session: &mut dyn IdentitySession,
    operation: &str,
    path: Value,
    query: Value,
) -> Result<Value> {
    let mut step = json!({"operation":operation});
    if !path.is_null() {
        step["path"] = path;
    }
    if !query.is_null() {
        step["query"] = query;
    }
    Ok(session.request(&step).await?["data"].clone())
}
pub(super) fn same_set(actual: &Value, expected: &Value) -> bool {
    fn convert(value: &Value) -> Option<std::collections::BTreeSet<&str>> {
        let values = value.as_array()?;
        let selected = values
            .iter()
            .map(Value::as_str)
            .collect::<Option<std::collections::BTreeSet<_>>>()?;
        (selected.len() == values.len()).then_some(selected)
    }
    match (convert(actual), convert(expected)) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => false,
    }
}
pub(super) fn permission_ids(tree: &Value, required: &Value) -> Result<Value> {
    fn visit(
        tree: &Value,
        required: &[Value],
        found: &mut std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        for item in array(tree)? {
            if required.contains(&item["code"]) {
                let code = text(item, "code")?;
                let id = crate::data::performance_identities::state::id(&item["id"])?;
                if item["status"] != "1" || found.insert(code.into(), id.into()).is_some() {
                    return Err("权限码重复或不可用".into());
                }
            }
            visit(&item["children"], required, found)?;
        }
        Ok(())
    }
    let required = array(required)?;
    let mut found = std::collections::BTreeMap::new();
    visit(tree, required, &mut found)?;
    if found.len() != required.len() {
        return Err("权限目录缺少固定最小权限".into());
    }
    Ok(json!(
        required
            .iter()
            .map(|code| found
                .get(code.as_str().unwrap_or(""))
                .cloned()
                .ok_or("权限码缺失"))
            .collect::<std::result::Result<Vec<_>, _>>()?
    ))
}
