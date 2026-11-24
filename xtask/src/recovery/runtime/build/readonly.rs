//! 共享Python消费者只调用此只读端口；不编译、不写Git或收据。
use super::{Native, source, verify};
use crate::{
    Result,
    recovery::clone::native::{bridge, json as document, maintenance},
};
use serde_json::{Value, json};

pub(in crate::recovery) fn request(args: &Value) -> Result<Value> {
    document::exact(args, &["action", "root", "value"], "恢复构建只读参数")?;
    let root = source::path(&args["root"])?;
    let value = &args["value"];
    match document::text(&args["action"], "恢复构建只读操作")? {
        "repository" => Ok(json!(source::repository(root)?)),
        "context" => {
            require_null(value)?;
            bridge::build_context::current(root)
        }
        "context-model" => {
            bridge::validate_build_context(value)?;
            Ok(value.clone())
        }
        "command" => Ok(json!(bridge::build_command(
            document::text(value, "构建角色")?,
            bridge::build_jobs(root)?
        )?)),
        "jobs" => {
            require_null(value)?;
            Ok(json!(bridge::build_jobs(root)?))
        }
        "maintenance-command" => {
            document::exact(value, &["role", "target"], "维护构建命令输入")?;
            Ok(json!(maintenance::command(
                document::text(&value["role"], "维护角色")?,
                std::path::Path::new(document::text(&value["target"], "维护 target")?),
                bridge::build_jobs(root)?,
            )?))
        }
        "maintenance-evidence" => {
            document::exact(value, &["filename", "receipt"], "维护构建核验输入")?;
            maintenance::verify_evidence(
                root,
                source::path(&value["filename"])?,
                (!value["receipt"].is_null()).then_some(&value["receipt"]),
            )
        }
        "artifacts" => {
            bridge::verify_restore_artifacts(value)?;
            Ok(json!({"verified":true}))
        }
        "verify" => {
            document::exact(value, &["receipt", "expected_head"], "构建复核输入")?;
            verify(
                root,
                &value["receipt"],
                document::text(&value["expected_head"], "构建精确SHA")?,
                &mut Native,
            )?;
            Ok(json!({"verified":true}))
        }
        "registered" => {
            document::exact(
                value,
                &[
                    "coordinator",
                    "expected_head",
                    "adapter_contract",
                    "product_backend",
                ],
                "登记来源输入",
            )?;
            source::registered(
                source::path(&value["coordinator"])?,
                root,
                document::text(&value["expected_head"], "来源精确SHA")?,
                &value["adapter_contract"],
                &value["product_backend"],
            )
        }
        _ => Err("未知恢复构建只读操作".into()),
    }
}

fn require_null(value: &Value) -> Result<()> {
    if value.is_null() {
        Ok(())
    } else {
        Err("构建上下文采集不接受自报值".into())
    }
}
