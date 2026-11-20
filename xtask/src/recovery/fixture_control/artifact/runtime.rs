use super::super::{exact_object, file_binding, read_json, text_field};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::target_verify::input::request_configuration::{
        canonical_path, configuration_identity, python_configuration_hash,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) struct Binding {
    pub receipt: Value,
    pub table: toml::Table,
    pub scope: String,
}

/// 正式登记只组合现有配置摘要和校验；不从环境选择目录，也不创建配置或运行收据。
pub(crate) fn contract(root: &Path, directory: &Path, api: &Path, worker: &Path) -> Result<Value> {
    let receipt = contract_inputs(root, directory, api, worker)?;
    super::source::verify(root, directory)?;
    if configuration_digest(api)? != receipt["api_configuration_sha256"]
        || configuration_digest(worker)? != receipt["worker_configuration_sha256"]
    {
        return Err("运行登记期间显式绑定配置发生变化，保留现场，禁止重放".into());
    }
    Ok(receipt)
}

fn contract_inputs(root: &Path, directory: &Path, api: &Path, worker: &Path) -> Result<Value> {
    if api.file_name().and_then(|v| v.to_str()) != Some("config")
        || worker.file_name().and_then(|v| v.to_str()) != Some("worker-config")
        || api.parent() != worker.parent()
    {
        return Err("运行登记必须显式绑定同一受管根内的 config 与 worker-config".into());
    }
    let mut tables = Vec::new();
    let mut digests = Vec::new();
    for path in [api, worker] {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
        tables.push(
            crate::rate_limit::load_app_table(root, Some(path), "test").map_err(
                |_| "运行登记配置读取失败；检查绑定目录的 app.test.toml 和字段类型，不回显配置正文",
            )?,
        );
        digests.push(configuration_digest(path)?);
    }
    let binaries = read_json(
        &directory.join("binaries.json"),
        "全栈二进制清单",
        64 * 1024,
    )?;
    let mut artifacts = serde_json::Map::new();
    for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        let path = Path::new(text_field(&binaries, name, "全栈二进制清单")?);
        validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
        let binding = file_binding(path, "运行产物")?;
        artifacts.insert(
            role.into(),
            json!({"path":binding["path"],"sha256":binding["sha256"]}),
        );
    }
    let scope = tables[0]
        .get("scope_id")
        .and_then(toml::Value::as_str)
        .ok_or("运行配置缺少 scope_id")?;
    let port = tables[1]
        .get("jobs")
        .and_then(toml::Value::as_table)
        .and_then(|jobs| jobs.get("health_port"))
        .and_then(toml::Value::as_integer)
        .filter(|v| (1..=65535).contains(v))
        .ok_or("运行配置 jobs.health_port 无效")?;
    let receipt = json!({"format_version":1,"backend_root":root,"scope_id":scope,
        "api_config_dir":api,"worker_config_dir":worker,"api_configuration_sha256":digests[0],
        "worker_configuration_sha256":digests[1],"configuration_sha256":python_configuration_hash(&json!(digests))?,
        "worker_ready_url":format!("http://127.0.0.1:{port}/readyz"),"artifacts":artifacts});
    validate_tables(&receipt, tables)?;
    verify_binaries(directory, &receipt)?;
    if configuration_digest(api)? != receipt["api_configuration_sha256"]
        || configuration_digest(worker)? != receipt["worker_configuration_sha256"]
    {
        return Err("运行登记期间显式绑定配置发生变化，保留现场，禁止重放".into());
    }
    Ok(receipt)
}

/// 首次发布仍须经过与读侧相同的完整来源和产物核验，不接受已有收据。
pub(in crate::recovery) fn register(
    root: &Path,
    directory: &Path,
    api: &Path,
    worker: &Path,
) -> Result<Value> {
    use crate::recovery::{clone::native::json::write_new, full_stack::provenance};
    let source = provenance::current(root, directory)?;
    if source.is_null() {
        return Err("运行登记来源与当前完整构建证据不匹配".into());
    }
    let runtime_path = directory.join("runtime.json");
    let evidence_path = directory.join("runtime-evidence.json");
    for path in [&runtime_path, &evidence_path] {
        validate_absolute_path(path, LocalTestPathKind::OutputFile)?;
    }
    let (receipt, build_binding) =
        super::source::verify_build_observed(root, directory, |recorded, build_binding| {
            if *recorded != source {
                return Err("运行登记来源与当前完整构建证据不匹配".into());
            }
            Ok((
                contract_inputs(root, directory, api, worker)?,
                build_binding.clone(),
            ))
        })?;
    write_new(&runtime_path, &receipt)?;
    write_new(
        &evidence_path,
        &super::source::runtime_evidence(directory, &source, &build_binding)?,
    )?;
    let actual = verify(root, directory)?.receipt;
    if actual != receipt {
        return Err("运行收据发布期间发生漂移，保留现场".into());
    }
    Ok(actual)
}

pub(crate) fn verify(root: &Path, directory: &Path) -> Result<Binding> {
    let binding = verify_configuration(root, directory)?;
    verify_binaries(directory, &binding.receipt)?;
    super::source::verify(root, directory)?;
    Ok(binding)
}

