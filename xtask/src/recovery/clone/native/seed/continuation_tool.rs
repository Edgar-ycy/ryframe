//! 续作工具绑定到本次控制器、已冻结产物及通过的实际构建结果。
use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common, target_verify::input::request},
};

pub(crate) fn bind(
    backend: &Path,
    path: &Path,
    purpose: &str,
    executor: &Value,
) -> Result<(Value, Value)> {
    let path = common::common_file(backend, path, "续作工具构建请求")?;
    let binding = common::binding(&path)?;
    let tool = verify(backend, &binding, purpose, executor)?;
    Ok((binding, tool))
}

pub(crate) fn verify(
    backend: &Path,
    binding: &Value,
    purpose: &str,
    executor: &Value,
) -> Result<Value> {
    let declared = common::read_bound(backend, binding, "续作工具构建请求")?;
    document::exact(
        &declared,
        &[
            "format_version",
            "kind",
            "purpose",
            "tool",
            "frozen",
            "validation",
        ],
        "续作工具构建请求",
    )?;
    if declared["format_version"] != 1
        || declared["kind"] != "devex-seed-continuation-tool-build"
        || declared["purpose"] != purpose
    {
        return Err("续作工具构建请求不属于本阶段".into());
    }
    let tool = request::external_file(&declared["tool"])?;
    let frozen = request::external_file(&declared["frozen"])?;
    let validation = request::external_file(&declared["validation"])?;
    if json!(tool) != executor["executable"] {
        return Err("续作工具不同于原控制器的实际可执行文件".into());
    }
    let frozen = document::read(&frozen)?;
    let validation = document::read(&validation)?;
    if !matches_frozen(&declared, &frozen, &validation) {
        return Err("续作工具缺少绑定的独立冻结产物及通过的构建验证".into());
    }
    for key in ["tool", "frozen", "validation"] {
        request::external_file(&declared[key])?;
    }
    if common::read_bound(backend, binding, "续作工具构建后像")? != declared {
        return Err("续作工具构建请求在读取期间发生变化".into());
    }
    Ok(declared["tool"].clone())
}

fn matches_frozen(declared: &Value, frozen: &Value, validation: &Value) -> bool {
    frozen["Tool"] == declared["tool"]["path"]
        && same_sha(&frozen["ToolSha"], &declared["tool"]["sha256"])
        && same_sha(&frozen["ResultSHA"], &declared["validation"]["sha256"])
        && frozen["ArchivedInputs"]
            .as_u64()
            .is_some_and(|count| count > 0)
        && validation["ExitCode"] == 0
        && validation["ChangedInputs"]
            .as_array()
            .is_some_and(Vec::is_empty)
        && validation["TestSummaries"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty() && rows.iter().all(Value::is_string))
}

fn same_sha(left: &Value, right: &Value) -> bool {
    matches!((left.as_str(), right.as_str()), (Some(a), Some(b))
        if a.len() == 64 && a.bytes().all(|byte| byte.is_ascii_hexdigit()) && a.eq_ignore_ascii_case(b))
}

#[cfg(test)]
#[test]
fn continuation_tool_requires_the_frozen_executable_and_passed_input_snapshot() {
    let tool = "a".repeat(64);
    let report = "b".repeat(64);
    let request = json!({"tool":{"path":"tool.exe","sha256":tool},
        "validation":{"sha256":report}});
    let mut frozen = json!({"Tool":"tool.exe","ToolSha":tool.to_ascii_uppercase(),
        "ResultSHA":report.to_ascii_uppercase(),"ArchivedInputs":1});
    let mut result = json!({"ExitCode":0,"ChangedInputs":[],"TestSummaries":["passed"]});
    assert!(matches_frozen(&request, &frozen, &result));
    result["ChangedInputs"] = json!(["drift"]);
    assert!(!matches_frozen(&request, &frozen, &result));
    result["ChangedInputs"] = json!([]);
    frozen["Tool"] = json!("other.exe");
    assert!(!matches_frozen(&request, &frozen, &result));
}
