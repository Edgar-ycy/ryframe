use serde_json::{Value, json};

use super::super::{
    super::{
        digest,
        json::{self as json_io, text},
        lifecycle::manifest,
        resources::common,
    },
    input::request_physical,
    storage::StorageInputs,
};
use crate::Result;

pub(super) fn capture_source(
    context: &StorageInputs<'_>,
    source: &super::SourceBinding<'_>,
) -> Result<Value> {
    let execution = context.execution_backend()?;
    let configuration_value = configuration(context)?;
    let selected = source.selected;
    if selected["scope_id"] != context.target()["scope_id"]
        || selected["databases"] != context.target()["databases"]
        || source.tools["mysql"] != context.request["tools"]["mysql"]
        || *source.maintenance != context.request["maintenance_build"]
    {
        return Err("来源四库采集端口与已绑定原始计划、维护构建或 MySQL 工具不同".into());
    }
    let physical = crate::recovery::source::evidence::binding::physical(&json!({
        "backend":execution,"evidence_root":context.backend,"source":selected,"app_env":"test",
        "config_dir":configuration_value["directory"]}))?;
    let path = common::bound_file(context.backend, source.maintenance, "来源维护构建")?;
    let value = json_io::read(&path)?;
    let maintenance = source.scope.maintenance(&execution, &path, &value)?;
    context.maintenance_executable("tenant-data")?;
    context.tool("mysql")?;
    let observed = json!({"side":"source","tools_plan_sha256":digest::plan_hash(&json!({"source":selected,"tools":source.tools}))?,
        "configuration":configuration_value,"physical":physical,"maintenance":common::binding(&path)?,"source":maintenance["source"]});
    common::bound_file(context.backend, source.maintenance, "来源维护构建")?;
    if configuration(context)? != observed["configuration"] {
        return Err("来源四库配置在读取期间变化".into());
    }
    source.scope.assert_unchanged()?;
    Ok(observed)
}

pub(super) fn capture(context: &StorageInputs<'_>) -> Result<Value> {
    let execution = context.execution_backend()?;
    let review = &context.request["review"];
    let descriptor =
        json!({"path": review["path"], "bytes": review["bytes"], "sha256": review["sha256"]});
    let review_path = manifest::bound_file(context.backend, &descriptor, "审阅计划", true)?;
    let review_value = json_io::read(&review_path)?;
    if digest::plan_hash(&review_value)? != text(&review["canonical_sha256"], "审阅规范摘要")?
    {
        return Err("库存审阅计划规范摘要不同".into());
    }
    let side = text(&context.request["side"], "审阅单侧")?;
    let selected = review_value["scopes"].get(side).ok_or("库存审阅单侧缺失")?;
    let mut target = context.target().clone();
    let object = target.as_object_mut().ok_or("库存目标无效")?;
    for field in ["runtime_dir", "api_url"] {
        object.insert(
            field.into(),
            selected.get(field).ok_or("库存缺少审阅运行字段")?.clone(),
        );
    }
    let configuration = configuration(context)?;
    let table = serde_json::to_value(
        crate::rate_limit::load_app_table(
            &execution,
            configuration["directory"]
                .as_str()
                .map(std::path::Path::new),
            context
                .environment
                .get("APP_ENV")
                .map(String::as_str)
                .unwrap_or(""),
        )
        .map_err(|error| {
            format!(
                "库存运行配置无法读取：{}；{}",
                error.code(),
                error.message()
            )
        })?,
    )?;
    // 配置从执行工作树读取；凭据和物理资源的边界仍属于协调后端。
    let physical = request_physical::binding(
        context.backend,
        context.target(),
        &table,
        context.environment,
        selected,
    )?;
    let maintenance_path = manifest::bound_file(
        context.backend,
        &context.request["maintenance_build"],
        "维护构建",
        true,
    )?;
    let maintenance_digest = super::evidence::descriptor(&maintenance_path)?;
    let maintenance = json_io::read(&maintenance_path)?;
    let source = maintenance
        .get("source")
        .filter(|value| value.is_object())
        .ok_or("维护构建缺少来源绑定")?;
    context.maintenance_executable("tenant-data")?;
    context.tool("mysql")?;
    target["s3"] = json!({"configuration":super::super::super::s3::app_configuration::configuration_value(
        &execution, std::path::Path::new(text(&configuration["directory"], "库存配置目录")?), "test")?});
    let plan = json!({"target": target, "tools": context.request["tools"]});
    let result = json!({"side": "target", "tools_plan_sha256": digest::plan_hash(&plan)?, "configuration": configuration,
        "physical": physical, "maintenance": maintenance_digest, "source": source});
    manifest::bound_file(context.backend, &descriptor, "审阅计划", true)?;
    manifest::bound_file(
        context.backend,
        &context.request["maintenance_build"],
        "维护构建",
        true,
    )?;
    Ok(result)
}

pub(super) fn configuration(context: &StorageInputs<'_>) -> Result<Value> {
    let execution = context.execution_backend()?;
    let (directory, files) = super::super::input::request_configuration::fresh_files(
        &execution,
        context.environment,
        context.request,
    )?;
    Ok(
        json!({"directory": manifest::path_text(&directory, "config_dir")?, "files": files,
        "environment_sha256": digest::plan_hash(&json!({"APP_ENV": "test"}))?}),
    )
}
