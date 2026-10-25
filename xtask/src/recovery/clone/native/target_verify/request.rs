use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};

use super::super::super::{
    digest,
    input::digest_shape,
    json::{self as json_io, exact, text},
    lifecycle::manifest,
    source,
};
use super::{request_physical, request_review};
use crate::{
    Result,
    local_test_path::{
        LocalTestPathKind, validate_absolute_path, validate_external_existing_file,
        validate_local_test_path,
    },
};

pub(crate) const KEYS: [(&str, &str, &str); 4] = [
    ("shared-control", "combined", "shared"),
    ("shared", "tenant", "shared"),
    ("dedicated-a", "tenant", "dedicated"),
    ("dedicated-b", "tenant", "dedicated"),
];
pub(crate) const EXCLUSIVE: [&str; 3] = [
    "mysql_exclusive",
    "redis_exclusive",
    "object_storage_exclusive",
];

pub(crate) struct Binding {
    pub(crate) selected: Value,
    pub(crate) execution_backend: PathBuf,
    pub(crate) execution: Value,
}

/// Git 快照观察与静态字段校验分离，测试必须显式提供观察实现。
pub(crate) trait SourceObserver {
    fn snapshot(&self, root: &Path) -> Result<Value>;
}

pub(crate) struct SystemSourceObserver;

impl SourceObserver for SystemSourceObserver {
    fn snapshot(&self, root: &Path) -> Result<Value> {
        let inventory = source::capture(root)?;
        let mut snapshot = source::binding_value(&inventory.source)["snapshot"].clone();
        snapshot
            .as_object_mut()
            .ok_or("源码快照无效")?
            .remove("clean");
        Ok(snapshot)
    }
}

pub(crate) fn validate(backend: &Path, value: &Value) -> Result<Binding> {
    validate_with(backend, value, &SystemSourceObserver)
}

/// fresh 请求字段由正式绑定与目标验证共同消费，不接受旧目录键或隐式配置。
pub(crate) fn fields(value: &Value) -> Vec<&'static str> {
    let mut fields = vec![
        "format_version",
        "kind",
        "id",
        "review",
        "side",
        "target",
        "maintenance_build",
        "configuration_sha256",
        "config_dir",
        "tools",
        "storage",
        "reset",
    ];
    if value.get("execution_backend").is_some() {
        fields.push("execution_backend");
    }
    fields
}

const CONFIG_DIR_ERROR: &str =
    "目标 config_dir 必须是已绑定执行树内的无链接配置目录；请修正请求 config_dir";

pub(crate) fn config_directory(backend: &Path, request: &Value) -> Result<Value> {
    let execution = match request.get("execution_backend") {
        Some(value) => Path::new(value["path"].as_str().ok_or(CONFIG_DIR_ERROR)?),
        None => backend,
    };
    validate_absolute_path(execution, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| CONFIG_DIR_ERROR)?;
    let path = PathBuf::from(request["config_dir"].as_str().ok_or(CONFIG_DIR_ERROR)?);
    validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| CONFIG_DIR_ERROR)?;
    if !(path.is_absolute()
        && execution.starts_with(backend)
        && path.starts_with(execution)
        && path.file_name().is_some_and(|name| name == "config"))
    {
        return Err(CONFIG_DIR_ERROR.into());
    }
    Ok(json!({"path":path}))
}

