//! fresh-target 正式生命周期；写操作保持显式意图、真实前后像和持锁发布。
mod status;
mod storage;
pub(crate) use status::observe;

mod environment;
mod failure;
pub(crate) mod initialize;
pub(crate) mod initialized_objects;
pub(crate) mod objects;
mod prepare;
mod receipts;
pub(crate) mod registration;
pub(crate) mod resources;
mod resume_prepare;
pub(crate) mod selection;
mod snapshot;
pub(crate) mod verify;
pub(crate) use prepare::{prepare, resume as resume_prepare};

pub(crate) fn dispatch(
    backend: &std::path::Path,
    options: &crate::cli::FreshTargetOptions,
) -> crate::Result<serde_json::Value> {
    super::cli::execute(
        backend,
        &options.workspace,
        "cli-dispatch",
        &serde_json::json!({
        "operation":options.operation.as_str(),"write":options.write,"supplied":[options.request.is_some(),
        options.environment.is_some(),options.storage_run.is_some(),options.observation_dir.is_some()]}),
    )?;
    match options.operation.as_str() {
        "prepare" => prepare(
            backend,
            &options.workspace,
            options.request.as_deref().ok_or("prepare 缺少请求")?,
            options
                .environment
                .as_deref()
                .ok_or("prepare 缺少环境文件")?,
            options
                .storage_run
                .as_deref()
                .ok_or("prepare 缺少存储运行")?,
        ),
        "resume-prepare" => resume_prepare(backend, &options.workspace),
        "initialize" => initialize::execute(backend, &options.workspace, false),
        "resume-initialize" => initialize::execute(backend, &options.workspace, true),
        "verify" => verify::execute(
            backend,
            &options.workspace,
            options
                .observation_dir
                .as_deref()
                .ok_or("verify 缺少观察目录")?,
        ),
        _ => Err("原生 fresh 入口拒绝未接线的生命周期操作".into()),
    }
}

pub(super) fn binding(
    backend: &std::path::Path,
    operation: &str,
    args: &serde_json::Value,
) -> crate::Result<serde_json::Value> {
    use super::super::json as document;
    use std::path::Path;
    if operation == "fresh-verify-target" {
        return verify::binding(backend, args);
    }
    if operation == "fresh-status" {
        document::exact(args, &["workspace"], "fresh 状态请求")?;
        return observe(
            backend,
            Path::new(document::text(&args["workspace"], "fresh 工作区")?),
        );
    }
    let fields: &[&str] = if operation == "fresh-prepare" {
        &[
            "workspace",
            "request",
            "environment",
            "storage_run",
            "write",
        ]
    } else {
        &["workspace", "write"]
    };
    document::exact(args, fields, "fresh prepare 请求")?;
    if args["write"] != true {
        return Err("fresh prepare 需要显式 --write".into());
    }
    let workspace = Path::new(document::text(&args["workspace"], "fresh 工作区")?);
    match operation {
        "fresh-prepare" => prepare(
            backend,
            workspace,
            Path::new(document::text(&args["request"], "fresh 请求")?),
            Path::new(document::text(&args["environment"], "fresh 环境文件")?),
            Path::new(document::text(&args["storage_run"], "fresh 存储运行")?),
        ),
        "fresh-resume-prepare" => resume_prepare(backend, workspace),
        _ => Err("未知 fresh prepare 操作".into()),
    }
}
