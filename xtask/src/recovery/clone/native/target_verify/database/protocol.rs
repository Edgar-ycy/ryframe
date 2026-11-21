//! 只接受冻结文件绑定的四目标只读采集；本地证据写入不授予远端写入资格。
use std::{collections::BTreeMap, fs};

use serde_json::{Value, json};

use super::{SourceBinding, capture_created};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        bridge::scope::Scope, export::transport::SystemTransport, json as document,
        target_verify::storage::StorageInputs,
    },
};

#[path = "protocol/input.rs"]
mod input;

#[cfg(test)]
pub(crate) use input::Inputs as TestInputs;

pub(crate) fn request(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "evidence_root",
            "backend",
            "side",
            "plan",
            "maintenance",
            "configuration",
            "app_env",
            "output",
        ],
        "四目标库存采集参数",
    )?;
    let evidence = input::directory(&args["evidence_root"])?;
    let output = crate::recovery::source::evidence::binding::path(&args["output"])?;
    validate_local_test_path(&output, &evidence, LocalTestPathKind::NewDirectory)?;
    if !output.parent().is_some_and(|parent| parent.is_dir()) {
        return Err("库存输出须使用已有父目录中的新目录".into());
    }
    fs::create_dir(&output)?;
    let result = collect(args, &evidence, &output);
    if result.is_err() && !output.join("failure.json").try_exists()? {
        document::write_new(
            &output.join("failure.json"),
            &json!({
                "format_version":1,"status":"side_inventory_failed","stage":"inputs",
                "error_type":"RustError","remote_writes":0,"target_ready":false,"clone_verified":false,
            }),
        )?;
    }
    result
}

fn collect(args: &Value, evidence: &std::path::Path, output: &std::path::Path) -> Result<Value> {
    let input = input::Inputs::load(args, evidence)?;
    // Scope 自行观察执行树及真实维护来源，不能使用请求内的摘要替代当前源码核验。
    let scope = Scope::load(&input.backend, &[], &[])?;
    let private = BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let context = StorageInputs {
        backend: evidence,
        request: &input.port,
        environment: &private,
        observation_dir: output,
    };
    let source = SourceBinding {
        side: input.side,
        plan: Some(&args["plan"]),
        selected: &args["plan"][input.side.as_str()],
        tools: &args["plan"]["tools"],
        maintenance: &args["maintenance"],
        scope: &scope,
        configuration: Some(&args["configuration"]),
    };
    let checkpoint = || {
        input.verify(args, evidence)?;
        scope.assert_unchanged()
    };
    let captured = capture_created(
        &context,
        output,
        &SystemTransport,
        &checkpoint,
        Some(&source),
        false,
    )?;
    // 响应只携带既有有界收据；完整逐表数据继续保存在原八份文件中，不截断内容。
    Ok(
        json!({"side": input.side.as_str(), "binding":captured.binding, "receipt":captured.inventory,
        "receipt_file":captured.receipt,"evidence_files":captured.evidence_files}),
    )
}