pub(crate) fn validate_with(
    backend: &Path,
    value: &Value,
    observer: &dyn SourceObserver,
) -> Result<Binding> {
    let root = exact(value, &fields(value), "目标请求")?;
    config_directory(backend, value)?;
    if root["format_version"].as_u64() != Some(1)
        || root["kind"].as_str() != Some("devex-clone-fresh-target")
    {
        return Err("新目标请求类型错误".into());
    }
    // 与当前 Python devex_clone_model.name 共用相同的主仓 Rust 规则。
    super::super::super::input::name(text(&root["id"], "目标请求 ID")?)?;
    digest_shape(&root["configuration_sha256"])?;
    let review = request_review::read(backend, &root["review"])?;
    let selected = review["scopes"]
        .get(text(&root["side"], "目标单侧")?)
        .ok_or("必须选择已就绪审阅计划的明确单侧")?;
    target(backend, &root["target"], selected)?;
    tools(&root["tools"], &review)?;
    exact(&root["storage"], &["rustfs", "redis"], "目标存储")?;
    reset(value, selected)?;
    manifest::bound_file(backend, &root["maintenance_build"], "目标维护构建", true)?;
    let (execution_backend, execution) =
        execution_backend(backend, root.get("execution_backend"), observer)?;
    Ok(Binding {
        selected: selected.clone(),
        execution_backend,
        execution,
    })
}

pub(crate) fn target(backend: &Path, value: &Value, selected: &Value) -> Result<()> {
    let target = exact(value, &["scope_id", "s3", "databases"], "目标配置")?;
    let scope = text(&target["scope_id"], "目标 scope")?;
    super::super::super::input::name(scope)?;
    if selected.get("scope_id") != Some(&target["scope_id"]) {
        return Err("目标 scope 与单侧审阅计划不符".into());
    }
    let s3 = exact(
        &target["s3"],
        &["endpoint", "region", "access_key", "secret_key"],
        "目标对象配置",
    )?;
    for field in ["endpoint", "region"] {
        if selected["objects"].get(field) != s3.get(field) {
            return Err("对象存储不是计划中的目标端点".into());
        }
    }
    for field in ["access_key", "secret_key"] {
        request_physical::strict_text(&s3[field])?;
    }
    let actual = json_io::array(&target["databases"], "目标数据库")?;
    let mut seen = BTreeSet::new();
    if actual.len() != KEYS.len() {
        return Err("新目标必须精确包含四个逻辑数据库".into());
    }
    let database_name = Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$")?;
    for db in actual {
        let db = exact(
            db,
            &[
                "key",
                "kind",
                "mode",
                "database",
                "server_uuid",
                "defaults_file",
                "defaults_sha256",
            ],
            "目标数据库",
        )?;
        let key = text(&db["key"], "数据库 key")?;
        let (_, kind, mode) = KEYS
            .iter()
            .find(|(wanted, _, _)| *wanted == key)
            .ok_or("目标数据库 key 无效")?;
        let planned = selected["databases"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("key") == Some(&db["key"]))
            })
            .ok_or("审阅计划缺少数据库")?;
        let name = text(&db["database"], "数据库名称")?;
        if !seen.insert(key)
            || db["kind"].as_str() != Some(*kind)
            || db["mode"].as_str() != Some(*mode)
            || !database_name.is_match(name)
            || planned.get("database") != Some(&db["database"])
            || planned.get("expected_server_uuid") != Some(&db["server_uuid"])
            || planned.get("connection_file") != Some(&db["defaults_file"])
        {
            return Err("数据库物理身份或凭据路径不是精确审阅目标".into());
        }
        text(&db["server_uuid"], "数据库 UUID")?;
        let path = Path::new(text(&db["defaults_file"], "MySQL 凭据路径")?);
        validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
            .map_err(|_| "目标 MySQL 凭据路径无效")?;
        if digest::file(path)?.sha256 != digest_shape(&db["defaults_sha256"])? {
            return Err("目标 MySQL 凭据文件变化".into());
        }
    }
    Ok(())
}

pub(crate) fn external_file(value: &Value) -> Result<PathBuf> {
    let item = exact(value, &["path", "sha256"], "目标工具")?;
    let path = PathBuf::from(text(&item["path"], "目标工具路径")?);
    validate_external_existing_file(&path).map_err(|_| "目标工具必须是无链接的绝对普通文件")?;
    if digest::file(&path)?.sha256 != digest_shape(&item["sha256"])? {
        return Err("目标工具实际文件与绑定不符".into());
    }
    Ok(path)
}

