//! 身份计划只在内存读取私有文件，公开证据不包含凭据。
use std::path::Path;

use serde_json::{Value, json};

use super::history::Post;
use crate::{
    Result,
    data::performance_identities::plan,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::{digest, json as document, resources::common},
};

pub(crate) fn inputs(backend: &Path, run: &Path, request: &Value, post: &Post) -> Result<Value> {
    let prepared = common::read_bound(backend, &request["identity_plan"], "seed 身份计划")?;
    let private = plan::load(&prepared, backend)?;
    let environment = &prepared["environment"];
    let runtime_path = post.runtime.join("runtime.json");
    let contract = document::read(&runtime_path)?;
    let directory = Path::new(
        contract["api_config_dir"]
            .as_str()
            .ok_or("身份 API 配置目录缺失")?,
    );
    let api = crate::rate_limit::load_app_table(backend, Some(directory), "test")
        .map_err(|_| "身份 API 文件配置无法加载；检查明确绑定的 app.test.toml，不回显私有值")?;
    let api = serde_json::to_value(api)?;
    external_tool(&request["identity_tool"])?;
    let process = common::binding(&post.runtime.join("api.json"))?;
    if environment["scope_id"] != api["scope_id"]
        || contract["scope_id"] != environment["scope_id"]
        || environment["backend_dir"].as_str() != backend.to_str()
        || environment["frontend_dir"] != post.request["frontend_root"]
        || environment["runtime"]["directory"].as_str() != post.runtime.to_str()
        || environment["runtime"]["receipt_sha256"] != common::binding(&runtime_path)?["sha256"]
        || request["api_process"] != process
        || environment["runtime"]["api_process_sha256"] != process["sha256"]
    {
        return Err("身份计划必须保留原 API 的 scope、配置与创建身份".into());
    }
    let handoff = document::read(&post.runtime.join("handoff.json"))?;
    let url = |value: &Value| -> Result<String> {
        Ok(value
            .as_str()
            .ok_or("身份地址必须为字符串")?
            .trim_end_matches('/')
            .to_owned())
    };
    if url(&environment["api_url"])? != url(&handoff["api_url"])?
        || api["cors"]["allow_origins"] != json!([url(&environment["frontend_url"])?])
    {
        return Err("身份计划 API 或前端地址不同".into());
    }
    let state = Path::new(
        request["identity_state"]
            .as_str()
            .ok_or("身份账本路径缺失")?,
    );
    validate_local_test_path(state, backend, LocalTestPathKind::StateDirectory)?;
    if state != run.join("seed-runtime/identities") {
        return Err("身份账本必须是本 run 固定 seed 目录".into());
    }
    let mut control = serde_json::Map::new();
    for field in [
        "host", "port", "username", "password", "database", "tls_mode",
    ] {
        let value = api["database"]["primary"]
            .get(field)
            .ok_or("身份 API 控制库字段缺失")?;
        control.insert(field.into(), value.clone());
    }
    if private["database"]["control"] != Value::Object(control) {
        return Err("身份控制库必须与固定文件配置一致".into());
    }
    common::bound_file(backend, &request["identity_plan"], "seed 身份计划")?;
    plan::load(&prepared, backend)?;
    Ok(prepared)
}

pub(crate) fn external_tool(value: &Value) -> Result<()> {
    document::exact(value, &["path", "sha256"], "身份工具")?;
    let path = Path::new(value["path"].as_str().ok_or("身份工具路径缺失")?);
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    if value["sha256"].as_str() != Some(digest::file(path)?.sha256.as_str()) {
        return Err("身份工具摘要变化".into());
    }
    Ok(())
}
