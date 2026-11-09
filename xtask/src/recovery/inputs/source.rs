use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::Value;

use super::{
    document::{Document, array, digest, exact, integer, object, text},
    domain::{endpoint, file_sha256, identifier, validate_local_optional},
    target::clone_name,
};
use crate::{
    Result,
    local_test_path::{
        LocalTestPathKind, validate_external_existing_file, validate_local_test_path,
    },
};

pub(crate) const REQUEST_FIELDS: [&str; 12] = [
    "format_version",
    "kind",
    "id",
    "source",
    "tools",
    "backend_build",
    "maintenance_build",
    "worktree_fingerprint",
    "runtime",
    "processes",
    "producers_registry",
    "max_object_bytes",
];

pub(crate) fn load(backend: &Path, descriptor: &Value) -> Result<Document> {
    let (_, document) = super::document::bound_file(backend, descriptor, "共享源导出请求")?;
    validate(backend, &document.value)?;
    Ok(document)
}

pub(crate) fn validate(backend: &Path, request: &Value) -> Result<()> {
    let request = exact(request, &REQUEST_FIELDS, "源导出请求")?;
    if request.get("format_version") != Some(&Value::from(1))
        || request.get("kind").and_then(Value::as_str) != Some("devex-clone-source-export")
    {
        return Err("源导出请求类型无效".into());
    }
    clone_name(text(request.get("id").unwrap(), "源导出 ID")?)?;
    validate_source(backend, request.get("source").unwrap())?;
    let fingerprint = text(request.get("worktree_fingerprint").unwrap(), "工作区指纹")?;
    if !Regex::new(r"^sha256:[a-f0-9]{64}$")?.is_match(fingerprint) {
        return Err("源导出须绑定完整工作区指纹和对象大小上限".into());
    }
    integer(request.get("max_object_bytes").unwrap(), "对象大小上限")?;
    let tools = exact(
        request.get("tools").unwrap(),
        &["mysql", "mysqldump", "node"],
        "源导出工具",
    )?;
    for tool in tools.values() {
        validate_external(tool)?;
    }
    for field in ["backend_build", "runtime", "producers_registry"] {
        super::document::bound_file(backend, request.get(field).unwrap(), "源导出输入")?;
    }
    let (_, maintenance) = super::document::bound_file(
        backend,
        request.get("maintenance_build").unwrap(),
        "源维护构建",
    )?;
    let maintenance = object(&maintenance.value, "源维护构建")?;
    if maintenance.get("kind").and_then(Value::as_str) != Some("devex-clone-tool-build") {
        return Err("源维护构建类型无效".into());
    }
    let processes = exact(
        request.get("processes").unwrap(),
        &["api", "worker"],
        "源进程收据",
    )?;
    let runtime_dir = PathBuf::from(text(
        request["source"].get("runtime_dir").unwrap(),
        "源运行目录",
    )?);
    let (runtime_binding, _) =
        super::document::bound_file(backend, request.get("runtime").unwrap(), "源运行收据")?;
    if runtime_binding.path != runtime_dir.join("runtime.json") {
        return Err("运行收据不属于源明确运行目录".into());
    }
    for (role, binding) in processes {
        let (binding, _) = super::document::bound_file(backend, binding, "源进程收据")?;
        if binding.path != runtime_dir.join(format!("{role}.json")) {
            return Err("进程收据不属于源明确运行目录".into());
        }
    }
    Ok(())
}

fn validate_source(backend: &Path, value: &Value) -> Result<()> {
    let source = exact(
        value,
        &["scope_id", "runtime_dir", "api_url", "s3", "databases"],
        "源定义",
    )?;
    clone_name(text(source.get("scope_id").unwrap(), "源 scope")?)?;
    validate_local_optional(
        Path::new(text(source.get("runtime_dir").unwrap(), "源运行目录")?),
        &backend.join(".local-tests"),
        "源运行目录",
    )?;
    endpoint(source.get("api_url").unwrap(), "源 API 地址", None)?;
    let s3 = exact(source.get("s3").unwrap(), &["configuration"], "源对象存储")?;
    super::super::clone::native::s3::Configuration::from_app_binding(
        backend,
        s3.get("configuration").unwrap(),
    )?;
    let databases = array(source.get("databases").unwrap(), "源数据库")?;
    if databases.is_empty() {
        return Err("源数据库目标不能为空".into());
    }
    let mut keys = HashSet::new();
    let mut physical = HashSet::new();
    let server_identity = Regex::new(r"^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$")?;
    for database in databases {
        let database = exact(
            database,
            &[
                "key",
                "kind",
                "mode",
                "database",
                "server_uuid",
                "defaults_file",
                "defaults_sha256",
            ],
            "源数据库",
        )?;
        let key = text(database.get("key").unwrap(), "源数据库键")?;
        clone_name(key)?;
        let server = text(database.get("server_uuid").unwrap(), "源数据库身份")?;
        if !server_identity.is_match(server) {
            return Err("源数据库逻辑目标重复、物理身份重叠或类型不符".into());
        }
        let name = text(database.get("database").unwrap(), "源数据库名")?;
        identifier(name)?;
        let kind = text(database.get("kind").unwrap(), "源数据库类型")?;
        let mode = text(database.get("mode").unwrap(), "源数据库模式")?;
        if !keys.insert(key)
            || !physical.insert((server, name.to_ascii_lowercase()))
            || !matches!(kind, "combined" | "tenant")
            || !matches!(mode, "shared" | "dedicated")
            || (key == "shared-control") != (kind == "combined")
            || kind == "combined" && mode != "shared"
        {
            return Err("源数据库逻辑目标重复、物理身份重叠或类型不符".into());
        }
        let defaults = Path::new(text(database.get("defaults_file").unwrap(), "源凭据文件")?);
        validate_local_test_path(defaults, backend, LocalTestPathKind::ExistingFile)
            .map_err(|error| format!("源数据库凭据文件无效：{error}"))?;
        if file_sha256(defaults)? != digest(database.get("defaults_sha256").unwrap(), "源凭据摘要")?
        {
            return Err("源数据库凭据文件已变化".into());
        }
    }
    if !keys.contains("shared-control") {
        return Err("源必须明确包含 combined 控制目标".into());
    }
    Ok(())
}

fn validate_external(tool: &Value) -> Result<()> {
    let tool = exact(tool, &["path", "sha256"], "源外部工具")?;
    let path = Path::new(text(tool.get("path").unwrap(), "源外部工具路径")?);
    validate_external_existing_file(path)?;
    if file_sha256(path)? != digest(tool.get("sha256").unwrap(), "源工具摘要")? {
        return Err("源导出外部工具实际文件与绑定不符".into());
    }
    Ok(())
}
