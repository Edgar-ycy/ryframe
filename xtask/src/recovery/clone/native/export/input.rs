use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::Value;

use super::super::{
    digest,
    json::{self as json_io, exact, text},
    lifecycle::{manifest, stage::StageRequest, state},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const REQUEST_FIELDS: &[&str] = &[
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

/// 公开证据仅保留连接身份；原请求仍通过私有文件的字节数和摘要绑定。
pub(crate) fn public_request(backend: &Path, request: &Value) -> Result<Value> {
    let mut proof = request.clone();
    proof["source"]["s3"] = super::super::s3::app_configuration::public_configuration(
        backend,
        &request["source"]["s3"]["configuration"],
    )?;
    Ok(proof)
}

pub(crate) struct Context {
    pub(crate) backend: PathBuf,
    pub(crate) run_dir: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) mode: String,
    pub(crate) attempt: u64,
    pub(crate) manifest: Value,
    pub(crate) request: Value,
    pub(crate) object_identity: Value,
    pub(crate) request_path: PathBuf,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) execution_backend: PathBuf,
}

impl Context {
    pub(crate) fn verify_copy_export(&self, descriptor: &Value) -> Result<Value> {
        super::verify::binding(self, descriptor)
    }
    /// 复制后只读观察复用同一来源模型；不授予重新导出或来源登记资格。
    pub(crate) fn for_copy(backend: &Path, run: &Path, output: &Path) -> Result<Self> {
        let manifest = manifest::load(backend, &run.join("manifest.json"))?.value;
        Self::from_copy_manifest(backend, run, output, manifest)
    }

    pub(super) fn from_copy_manifest(
        backend: &Path,
        run: &Path,
        output: &Path,
        manifest: Value,
    ) -> Result<Self> {
        let request_path =
            manifest::bound_file(backend, &manifest["source_request"], "复制来源请求", true)?;
        let request = json_io::read(&request_path)?;
        validate_request(backend, &request)?;
        let environment = load_environment(backend, &manifest["source_environment"])?;
        let execution_backend = execution_backend(backend, &request)?;
        let object_identity = super::super::s3::app_configuration::public_configuration(
            backend,
            &request["source"]["s3"]["configuration"],
        )?;
        Ok(Self {
            backend: backend.to_owned(),
            run_dir: run.to_owned(),
            output_dir: output.to_owned(),
            mode: "copy-observation".into(),
            attempt: 0,
            manifest,
            request,
            object_identity,
            request_path,
            environment,
            execution_backend,
        })
    }

    pub(crate) fn load(backend: &Path, request: &StageRequest<'_>) -> Result<Self> {
        if request.stage != "export" {
            return Err(format!("源导出阶段不匹配：{}", request.stage).into());
        }
        let attempt = request.attempt.unwrap_or(0);
        if !matches!(request.mode, "run" | "reconcile" | "resume") {
            return Err("源导出模式无效".into());
        }
        let loaded = manifest::load(backend, &request.run_dir.join("manifest.json"))?;
        if loaded.value.get("copy_stage").and_then(Value::as_str) != Some("source_to_seed") {
            return Err("原生源导出只处理 source_to_seed 清单".into());
        }
        let request_path = manifest::bound_file(
            backend,
            loaded
                .value
                .get("source_request")
                .ok_or("复制运行清单缺少源请求")?,
            "源请求",
            true,
        )?;
        let request_value = json_io::read(&request_path)?;
        validate_request(backend, &request_value)?;
        let environment = load_environment(
            backend,
            loaded
                .value
                .get("source_environment")
                .ok_or("复制运行清单缺少来源环境")?,
        )?;
        let execution_backend = execution_backend(backend, &request_value)?;
        let object_identity = super::super::s3::app_configuration::public_configuration(
            backend,
            &request_value["source"]["s3"]["configuration"],
        )?;
        Ok(Self {
            backend: backend.to_path_buf(),
            run_dir: request.run_dir.to_path_buf(),
            output_dir: request.run_dir.join(format!("e{attempt:04}")),
            mode: request.mode.to_owned(),
            attempt,
            manifest: loaded.value,
            request: request_value,
            object_identity,
            request_path,
            environment,
            execution_backend,
        })
    }

