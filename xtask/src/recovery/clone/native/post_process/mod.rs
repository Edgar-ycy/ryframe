//! 复制后处理生产者的计划、证据核验、精确回收和私有协议。

pub(crate) mod command;
mod inspection;
mod lock;
pub(crate) mod protocol;
pub(crate) mod registration;
mod runtime;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/tests.rs"]
mod tests;

pub(crate) fn runtime_proof(request: &serde_json::Value) -> crate::Result<serde_json::Value> {
    runtime::proof(request)
}
