use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::Value;

use super::{
    document::{Document, array, digest, exact, object, text},
    domain::{
        TARGET_DATABASES, endpoint, file_sha256, identifier, mode_for_key, validate_local_optional,
    },
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) struct TargetRequest {
    pub(crate) document: Document,
    pub(crate) selected: Value,
    pub(crate) execution: PathBuf,
    pub(crate) execution_binding: Value,
}

pub(crate) fn validate(backend: &Path, request: Document, side: &str) -> Result<TargetRequest> {
    let fields = super::super::clone::native::target_verify::input::request::fields(&request.value);
    let object = exact(&request.value, &fields, "新目标请求")?;
    super::super::clone::native::target_verify::input::request::config_directory(
        backend,
        &request.value,
    )?;
    if object.get("format_version") != Some(&Value::from(1))
        || object.get("kind").and_then(Value::as_str) != Some("devex-clone-fresh-target")
    {
        return Err("新目标请求类型错误".into());
    }
    clone_name(text(object.get("id").unwrap(), "新目标 ID")?)?;
    if object.get("side").and_then(Value::as_str) != Some(side) {
        return Err("arm 目标侧与请求不同".into());
    }
    let review = read_review(backend, object.get("review").unwrap())?;
    let selected = review
        .value
        .get("scopes")
        .and_then(|scopes| scopes.get(side))
        .cloned()
        .ok_or("必须选择已就绪审阅计划的明确单侧")?;
    text(
        selected
            .get("backend_dir")
            .ok_or("审阅单侧资源缺少执行后端")?,
        "审阅执行后端",
    )?;
    validate_review_database_isolation(&review.value)?;
    validate_target(backend, object.get("target").unwrap(), &selected)?;
    validate_tools(object.get("tools").unwrap(), &review.value)?;
    exact(
        object.get("storage").unwrap(),
        &["rustfs", "redis"],
        "目标存储定义",
    )?;
    digest(object.get("configuration_sha256").unwrap(), "目标配置摘要")?;
    super::document::bound_file(
        backend,
        object.get("maintenance_build").unwrap(),
        "维护构建",
    )?;
    let (execution, execution_binding) =
        execution_backend(backend, object.get("execution_backend"))?;
    request.assert_unchanged()?;
    review.assert_unchanged()?;
    Ok(TargetRequest {
        document: request,
        selected,
        execution,
        execution_binding,
    })
}

fn read_review(backend: &Path, value: &Value) -> Result<Document> {
    let binding = exact(
        value,
        &["path", "bytes", "sha256", "canonical_sha256"],
        "审阅计划绑定",
    )?;
    let descriptor = serde_json::json!({
        "path": binding.get("path").unwrap(),
        "bytes": binding.get("bytes").unwrap(),
        "sha256": binding.get("sha256").unwrap(),
    });
    let (_, document) = super::document::bound_file(backend, &descriptor, "审阅计划")?;
    if super::document::hash(&document.value)?
        != digest(binding.get("canonical_sha256").unwrap(), "审阅计划规范摘要")?
    {
        return Err("审阅计划规范摘要不匹配".into());
    }
    validate_review(&document.value)?;
    Ok(document)
}

fn validate_review(review: &Value) -> Result<()> {
    let review = object(review, "审阅计划")?;
    object(
        review.get("reference").ok_or("审阅计划缺少保护资源")?,
        "审阅保护资源",
    )?;
    if review.get("kind").and_then(Value::as_str)
        != Some("review-only-perf-resource-plan-with-readonly-preflight")
    {
        return Err("审阅计划类型无效".into());
    }
    if review.get("ready_for_execution") != Some(&Value::Bool(true)) {
        return Err("审阅计划尚未就绪".into());
    }
    let scopes = object(review.get("scopes").ok_or("审阅计划缺少资源")?, "审阅资源")?;
    if scopes.keys().map(String::as_str).collect::<HashSet<_>>()
        != ["seed", "base", "candidate"].into_iter().collect()
    {
        return Err("审阅计划必须完整声明三侧资源".into());
    }
    let tools = object(review.get("tools").ok_or("审阅计划缺少工具")?, "审阅工具")?;
    let services = object(
        review.get("services").ok_or("审阅计划缺少服务")?,
        "审阅服务",
    )?;
    validate_review_tools(review, tools, services)?;
    validate_review_scopes(scopes)?;
    Ok(())
}