    /// 仅构造当前来源登记代次；运行锁与完整来源证明由调用方负责。
    pub(crate) fn from_source_registration(
        backend: &Path,
        run: &Path,
        attempt: u64,
        source_request_binding: &Value,
        source_environment_binding: &Value,
    ) -> Result<Self> {
        let before = registration_attempt(run, attempt)?;
        let context = Self::from_bound_seed(
            backend,
            run,
            attempt,
            "source-register",
            source_request_binding,
            source_environment_binding,
        )?;
        if registration_attempt(run, attempt)? != before {
            return Err("来源登记上下文读取期间阶段状态变化".into());
        }
        Ok(context)
    }

    pub(crate) fn from_bound_seed(
        backend: &Path,
        run: &Path,
        attempt: u64,
        mode: &str,
        source_request_binding: &Value,
        source_environment_binding: &Value,
    ) -> Result<Self> {
        let manifest_path = run.join("manifest.json");
        let loaded = manifest::load(backend, &manifest_path)?;
        if loaded.value.get("copy_stage").and_then(Value::as_str) != Some("source_to_seed") {
            return Err("来源登记只处理 source_to_seed 清单".into());
        }
        let request_path =
            manifest::bound_file(backend, source_request_binding, "派生源请求", true)?;
        let request = json_io::read(&request_path)?;
        validate_request(backend, &request)?;
        let environment = load_environment(backend, source_environment_binding)?;
        super::super::seed::runtime_model::validate_api_environment(backend, &environment)?;
        let execution_backend = execution_backend(backend, &request)?;
        let object_identity = super::super::s3::app_configuration::public_configuration(
            backend,
            &request["source"]["s3"]["configuration"],
        )?;
        manifest::bound_file(backend, source_request_binding, "派生源请求", true)?;
        manifest::bound_file(backend, source_environment_binding, "来源环境", true)?;
        let after = manifest::load(backend, &manifest_path)?;
        if after.digest != loaded.digest || after.value != loaded.value {
            return Err("派生来源上下文读取期间清单变化".into());
        }
        Ok(Self {
            backend: backend.to_path_buf(),
            run_dir: run.to_path_buf(),
            output_dir: run.join(format!("e{attempt:04}")),
            mode: mode.into(),
            attempt,
            manifest: loaded.value,
            request,
            object_identity,
            request_path,
            environment,
            execution_backend,
        })
    }

    pub(crate) fn output(&self) -> PathBuf {
        self.output_dir.clone()
    }

    pub(crate) fn source(&self) -> &Value {
        self.request.get("source").expect("已校验来源请求")
    }

    pub(crate) fn databases(&self) -> Result<Vec<Value>> {
        let values = self
            .source()
            .get("databases")
            .and_then(Value::as_array)
            .ok_or("源数据库目标不能为空")?;
        Ok(values.clone())
    }