/// 重启事实只核对原运行配置合同；当前产物和源码漂移由完整核验单独报告。
pub(crate) fn verify_configuration(root: &Path, directory: &Path) -> Result<Binding> {
    let receipt = read_json(&directory.join("runtime.json"), "全栈运行收据", 64 * 1024)?;
    exact_object(
        &receipt,
        &[
            "format_version",
            "backend_root",
            "scope_id",
            "api_config_dir",
            "worker_config_dir",
            "api_configuration_sha256",
            "worker_configuration_sha256",
            "configuration_sha256",
            "worker_ready_url",
            "artifacts",
        ],
        "全栈运行收据",
    )?;
    let declared = PathBuf::from(text_field(&receipt, "backend_root", "运行收据")?);
    validate_absolute_path(&declared, LocalTestPathKind::ExistingDirectory)?;
    let canonical = crate::recovery::fixture_control::portable_path(&declared.canonicalize()?);
    if receipt["format_version"] != 1 || declared != canonical || declared != root {
        return Err("全栈运行收据版本或后端目录不匹配".into());
    }
    let api = PathBuf::from(text_field(&receipt, "api_config_dir", "运行收据")?);
    let worker = PathBuf::from(text_field(&receipt, "worker_config_dir", "运行收据")?);
    if api.file_name().and_then(|v| v.to_str()) != Some("config")
        || worker.file_name().and_then(|v| v.to_str()) != Some("worker-config")
        || api.parent() != worker.parent()
    {
        return Err("运行收据必须绑定同一受管配置根内的 config 与 worker-config".into());
    }
    let mut tables = Vec::new();
    let mut digests = Vec::new();
    for path in [&api, &worker] {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
        let table = crate::rate_limit::load_app_table(root, Some(path), "test")
            .map_err(|error| error.message().to_owned())?;
        let digest = configuration_digest(path)?;
        tables.push(table);
        digests.push(digest);
    }
    if receipt["api_configuration_sha256"] != digests[0]
        || receipt["worker_configuration_sha256"] != digests[1]
        || receipt["configuration_sha256"] != python_configuration_hash(&json!(digests))?
    {
        return Err("全栈运行收据与绑定文件配置摘要不匹配".into());
    }
    let (api_table, scope) = validate_tables(&receipt, tables)?;
    exact_object(&receipt["artifacts"], &["api", "worker"], "运行产物")?;
    for role in ["api", "worker"] {
        let artifact = &receipt["artifacts"][role];
        exact_object(artifact, &["path", "sha256"], "运行产物绑定")?;
        if !Path::new(text_field(artifact, "path", "运行产物绑定")?).is_absolute()
            || !text_field(artifact, "sha256", "运行产物绑定")?
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || text_field(artifact, "sha256", "运行产物绑定")?.len() != 64
        {
            return Err("运行产物绑定缺少绝对路径或规范摘要".into());
        }
    }
    Ok(Binding {
        receipt,
        table: api_table,
        scope,
    })
}

fn validate_tables(receipt: &Value, mut tables: Vec<toml::Table>) -> Result<(toml::Table, String)> {
    let mut api_table = tables.remove(0);
    let mut worker_table = tables.remove(0);
    let api_id = api_table
        .remove("snowflake_worker_id")
        .and_then(|v| v.as_integer());
    let worker_id = worker_table
        .remove("snowflake_worker_id")
        .and_then(|v| v.as_integer());
    if api_id.is_none_or(|v| !(0..=1023).contains(&v))
        || worker_id.is_none_or(|v| !(0..=1023).contains(&v))
        || api_id == worker_id
        || api_table != worker_table
    {
        return Err("API 与 Worker 的配置不匹配或实例编号重复".into());
    }
    let scope = api_table
        .get("scope_id")
        .and_then(toml::Value::as_str)
        .ok_or("配置缺少 scope_id")?
        .to_owned();
    if !regex::Regex::new(r"^[a-z0-9][a-z0-9-]{2,63}$")?.is_match(&scope)
        || receipt["scope_id"] != scope
    {
        return Err("全栈运行 scope 与配置不匹配".into());
    }
    let jobs = api_table
        .get("jobs")
        .and_then(toml::Value::as_table)
        .ok_or("配置缺少 jobs")?;
    let port = jobs
        .get("health_port")
        .and_then(toml::Value::as_integer)
        .filter(|v| (1..=65535).contains(v))
        .ok_or("jobs.health_port 无效")?;
    if jobs.get("mode").and_then(toml::Value::as_str) != Some("external")
        || jobs.get("health_host").and_then(toml::Value::as_str) != Some("127.0.0.1")
        || receipt["worker_ready_url"] != format!("http://127.0.0.1:{port}/readyz")
    {
        return Err("全栈 Worker 就绪端点与绑定配置不匹配".into());
    }
    Ok((api_table, scope))
}

pub(in crate::recovery) fn read_binaries(directory: &Path) -> Result<Value> {
    let binaries = read_json(
        &directory.join("binaries.json"),
        "全栈二进制清单",
        64 * 1024,
    )?;
    let roles = [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ];
    exact_object(&binaries, &roles, "全栈二进制清单")?;
    for name in roles {
        let path = PathBuf::from(text_field(&binaries, name, "二进制清单")?);
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    }
    Ok(binaries)
}

fn verify_binaries(directory: &Path, receipt: &Value) -> Result<()> {
    let binaries = read_binaries(directory)?;
    exact_object(&receipt["artifacts"], &["api", "worker"], "运行产物")?;
    for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        let path = PathBuf::from(text_field(&binaries, name, "二进制清单")?);
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        let binding = file_binding(&path, "运行产物")?;
        if receipt["artifacts"][role] != json!({"path":binding["path"], "sha256":binding["sha256"]})
        {
            return Err("全栈运行二进制与登记摘要不匹配".into());
        }
    }
    Ok(())
}

pub(in crate::recovery) fn configuration_digest(path: &Path) -> Result<String> {
    let mut files = BTreeMap::new();
    for name in ["app.toml", "app.test.toml"] {
        let file = path.join(name);
        if name == "app.toml" && !file.exists() {
            continue;
        }
        validate_absolute_path(&file, LocalTestPathKind::ExistingFile)?;
        files.insert(name.to_owned(), file_binding(&file, "绑定配置")?);
    }
    configuration_identity(&canonical_path(path)?, &files)
}