fn validate_review_tools(
    review: &serde_json::Map<String, Value>,
    tools: &serde_json::Map<String, Value>,
    services: &serde_json::Map<String, Value>,
) -> Result<()> {
    for role in ["mysql", "mysqldump", "rustfs"] {
        let tool = object(tools.get(role).ok_or("审阅工具缺失")?, "审阅工具")?;
        super::document::digest(
            tool.get("sha256").ok_or("审阅工具缺少摘要")?,
            "审阅工具摘要",
        )?;
        text(tool.get("path").ok_or("审阅工具缺少路径")?, "审阅工具路径")?;
    }
    let redis = object(
        tools.get("redis_server").ok_or("缺少 Redis 工具")?,
        "Redis 工具",
    )?;
    for field in ["distribution", "resolved_path", "sha256"] {
        text(redis.get(field).ok_or("Redis 工具字段缺失")?, "Redis 工具")?;
    }
    let rustfs = object(
        services.get("rustfs").ok_or("审阅 RustFS 服务缺失")?,
        "审阅 RustFS 服务",
    )?;
    for field in ["api", "console", "data_dir"] {
        if text(
            rustfs.get(field).ok_or("审阅 RustFS 服务字段缺失")?,
            "审阅 RustFS 服务",
        )?
        .is_empty()
        {
            return Err("审阅服务定义无效".into());
        }
    }
    let redis_service = object(
        services.get("redis").ok_or("审阅 Redis 服务缺失")?,
        "审阅 Redis 服务",
    )?;
    text(
        redis_service
            .get("directory")
            .ok_or("审阅 Redis 服务目录缺失")?,
        "审阅 Redis 服务目录",
    )?;
    for role in ["source", "protected_target"] {
        let side = object(
            review["reference"].get(role).ok_or("审阅保护资源缺失")?,
            "审阅保护资源",
        )?;
        array(
            side.get("databases").ok_or("保护资源缺少数据库")?,
            "保护数据库",
        )?;
    }
    Ok(())
}

fn validate_review_scopes(scopes: &serde_json::Map<String, Value>) -> Result<()> {
    for (name, side) in scopes {
        let side = object(side, "审阅单侧资源")?;
        let scope = text(side.get("scope_id").ok_or("审阅 scope 缺失")?, "审阅 scope")?;
        clone_name(scope)?;
        for field in [
            "runtime_dir",
            "identity_ledger",
            "api_url",
            "worker_ready_url",
            "frontend_url",
        ] {
            text(side.get(field).ok_or("审阅运行路径缺失")?, "审阅运行路径")?;
        }
        let objects = object(
            side.get("objects").ok_or("审阅对象定义缺失")?,
            "审阅对象定义",
        )?;
        for field in ["endpoint", "region"] {
            text(
                objects.get(field).ok_or("审阅对象字段缺失")?,
                "审阅对象定义",
            )?;
        }
        let redis = object(side.get("redis").ok_or("审阅 Redis 缺失")?, "审阅 Redis")?;
        for field in ["url", "namespace", "ownership_key", "ownership_value"] {
            text(redis.get(field).ok_or("审阅 Redis 字段缺失")?, "审阅 Redis")?;
        }
        let databases = array(side.get("databases").ok_or("审阅数据库缺失")?, "审阅数据库")?;
        let keys = databases
            .iter()
            .filter_map(|database| database.get("key").and_then(Value::as_str))
            .collect::<HashSet<_>>();
        if keys != TARGET_DATABASES.iter().map(|(key, _, _)| *key).collect() {
            return Err(format!("审阅计划 {name} 数据库定义无效").into());
        }
        for database in databases {
            let database = object(database, "审阅数据库")?;
            text(
                database.get("database").ok_or("审阅数据库名称缺失")?,
                "审阅数据库",
            )?;
            text(
                database
                    .get("expected_server_uuid")
                    .ok_or("审阅数据库身份缺失")?,
                "审阅数据库",
            )?;
        }
    }
    Ok(())
}

