use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::Result;

use super::{config, contract::Binding, lifecycle::record_failure, process};

/// 回收错误必须留在返回原因链中；没有完整回收证明时不宣称端口已清理。
pub(crate) fn finish(
    binding: &Binding,
    stage: &str,
    primary: Box<dyn std::error::Error>,
    trees: Result<BTreeMap<String, Value>>,
) -> Result<Value> {
    let mut reasons = vec![primary.to_string()];
    let cleanup = trees.and_then(|trees| process::stop_trees(&trees, true));
    let stopped = cleanup.is_ok();
    let completions = match cleanup {
        Ok(value) => value,
        Err(error) => {
            reasons.push(format!("进程树回收未确认：{error}"));
            json!({})
        }
    };
    let ports = config::verify_ports_closed(&binding.value);
    let ports_closed = stopped && ports.is_ok();
    if let Err(error) = ports {
        reasons.push(format!("端口关闭未确认：{error}"));
    }
    if let Err(error) = record_failure(
        &binding.run,
        stage,
        &reasons.join("；"),
        completions,
        ports_closed,
        binding,
    ) {
        reasons.push(format!("失败收据发布失败：{error}"));
    }
    Err(reasons.join("；").into())
}
