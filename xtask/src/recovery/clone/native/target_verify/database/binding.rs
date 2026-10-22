use std::{collections::BTreeMap, fs, path::PathBuf};

use serde_json::{Value, json};

use super::super::{
    super::{
        digest,
        json::{self as json_io, text},
        lifecycle::manifest,
    },
    input::{TargetContext, request_physical},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

pub(super) fn capture(context: &TargetContext) -> Result<Value> {
    let execution = context.execution_backend()?;
    let review = &context.request["review"];
    let descriptor =
        json!({"path": review["path"], "bytes": review["bytes"], "sha256": review["sha256"]});
    let review_path = manifest::bound_file(&context.backend, &descriptor, "审阅计划", true)?;
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
        &context.backend,
        context.target(),
        &table,
        &context.environment,
        selected,
    )?;
    let maintenance_path = manifest::bound_file(
        &context.backend,
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
    let plan = json!({"target": target, "tools": context.request["tools"]});
    let result = json!({"side": "target", "tools_plan_sha256": digest::plan_hash(&plan)?, "configuration": configuration,
        "physical": physical, "maintenance": maintenance_digest, "source": source});
    manifest::bound_file(&context.backend, &descriptor, "审阅计划", true)?;
    manifest::bound_file(
        &context.backend,
        &context.request["maintenance_build"],
        "维护构建",
        true,
    )?;
    Ok(result)
}

pub(super) fn configuration(context: &TargetContext) -> Result<Value> {
    let execution = context.execution_backend()?;
    let directory = match context.request.get("configuration_directory") {
        Some(value) => PathBuf::from(request_physical::strict_text(value)?),
        None => execution.join("config"),
    };
    let directory = if directory.is_absolute() {
        directory
    } else {
        execution.join(directory)
    };
    validate_absolute_path(&directory, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "库存配置目录不能经过链接")?;
    let resolved = fs::canonicalize(&directory)?;
    if !resolved.starts_with(fs::canonicalize(&execution)?)
        && !resolved.starts_with(fs::canonicalize(&context.backend)?)
    {
        return Err("库存配置必须位于执行后端或协调后端".into());
    }
    let app_env = context
        .environment
        .get("APP_ENV")
        .filter(|value| matches!(value.as_str(), "dev" | "test" | "prod"))
        .ok_or("库存 APP_ENV 必须明确选择 dev、test 或 prod")?;
    validate_absolute_path(
        &directory.join(format!("app.{app_env}.toml")),
        LocalTestPathKind::ExistingFile,
    )
    .map_err(|_| "库存配置缺少无链接的已选环境文件")?;
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                if cfg!(windows) {
                    value.eq_ignore_ascii_case("toml")
                } else {
                    value == "toml"
                }
            })
        {
            files.insert(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "配置文件名必须为 UTF-8")?,
                super::evidence::descriptor(&path)?,
            );
        }
    }
    if files.is_empty() {
        return Err("库存配置目录没有 TOML 文件".into());
    }
    let path = resolved.to_str().ok_or("配置目录必须为 UTF-8")?;
    #[cfg(windows)]
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    Ok(
        json!({"directory": path, "files": files, "environment_sha256": digest::plan_hash(&json!({"APP_ENV": app_env}))?}),
    )
}