fn validate_review_database_isolation(review: &Value) -> Result<()> {
    let mut names = HashSet::new();
    let scopes = object(review.get("scopes").unwrap(), "审阅资源")?;
    for side in scopes.values() {
        for database in array(
            object(side, "审阅单侧资源")?
                .get("databases")
                .ok_or("审阅数据库缺失")?,
            "审阅数据库",
        )? {
            let database = object(database, "审阅数据库")?;
            names.insert((
                text(
                    database
                        .get("expected_server_uuid")
                        .ok_or("审阅数据库身份缺失")?,
                    "审阅数据库身份",
                )?
                .to_owned(),
                text(
                    database.get("database").ok_or("审阅数据库名称缺失")?,
                    "审阅数据库名称",
                )?
                .to_ascii_lowercase(),
            ));
        }
    }
    if names.len() != 12 {
        return Err("三侧数据库相互重叠或碰到参考资源".into());
    }
    let reference = object(review.get("reference").unwrap(), "审阅保护资源")?;
    for role in ["source", "protected_target"] {
        for database in array(
            object(reference.get(role).unwrap(), "审阅保护资源")?
                .get("databases")
                .ok_or("保护资源缺少数据库")?,
            "保护数据库",
        )? {
            let database = object(database, "保护数据库")?;
            let identity = (
                text(
                    database.get("server_uuid").ok_or("保护数据库身份缺失")?,
                    "保护数据库身份",
                )?
                .to_owned(),
                text(
                    database.get("database").ok_or("保护数据库名称缺失")?,
                    "保护数据库名称",
                )?
                .to_ascii_lowercase(),
            );
            if names.contains(&identity) {
                return Err("三侧数据库相互重叠或碰到参考资源".into());
            }
        }
    }
    Ok(())
}

fn validate_target(backend: &Path, value: &Value, selected: &Value) -> Result<()> {
    let target = exact(value, &["scope_id", "s3", "databases"], "新目标")?;
    let scope = text(target.get("scope_id").unwrap(), "新目标 scope")?;
    if Some(scope) != selected.get("scope_id").and_then(Value::as_str) {
        return Err("目标 scope 与单侧审阅计划不符".into());
    }
    let s3 = exact(
        target.get("s3").unwrap(),
        &["endpoint", "region", "access_key", "secret_key"],
        "新目标对象存储",
    )?;
    let planned_objects = object(
        selected.get("objects").ok_or("审阅计划缺少对象存储")?,
        "审阅对象存储",
    )?;
    if s3.get("endpoint") != planned_objects.get("endpoint")
        || s3.get("region") != planned_objects.get("region")
    {
        return Err("对象存储不是计划中的目标端点".into());
    }
    endpoint(s3.get("endpoint").unwrap(), "对象存储端点", None)?;
    for field in ["access_key", "secret_key"] {
        let credential = text(s3.get(field).unwrap(), "目标对象凭据")?;
        if credential.is_empty() || credential.chars().any(char::is_control) {
            return Err("目标对象凭据必须在绑定文件中直接提供且不能为空".into());
        }
    }
    let actual = array(target.get("databases").unwrap(), "新目标数据库")?;
    let keys = actual
        .iter()
        .map(|database| text(database.get("key").unwrap(), "目标数据库键"))
        .collect::<Result<HashSet<_>>>()?;
    if actual.len() != 4 || keys != TARGET_DATABASES.iter().map(|(key, _, _)| *key).collect() {
        return Err("新目标必须精确包含四个逻辑数据库".into());
    }
    let planned = array(
        selected.get("databases").ok_or("审阅计划缺少数据库")?,
        "审阅数据库",
    )?
    .iter()
    .map(|database| {
        (
            text(database.get("key").unwrap(), "审阅数据库键").unwrap(),
            database,
        )
    })
    .collect::<std::collections::HashMap<_, _>>();
    for database in actual {
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
            "目标数据库",
        )?;
        let key = text(database.get("key").unwrap(), "目标数据库键")?;
        let (kind, mode) = mode_for_key(key)?;
        let expected = planned.get(key).ok_or("审阅计划缺少数据库")?;
        if text(database.get("kind").unwrap(), "目标数据库类型")? != kind
            || text(database.get("mode").unwrap(), "目标数据库模式")? != mode
            || database.get("database") != expected.get("database")
            || database.get("server_uuid") != expected.get("expected_server_uuid")
            || database.get("defaults_file") != expected.get("connection_file")
        {
            return Err("数据库物理身份或凭据路径不是精确审阅目标".into());
        }
        identifier(text(database.get("database").unwrap(), "目标数据库名")?)?;
        let defaults = Path::new(text(
            database.get("defaults_file").unwrap(),
            "目标凭据路径",
        )?);
        validate_local_test_path(defaults, backend, LocalTestPathKind::ExistingFile)
            .map_err(|_| "目标 MySQL 凭据文件必须位于当前 .local-tests")?;
        if file_sha256(defaults)?
            != digest(database.get("defaults_sha256").unwrap(), "目标凭据摘要")?
        {
            return Err("目标 MySQL 凭据文件变化".into());
        }
    }
    Ok(())
}