pub(crate) fn tools(value: &Value, review: &Value) -> Result<()> {
    let tools = exact(value, &["mysql"], "目标工具")?;
    for (role, tool) in tools {
        external_file(tool)?;
        for key in ["path", "sha256"] {
            if tool.get(key) != review["tools"][role].get(key) {
                return Err("目标工具不属于审阅版本".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn reset(value: &Value, selected: &Value) -> Result<()> {
    let reset = exact(
        &value["reset"],
        &[
            "legacy_ownership",
            "credential_version",
            "sentinel_key",
            "sentinel_value",
        ],
        "目标 reset 配置",
    )?;
    let ownership = exact(
        &reset["legacy_ownership"],
        &EXCLUSIVE,
        "目标 reset exclusive",
    )?;
    if ownership
        .values()
        .any(|value| value.as_bool() != Some(true))
    {
        return Err("目标 reset exclusive 必须全部明确为 true".into());
    }
    let version = text(&reset["credential_version"], "reset 凭据版本")?;
    let scope = text(&value["target"]["scope_id"], "目标 scope")?;
    let id = text(&value["id"], "目标请求 ID")?;
    let key = text(&reset["sentinel_key"], "reset sentinel key")?;
    let namespace = text(&selected["redis"]["namespace"], "Redis namespace")?;
    if !Regex::new(r"^[A-Za-z0-9._-]{1,64}$")?.is_match(version)
        || key != format!("ryframe:devex-fresh:{scope}:sentinel")
        || reset["sentinel_value"].as_str() != Some(format!("devex-fresh:{id}").as_str())
        || key.starts_with(namespace)
    {
        return Err("sentinel 必须是仅归本 scope 的精确外部哨兵".into());
    }
    Ok(())
}

pub(crate) fn execution_backend(
    backend: &Path,
    declared: Option<&Value>,
    observer: &dyn SourceObserver,
) -> Result<(PathBuf, Value)> {
    let Some(declared) = declared.filter(|value| !value.is_null()) else {
        validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)
            .map_err(|_| "后端根目录无效")?;
        return Ok((
            backend.to_path_buf(),
            json!({"kind": "current-backend", "path": manifest::path_text(backend, "后端根目录")?}),
        ));
    };
    let declared = exact(declared, &["fixture", "path"], "冻结执行工作树")?;
    let fixture_path =
        manifest::bound_file(backend, &declared["fixture"], "冻结业务 fixture", true)?;
    let fixture = json_io::read(&fixture_path)?;
    if fixture["format_version"].as_u64() != Some(1)
        || fixture["fixture"].as_str() != Some("business")
        || fixture["status"].as_str() != Some("ready")
        || !fixture["paths"].is_object()
        || !fixture["generated"].is_object()
    {
        return Err("冻结业务工作树收据无效".into());
    }
    let root = PathBuf::from(text(&declared["path"], "冻结执行工作树")?);
    validate_local_test_path(&root, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "冻结业务工作树路径无效")?;
    validate_external_existing_file(&root.join("Cargo.toml"))
        .map_err(|_| "冻结工作树缺少 Cargo.toml")?;
    if !root.join(".git").exists()
        || digest::linked_path(&root.join(".git"))?
            .into_iter()
            .any(|linked| linked)
        || Path::new(text(&fixture["paths"]["backend"], "fixture 后端路径")?) != root
        || !fixture["generated"]["backend"].is_object()
        || observer.snapshot(&root)? != fixture["generated"]["backend"]
    {
        return Err("冻结业务工作树或生成来源已变化".into());
    }
    manifest::bound_file(backend, &declared["fixture"], "冻结业务 fixture", true)?;
    Ok((
        root.clone(),
        json!({"kind": "business-fixture", "path": manifest::path_text(&root, "冻结工作树")?, "fixture": declared["fixture"], "source": fixture["generated"]["backend"]}),
    ))
}
