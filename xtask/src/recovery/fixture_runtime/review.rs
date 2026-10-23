use super::{
    dependency::Dependencies, manifest, model::*, plan::Context, process, receipts, responses,
};
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn keys(server: &str) -> Vec<&'static str> {
    if server == "dev" {
        vec!["browser"]
    } else {
        vec![
            "build",
            "build_verify_before",
            "browser",
            "build_verify_after",
        ]
    }
}
pub(super) fn intent(binding: &Value, binding_path: &Path, context: &Context) -> Result<Value> {
    Ok(
        json!({"format_version":1,"kind":"reference-fixture-browser-intent","binding":bound(binding_path)?,"commands":binding["commands"],"login_budget":{"path":context.output("login_budget")?,"existed_before":false},"response_audit":binding["response_audit"]}),
    )
}
pub(super) fn artifacts(context: &Context, build: &Value) -> Result<(Value, Vec<Value>)> {
    let server = text(&context.binding["server"])?;
    let run_id = text(&context.binding["run_id"])?;
    let report = context.output("report")?;
    let results = context.output("results")?;
    check(report.join("index.html").is_file(), "浏览器报告缺少首页")?;
    let tests = receipts::business(&results.join("business-tests.json"), server, run_id)?;
    let report_guard = manifest::capture(
        report,
        &context.frontend.join(".local-tests/playwright-real/report"),
        (MAX_FILES, MAX_BYTES),
    )?;
    let result_guard = manifest::capture(
        results,
        &context
            .frontend
            .join(".local-tests/playwright-real/results"),
        (MAX_FILES, MAX_BYTES),
    )?;
    let responses = if server == "preview" {
        responses::preview(
            context.output("response_audit")?,
            &context.binding,
            &build["dist"],
        )?
    } else {
        Value::Null
    };
    Ok((
        json!({"report":report_guard["manifest"],"results":result_guard["manifest"],"tests":tests,"responses":responses}),
        vec![report_guard, result_guard],
    ))
}
pub(super) fn evidence(
    api: &mut impl Dependencies,
    context: &Context,
    binding_path: &Path,
    value: &Value,
) -> Result<Value> {
    let binding = &context.binding;
    let server = text(&binding["server"])?;
    check(
        !context.output("failure")?.try_exists()?,
        "浏览器没有唯一成功结果；核对失败或不确定状态",
    )?;
    // 即使失败收据也无法发布，残留 pending 仍必须阻止消费者接受可见 result。
    let result_name = context
        .output("result")?
        .file_name()
        .ok_or("成功结果缺少名称")?
        .to_string_lossy();
    for entry in std::fs::read_dir(&context.runtime)? {
        check(
            !entry?
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{result_name}.pending-")),
            "成功收据尚未越过最终提交点；保留现场",
        )?;
    }
    let raw = serde_json::to_vec(value)?;
    check(
        !context
            .secrets
            .iter()
            .any(|s| !s.is_empty() && raw.windows(s.len()).any(|w| w == s.as_bytes())),
        "浏览器成功结果包含未脱敏凭据",
    )?;
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "run_id",
            "server",
            "binding",
            "intent",
            "build",
            "logs",
            "processes",
            "artifacts",
            "login_budget",
            "remote_writes",
        ],
        "浏览器成功结果",
    )?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["kind"] == "reference-fixture-browser-result"
            && value["status"] == "passed"
            && value["run_id"] == binding["run_id"]
            && value["server"] == binding["server"]
            && value["remote_writes"] == json!({"business_data":true}),
        "浏览器成功结果身份或状态无效",
    )?;
    descriptor(&value["binding"], binding_path)?;
    let (intent_value, intent_guard) = document(context.output("intent")?)?;
    check(
        intent_value == intent(binding, binding_path, context)?,
        "浏览器intent与首次写入边界不一致",
    )?;
    descriptor(&value["intent"], context.output("intent")?)?;
    let keys = keys(server);
    strict::exact(&value["logs"], &keys, "浏览器日志")?;
    strict::exact(&value["processes"], &keys, "浏览器进程")?;
    for name in keys {
        read_log(
            context.output(&format!("{name}_log"))?,
            &value["logs"][name],
            &context.secrets,
        )?;
        process::verify(
            api,
            &value["processes"][name],
            context.output(&format!("{name}_process"))?,
            text(&binding["scope_id"])?,
        )?;
    }
    verify_artifacts(api, context, value)?;
    unchanged(&intent_guard)?;
    Ok(value.clone())
}
fn verify_artifacts(api: &mut impl Dependencies, context: &Context, value: &Value) -> Result<()> {
    let binding = &context.binding;
    let server = text(&binding["server"])?;
    let artifacts = &value["artifacts"];
    strict::exact(
        artifacts,
        &["report", "results", "tests", "responses"],
        "浏览器产物",
    )?;
    for (name, kind) in [("report", "report"), ("results", "results")] {
        manifest::verify(
            &artifacts[name],
            context.output(name)?,
            &context
                .frontend
                .join(format!(".local-tests/playwright-real/{kind}")),
        )?;
    }
    check(
        artifacts["tests"]
            == receipts::business(
                &context.output("results")?.join("business-tests.json"),
                server,
                text(&binding["run_id"])?,
            )?,
        "浏览器场景收据与成功结果不一致",
    )?;
    check(
        value["login_budget"] == receipts::budget(context.output("login_budget")?, binding)?,
        "登录预算账本与成功结果不一致",
    )?;
    if server == "preview" {
        strict::exact(&value["build"], &["receipt", "dist"], "前端生产构建")?;
        let receipt = api.call("frontend-build", json!({"frontend":context.frontend}))?;
        descriptor(&value["build"]["receipt"], &path(&receipt["path"])?)?;
        manifest::verify(
            &value["build"]["dist"],
            &context.frontend.join("dist"),
            &context.frontend,
        )?;
        check(
            artifacts["responses"]
                == responses::preview(
                    context.output("response_audit")?,
                    binding,
                    &value["build"]["dist"],
                )?,
            "preview静态响应与成功结果不一致",
        )?;
    } else {
        check(
            value["build"].is_null() && artifacts["responses"].is_null(),
            "dev浏览器结果不得绑定生产构建",
        )?;
    }
    Ok(())
}

pub(super) fn result(
    api: &mut impl Dependencies,
    context: &Context,
    binding_path: &Path,
) -> Result<Value> {
    let (value, guard) = document(context.output("result")?)?;
    let value = evidence(api, context, binding_path, &value)?;
    unchanged(&guard)?;
    Ok(value)
}
