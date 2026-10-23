//! 固定阶段图只有原生一份；Python适配器只执行锁、资源和进程端口。
use super::documents;
use crate::Result;
use serde_json::{Value, json};

pub(super) fn plan(args: &Value) -> Result<Value> {
    documents::exact(args, &["mode"], "来源运行阶段参数")?;
    let stages: &[&str] = match args["mode"].as_str() {
        Some("verify") => &[
            "prepare",
            "login-before",
            "checkpoint",
            "capture-before",
            "checkpoint",
            "producer",
            "checkpoint",
            "login-after",
            "cache-cleanup",
            "checkpoint",
            "capture-after",
            "effects",
            "receipt",
            "checkpoint",
            "publish",
            "verify-published",
            "checkpoint",
        ],
        Some("recover") => &[
            "recovery-intent",
            "checkpoint",
            "login-recheck",
            "cache-resume",
            "checkpoint",
            "capture-after",
            "login-recheck",
            "effects",
            "recovery-receipt",
            "receipt",
            "checkpoint",
            "publish",
            "verify-published",
            "checkpoint",
        ],
        _ => return Err("来源运行阶段模式无效".into()),
    };
    Ok(json!(stages))
}
