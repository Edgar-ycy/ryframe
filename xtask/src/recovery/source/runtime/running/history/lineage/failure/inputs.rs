//! 谱系恢复只读取绑定请求和实际冻结配置，不重建运行文件。
use super::{super::super::super::builds, model};
use crate::recovery::source::evidence::binding;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            digest, json as document, maintenance, resources::common, seed::generation,
        },
        fixture_control,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn requests(backend: &Path, output: &Path, external: Option<&Path>) -> Result<Value> {
    let internal_path = output.join("request.json");
    let internal_descriptor = common::binding(&internal_path)?;
    let internal = common::read_bound(backend, &internal_descriptor, "C68 内部请求")?;
    generation::coordinator::request(&internal)?;
    let path = external.unwrap_or(&internal_path);
    if external.is_some() && path == internal_path {
        return Err("C69 必须绑定重新登记的外部正式请求，不能复用 C68 内部副本".into());
    }
    let descriptor = common::binding(path)?;
    let request = common::read_bound(backend, &descriptor, "C69 当前请求")?;
    generation::coordinator::request(&request)?;
    for key in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "backend_build",
        "source_environment",
    ] {
        common::bound_file(backend, &request[key], "谱系请求绑定")?;
    }
    let execution = Path::new(document::text(&request["execution_backend"], "谱系执行根")?);
    common::bound_file(execution, &request["maintenance_build"], "谱系维护构建")?;
    let checked = model(
        "request",
        json!({"previous":internal,"request":request,"external":external.is_some()}),
    )?;
    if document::boolean(&checked["reregister"], "成对重登记构建")? {
        generation::builds::verify(backend, &internal, &request)?;
    }
    common::bound_file(backend, &internal_descriptor, "C68 内部请求")?;
    common::bound_file(backend, &descriptor, "C69 当前请求")?;
    Ok(
        json!({"request":request,"descriptor":descriptor,"internal":internal,"internal_descriptor":internal_descriptor}),
    )
}

pub(super) fn runtime(
    backend: &Path,
    output: &Path,
    request: &Value,
    source: &Value,
) -> Result<Value> {
    model("request-source", json!({"request":request,"source":source}))?;
    let execution = Path::new(document::text(&request["execution_backend"], "谱系执行根")?);
    let build = builds::verify(backend, request)?;
    if execution
        .parent()
        .is_some_and(|parent| parent.join("fixture.json").exists())
    {
        return Err("C68 运行环境混入独立 fixture".into());
    }
    let runtime = output.join("runtime");
    exact_directory(backend, &runtime, &["binaries.json", "runtime.json"], &[])?;
    let contract = fixture_control::verify_full_stack_runtime(execution, &runtime)?;
    let binaries = document::read(&runtime.join("binaries.json"))?;
    if binaries != self::binaries(backend, request)? {
        return Err("C68 运行二进制与绑定构建不同".into());
    }
    let mut selected = source["request"]["source"].clone();
    document::object(&selected, "谱系原始来源")?;
    selected["runtime_dir"] = json!(runtime);
    let private = common::read_bound(backend, &request["source_environment"], "谱系私有环境")?;
    let physical = binding::physical(
        &json!({"backend":execution,"evidence_root":backend,"source":selected,
        "app_env":private["environment"]["APP_ENV"],"config_dir":contract["api_config_dir"]}),
    )?;
    if physical != source["generation"]["physical_binding"] {
        return Err("C68 运行配置改变了 C52 物理来源".into());
    }
    api_address(execution, &contract["api_config_dir"], &selected["api_url"])?;
    if build["sources"]["full"].is_null() {
        return Err("C68 构建缺少完整来源".into());
    }
    Ok(json!({"contract":contract,"selected":selected}))
}

pub(in crate::recovery::source::evidence) fn binaries(
    backend: &Path,
    request: &Value,
) -> Result<Value> {
    let build = common::read_bound(backend, &request["backend_build"], "谱系产品构建")?;
    let execution = Path::new(document::text(&request["execution_backend"], "谱系执行根")?);
    let maintenance = maintenance::verify_evidence(
        execution,
        &common::bound_file(execution, &request["maintenance_build"], "谱系维护构建")?,
        None,
    )?;
    Ok(
        json!({"ryframe":build["artifacts"]["api"]["executable"],"ryframe-worker":build["artifacts"]["worker"]["executable"],
        "ryframe-reset":maintenance["artifacts"]["reset"]["executable"],"ryframe-migrate":maintenance["artifacts"]["migrate"]["executable"]}),
    )
}

pub(super) fn layout(backend: &Path, output: &Path) -> Result<()> {
    exact_directory(
        backend,
        output,
        &["intent.json", "request.json"],
        &["before", "runtime"],
    )
}

pub(in crate::recovery::source::evidence) fn exact_directory(
    backend: &Path,
    path: &Path,
    files: &[&str],
    directories: &[&str],
) -> Result<()> {
    common::local_directory(backend, path, "历史完整证据目录")?;
    let mut names = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "历史证据文件名不是 UTF-8")?;
        if digest::linked(&metadata)
            || !(files.contains(&name.as_str()) && metadata.is_file()
                || directories.contains(&name.as_str()) && metadata.is_dir())
        {
            return Err("历史证据目录包含未知、链接或类型不符的项".into());
        }
        names.insert(name);
    }
    if names.len() != files.len() + directories.len() {
        return Err("历史证据目录缺少必需输入".into());
    }
    Ok(())
}

pub(in crate::recovery::source::evidence::running::history) fn missing(
    backend: &Path,
    path: &Path,
) -> Result<()> {
    validate_local_test_path(path, backend, LocalTestPathKind::OutputFile)?;
    if path.try_exists()? {
        return Err("历史零输出位置存在未登记文件或目录".into());
    }
    Ok(())
}

pub(in crate::recovery::source::evidence) fn api_address(
    execution: &Path,
    config: &Value,
    endpoint: &Value,
) -> Result<()> {
    let config = Path::new(document::text(config, "谱系绑定配置目录")?);
    let table = crate::rate_limit::load_app_table(execution, Some(config), "test")
        .map_err(|_| "谱系 API 配置读取失败；检查绑定目录和 app.test.toml")?;
    let app = table
        .get("app")
        .and_then(toml::Value::as_table)
        .ok_or("谱系配置缺少 app 节")?;
    let url = url::Url::parse(document::text(endpoint, "谱系 API 地址")?)
        .map_err(|_| "谱系 API 地址无效")?;
    let hosts = match url.host_str() {
        Some("127.0.0.1") => ["127.0.0.1", "0.0.0.0"],
        Some("[::1]" | "::1") => ["::1", "::"],
        _ => return Err("谱系 API 必须使用明确回环地址".into()),
    };
    if !app
        .get("host")
        .and_then(toml::Value::as_str)
        .is_some_and(|host| hosts.contains(&host))
        || app.get("port").and_then(toml::Value::as_integer) != url.port().map(i64::from)
        || !matches!(url.path(), "" | "/")
    {
        return Err("源 API 精确端口未绑定实际 APP 配置".into());
    }
    Ok(())
}