fn validate_tools(value: &Value, review: &Value) -> Result<()> {
    let tools = exact(value, &["mysql"], "目标工具")?;
    let reviewed = object(review.get("tools").ok_or("审阅计划缺少工具")?, "审阅工具")?;
    for (role, tool) in tools {
        let tool = exact(tool, &["path", "sha256"], "目标工具")?;
        let path = Path::new(text(tool.get("path").unwrap(), "目标工具路径")?);
        if !path.is_absolute()
            || file_sha256(path)? != digest(tool.get("sha256").unwrap(), "目标工具摘要")?
        {
            return Err("目标工具实际字节或路径已变化".into());
        }
        if tool.get("path") != reviewed.get(role).and_then(|value| value.get("path"))
            || tool.get("sha256") != reviewed.get(role).and_then(|value| value.get("sha256"))
        {
            return Err("目标工具不属于审阅版本".into());
        }
    }
    Ok(())
}

fn execution_backend(backend: &Path, value: Option<&Value>) -> Result<(PathBuf, Value)> {
    if let Some(declared) = value.filter(|v| v.get("execution_source").is_some()) {
        return crate::recovery::fixture_control::source_pair::execution::target_binding(
            backend, declared,
        );
    }
    let Some(value) = value else {
        let root = backend.to_path_buf();
        return Ok((
            root.clone(),
            serde_json::json!({"kind": "current-backend", "path": root.to_string_lossy()}),
        ));
    };
    let declared = exact(value, &["fixture", "path"], "冻结执行工作树")?;
    let (_, fixture) = super::document::bound_file(
        backend,
        declared.get("fixture").unwrap(),
        "冻结执行来源收据",
    )?;
    let fixture = object(&fixture.value, "冻结执行来源收据")?;
    if fixture.get("format_version") != Some(&Value::from(1))
        || fixture.get("fixture").and_then(Value::as_str) != Some("business")
        || fixture.get("status").and_then(Value::as_str) != Some("ready")
    {
        return Err("冻结业务执行工作树收据无效".into());
    }
    let root = PathBuf::from(text(declared.get("path").unwrap(), "冻结执行工作树")?);
    validate_local_optional(&root, &backend.join(".local-tests"), "冻结执行工作树")?;
    if !root.join("Cargo.toml").is_file() || !root.join(".git").exists() {
        return Err("冻结业务执行工作树无效".into());
    }
    if fixture
        .get("paths")
        .and_then(|value| value.get("backend"))
        .and_then(Value::as_str)
        != Some(root.to_string_lossy().as_ref())
    {
        return Err("冻结业务执行工作树路径与收据不同".into());
    }
    let generated = fixture
        .get("generated")
        .and_then(|value| value.get("backend"))
        .ok_or("冻结业务执行来源缺少生成快照")?;
    if !generated.is_object() {
        return Err("冻结业务执行生成来源无效".into());
    }
    Ok((
        root,
        serde_json::json!({
            "kind": "business-fixture",
            "path": declared.get("path").unwrap(),
            "fixture": declared.get("fixture").unwrap(),
            "source": generated,
        }),
    ))
}

pub(crate) fn clone_name(value: &str) -> Result<()> {
    if !Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$")?.is_match(value) {
        return Err("开发复制名称无效".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/inputs/published_target.rs"]
mod published_tests;