    pub(crate) fn tool(&self, name: &str) -> Result<PathBuf> {
        let path = self
            .request
            .pointer(&format!("/tools/{name}/path"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("源导出缺少外部工具 {name}"))?;
        let path = PathBuf::from(path);
        if !path.is_absolute() || digest::linked_path(&path)?.into_iter().any(|value| value) {
            return Err(format!("源导出外部工具 {name} 必须为无链接的绝对路径").into());
        }
        let expected = self
            .request
            .pointer(&format!("/tools/{name}/sha256"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("源导出外部工具 {name} 缺少摘要"))?;
        if digest::file(&path)?.sha256 != expected {
            return Err(format!("源导出外部工具 {name} 在计划确认后发生变化").into());
        }
        Ok(path)
    }

    pub(crate) fn maintenance_executable(&self, role: &str) -> Result<PathBuf> {
        let path = manifest::bound_file(
            &self.execution_backend,
            self.request
                .get("maintenance_build")
                .ok_or("源导出缺少维护构建")?,
            "维护构建",
            true,
        )?;
        let receipt = json_io::read(&path)?;
        let executable = receipt
            .pointer(&format!("/artifacts/{role}/executable"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("维护构建缺少 {role} 工具"))?;
        let executable = PathBuf::from(executable);
        if !executable.is_absolute() {
            return Err(format!("维护构建 {role} 工具必须是绝对路径").into());
        }
        Ok(executable)
    }

    pub(crate) fn source_export(&self) -> Option<&Value> {
        self.manifest
            .get("source_export")
            .filter(|value| !value.is_null())
    }
}

pub(crate) fn validate_request(backend: &Path, request: &Value) -> Result<()> {
    let value = exact(request, REQUEST_FIELDS, "源导出请求")?;
    if value.get("format_version").and_then(Value::as_u64) != Some(1)
        || value.get("kind").and_then(Value::as_str) != Some("devex-clone-source-export")
    {
        return Err("源导出请求类型无效".into());
    }
    validate_name(text(value.get("id").unwrap(), "源导出 ID")?)?;
    validate_source(backend, value.get("source").unwrap())?;
    let fingerprint = text(value.get("worktree_fingerprint").unwrap(), "工作区指纹")?;
    if !Regex::new(r"^sha256:[a-f0-9]{64}$")?.is_match(fingerprint) {
        return Err("源导出须绑定完整工作区指纹".into());
    }
    let maximum = value
        .get("max_object_bytes")
        .and_then(Value::as_u64)
        .ok_or("源导出缺少对象大小上限")?;
    if maximum == 0 {
        return Err("源导出对象大小上限必须大于零".into());
    }
    let tools = exact(
        value.get("tools").unwrap(),
        &["mysql", "mysqldump", "node"],
        "源导出外部工具",
    )?;
    for item in tools.values() {
        let item = exact(item, &["path", "sha256"], "源导出外部工具")?;
        let path = PathBuf::from(text(item.get("path").unwrap(), "外部工具路径")?);
        let expected = text(item.get("sha256").unwrap(), "外部工具摘要")?;
        if !path.is_absolute()
            || digest::linked_path(&path)?.into_iter().any(|value| value)
            || digest::file(&path)?.sha256 != expected
        {
            return Err("源导出外部工具实际文件与绑定不符".into());
        }
    }
    for field in ["backend_build", "runtime", "producers_registry"] {
        manifest::bound_file(backend, value.get(field).unwrap(), field, true)?;
    }
    let runtime_path =
        manifest::bound_file(backend, value.get("runtime").unwrap(), "运行收据", true)?;
    let runtime = json_io::read(&runtime_path)?;
    let execution = PathBuf::from(
        runtime
            .get("backend_root")
            .and_then(Value::as_str)
            .ok_or("源 runtime 缺少执行后端")?,
    );
    if !execution.is_absolute() || !execution.is_dir() {
        return Err("源 runtime 的执行后端必须为绝对目录".into());
    }
    manifest::bound_file(
        &execution,
        value.get("maintenance_build").unwrap(),
        "维护构建",
        true,
    )?;
    let processes = exact(
        value.get("processes").unwrap(),
        &["api", "worker"],
        "源进程收据",
    )?;
    let directory = PathBuf::from(
        value
            .get("source")
            .and_then(|value| value.get("runtime_dir"))
            .and_then(Value::as_str)
            .ok_or("源请求缺少运行目录")?,
    );
    if runtime_path != directory.join("runtime.json") {
        return Err("运行收据不属于源明确运行目录".into());
    }
    for (role, binding) in processes {
        if manifest::bound_file(backend, binding, "源进程收据", true)?
            != directory.join(format!("{role}.json"))
        {
            return Err("进程收据不属于源明确运行目录".into());
        }
    }
    Ok(())
}

fn validate_source(backend: &Path, value: &Value) -> Result<()> {
    let source = exact(
        value,
        &["scope_id", "runtime_dir", "api_url", "s3", "databases"],
        "源配置",
    )?;
    validate_name(text(source.get("scope_id").unwrap(), "scope_id")?)?;
    local_path(
        backend,
        text(source.get("runtime_dir").unwrap(), "运行目录")?,
        LocalTestPathKind::ExistingDirectory,
    )?;
    endpoint(text(source.get("api_url").unwrap(), "API 地址")?)?;
    exact(source.get("s3").unwrap(), &["configuration"], "源对象配置")?;
    super::super::s3::Configuration::from_app_binding(backend, &source["s3"]["configuration"])?;
    let databases = source
        .get("databases")
        .and_then(Value::as_array)
        .ok_or("源数据库目标不能为空")?;
    if databases.is_empty() {
        return Err("源数据库目标不能为空".into());
    }
    let mut keys = BTreeSet::new();
    let mut physical = BTreeSet::new();
    let database_name = Regex::new(r"^[a-z0-9_]{1,64}$")?;
    let server_uuid = Regex::new(r"^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$")?;
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
            "源数据库目标",
        )?;
        let key = validate_name(text(database.get("key").unwrap(), "数据库键")?)?;
        let name = text(database.get("database").unwrap(), "database")?;
        let uuid = text(database.get("server_uuid").unwrap(), "server_uuid")?;
        let kind = text(database.get("kind").unwrap(), "数据库类型")?;
        let mode = text(database.get("mode").unwrap(), "数据库模式")?;
        if !database_name.is_match(name)
            || !server_uuid.is_match(uuid)
            || !matches!(kind, "combined" | "tenant")
            || !matches!(mode, "shared" | "dedicated")
            || (key == "shared-control") != (kind == "combined")
            || (kind == "combined" && mode != "shared")
            || !keys.insert(key.to_owned())
            || !physical.insert((uuid.to_owned(), name.to_ascii_lowercase()))
        {
            return Err("源数据库逻辑目标重复、物理身份重叠或类型不符".into());
        }
        let defaults = PathBuf::from(text(
            database.get("defaults_file").unwrap(),
            "MySQL 凭据文件",
        )?);
        validate_local_test_path(&defaults, backend, LocalTestPathKind::ExistingFile)
            .map_err(|error| format!("源数据库凭据文件无效：{error}"))?;
        let expected = text(database.get("defaults_sha256").unwrap(), "MySQL 凭据摘要")?;
        if digest::file(&defaults)?.sha256 != expected {
            return Err("源数据库凭据文件已变化".into());
        }
    }
    if !keys.contains("shared-control") {
        return Err("源必须明确包含 combined 控制目标".into());
    }
    Ok(())
}

fn load_environment(backend: &Path, descriptor: &Value) -> Result<BTreeMap<String, String>> {
    let path = manifest::bound_file(backend, descriptor, "来源环境", true)?;
    let value = json_io::read(&path)?;
    let root = exact(&value, &["environment"], "来源环境")?;
    let values = json_io::object(root.get("environment").unwrap(), "来源环境变量")?;
    values
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .ok_or_else(|| format!("来源环境变量 {key} 必须为字符串"))?;
            Ok((key.clone(), value.to_owned()))
        })
        .collect()
}

pub(crate) fn execution_backend(backend: &Path, request: &Value) -> Result<PathBuf> {
    let path = manifest::bound_file(backend, request.get("runtime").unwrap(), "运行收据", true)?;
    let value = json_io::read(&path)?;
    let root = PathBuf::from(
        value
            .get("backend_root")
            .and_then(Value::as_str)
            .ok_or("源 runtime 缺少执行后端")?,
    );
    if !root.is_absolute()
        || !root.is_dir()
        || digest::linked_path(&root)?.into_iter().any(|value| value)
    {
        return Err("源 runtime 的执行后端必须是无链接的规范绝对目录".into());
    }
    Ok(root)
}

fn local_path(backend: &Path, value: &str, kind: LocalTestPathKind) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    validate_local_test_path(&path, backend, kind)
        .map(|_| path)
        .map_err(|error| format!("源导出本地路径无效：{error}").into())
}

pub(super) fn validate_name(value: &str) -> Result<&str> {
    if Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$")?.is_match(value) {
        Ok(value)
    } else {
        Err("开发复制名称无效".into())
    }
}

fn endpoint(value: &str) -> Result<()> {
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .ok_or("源端点只接受 HTTP 根地址")?;
    let host = rest.split('/').next().unwrap_or_default();
    let (name, port) = host.rsplit_once(':').ok_or("源端点必须包含明确端口")?;
    if !matches!(name, "127.0.0.1" | "localhost" | "[::1]")
        || port.is_empty()
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || port.parse::<u16>().ok().is_none_or(|value| value == 0)
    {
        return Err("源端点只接受明确的本机 loopback 端口".into());
    }
    Ok(())
}

fn registration_attempt(run: &Path, number: u64) -> Result<Value> {
    if number == 0 {
        return Err("来源登记代次必须大于零".into());
    }
    let value = state::load(run, true)?;
    let running = state::running_attempt(&value)?.ok_or("来源登记缺少运行中阶段")?;
    if running.get("number").and_then(Value::as_u64) != Some(number)
        || running.get("stage").and_then(Value::as_str) != Some("seed-runtime")
        || running.get("mode").and_then(Value::as_str) != Some("source-register")
    {
        return Err("来源登记必须属于当前 seed-runtime/source-register 运行代次".into());
    }
    Ok(value)
}
