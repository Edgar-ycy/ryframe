use super::*;
use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{clone::native::s3::app_configuration, inputs::domain},
};
use std::{collections::BTreeSet, path::PathBuf};

pub(super) struct Authority {
    pub value: Value,
    pub secrets: Vec<String>,
    guards: Vec<Value>,
    held: Inputs,
    execution: PathBuf,
}
impl Authority {
    pub(super) fn verify(&self) -> Result<()> {
        self.held.check()?;
        for guard in &self.guards {
            unchanged(guard)?;
        }
        domain::validate_plan(&self.value, &self.execution)
    }
}
pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    safe(path)?;
    Ok(crate::recovery::fixture_control::portable_path(
        &fs::canonicalize(path)?,
    ))
}
pub(super) fn child(
    runtime: &Runtime,
    value: &Path,
    new: bool,
    directory: bool,
) -> Result<PathBuf> {
    safe(value)?;
    let kind = match (new, directory) {
        (true, true) => LocalTestPathKind::NewDirectory,
        (true, false) => LocalTestPathKind::OutputFile,
        (false, _) => LocalTestPathKind::ExistingFile,
    };
    validate_local_test_path(value, &runtime.execution, kind)?;
    let parent = canonical(value.parent().ok_or("参考数据路径缺少父目录")?)?;
    let selected = parent.join(value.file_name().ok_or("参考数据路径缺少文件名")?);
    check(
        parent == runtime.output
            && selected.starts_with(runtime.execution.join(".local-tests/reference-fixture")),
        "参考数据计划与工作目录必须是源运行时的直接子项",
    )?;
    check(
        !new || !selected.try_exists()?,
        "参考数据目标已存在，禁止覆盖或重放",
    )?;
    Ok(selected)
}
pub(super) fn build(
    backend: &Path,
    environment: &Path,
    runtime: &Runtime,
    work: &Path,
    side: &str,
) -> Result<Authority> {
    check(
        matches!(side, "base" | "candidate"),
        "参考恢复目标必须显式选择 base 或 candidate",
    )?;
    let work = child(runtime, work, true, true)?;
    let bootstrap_file = canonical(environment)?;
    let (bootstrap, bootstrap_guard) = document(&bootstrap_file)?;
    let review_file = path(&bootstrap["plan"]["review"]["path"])?;
    validate_local_test_path(&review_file, backend, LocalTestPathKind::ExistingFile)?;
    let review_binding = &bootstrap["plan"]["review"];
    descriptor(
        &json!({"path":review_binding["path"],"bytes":review_binding["bytes"],"sha256":review_binding["sha256"]}),
        &review_file,
    )?;
    let (review, review_guard) = document(&review_file)?;
    crate::recovery::clone::native::target_verify::input::request_review::validate(&review)?;
    let administrator = path(&bootstrap["administrator_credentials"]["path"])?;
    check(
        administrator
            == bootstrap_file
                .parent()
                .ok_or("环境缺少父目录")?
                .join("administrator.toml"),
        "夹具当前管理员凭据位置无效",
    )?;
    validate_local_test_path(
        &administrator,
        &runtime.execution,
        LocalTestPathKind::ExistingFile,
    )?;
    descriptor(&bootstrap["administrator_credentials"], &administrator)?;
    let configuration = app_configuration::configuration_value(
        &runtime.execution,
        &path(&runtime.receipt["api_config_dir"])?,
        "test",
    )?;
    let value = assemble(
        (&runtime.output, &runtime.receipt["scope_id"]),
        &work,
        side,
        &review,
        &configuration,
        (&bootstrap, &bootstrap_file, &review_file),
    )?;
    domain::validate_plan(&value, &runtime.execution)?;
    protect(
        runtime,
        &bootstrap,
        value,
        vec![
            bootstrap_guard,
            review_guard,
            snapshot(&administrator, MAX_BYTES)?,
        ],
    )
}
fn protect(
    runtime: &Runtime,
    bootstrap: &Value,
    value: Value,
    mut guards: Vec<Value>,
) -> Result<Authority> {
    let configuration = &value["dataset"]["configuration"];
    // 仅在内存中合并脱敏文件，不改 bootstrap 或参考计划的绑定事实。
    let mut redaction_files = bootstrap["secret_files"]
        .as_object()
        .ok_or("bootstrap缺少秘密绑定")?
        .clone();
    guards.extend(super::super::plan::input_guards(
        &value["fixture_bindings"],
    )?);
    let mut paths = guards
        .iter()
        .map(|g| path(&g["descriptor"]["path"]))
        .collect::<Result<BTreeSet<_>>>()?;
    for name in configuration["files"]
        .as_object()
        .ok_or("配置绑定缺少文件")?
        .keys()
    {
        let file = path(&configuration["directory"])?.join(name);
        guards.push(snapshot(&file, MAX_BYTES)?);
        paths.insert(file);
    }
    for scope in ["source", "target"] {
        for db in value[scope]["databases"]
            .as_array()
            .ok_or("参考数据缺少四库")?
        {
            let file = path(&db["defaults_file"])?;
            let guard = snapshot(&file, MAX_BYTES)?;
            redaction_files.insert(
                format!("dataset-defaults-{scope}-{}", text(&db["key"])?),
                guard["descriptor"].clone(),
            );
            guards.push(guard);
            paths.insert(file);
        }
    }
    for tool in ["mysql", "mysqldump"] {
        paths.insert(path(&value["tools"][tool]["path"])?);
    }
    paths.insert(std::env::current_exe()?);
    let held = Inputs::open(paths)?;
    let redaction_inputs = json!({"secret_files":redaction_files});
    let mut secrets = super::super::plan::secrets(&json!({}), &redaction_inputs)?;
    secrets.push(
        crate::recovery::clone::native::session::credentials::load_account(
            &runtime.execution,
            &bootstrap["administrator_credentials"],
            "system",
            "admin",
        )?,
    );
    let objects = app_configuration::private_configuration(&runtime.execution, configuration)?;
    for key in ["access_key", "secret_key"] {
        secrets.push(text(&objects[key])?.to_owned());
    }
    let authority = Authority {
        value,
        secrets,
        guards,
        held,
        execution: runtime.execution.clone(),
    };
    authority.verify()?;
    Ok(authority)
}
pub(super) fn assemble(
    runtime: (&Path, &Value),
    work: &Path,
    side: &str,
    review: &Value,
    configuration: &Value,
    documents: (&Value, &Path, &Path),
) -> Result<Value> {
    let (output, runtime_scope) = runtime;
    let (bootstrap, bootstrap_file, review_file) = documents;
    let mut source = scope(&review["scopes"]["seed"], configuration)?;
    source["runtime_dir"] = json!(output);
    check(
        source["scope_id"] == *runtime_scope,
        "夹具源运行时与审阅计划不属于同一代次",
    )?;
    let mut target = scope(&review["scopes"][side], configuration)?;
    target["worker_ready_url"] = review["scopes"][side]["worker_ready_url"].clone();
    let approved = &review["dataset"];
    let mut dataset = json!({"timeout_seconds":TIMEOUT_SECONDS,"api_validation_posts":2,"post_batch_rows":1000,
        "admin":{"tenant_id":"system","username":"admin"},"configuration":configuration,
        "administrator_credentials":bootstrap["administrator_credentials"]});
    for name in [
        "request_interval_ms",
        "tenant_targets",
        "records",
        "object_count",
        "object_bytes",
    ] {
        dataset[name] = approved.get(name).ok_or("审阅缺少数据规模或节奏")?.clone();
    }
    let mut tools = json!({});
    for name in ["mysql", "mysqldump"] {
        tools[name] =
            json!({"path":review["tools"][name]["path"],"sha256":review["tools"][name]["sha256"]});
    }
    let scope = text(&source["scope_id"])?;
    Ok(
        json!({"format_version":1,"id":format!("fixture-dataset-{}",scope.strip_prefix("fixture-seed-").unwrap_or(scope)),
        "target_side":side,"work_dir":work,"source":source,"target":target,"tools":tools,"dataset":dataset,
        "fixture_bindings":{"bootstrap":bound(bootstrap_file)?,"runtime":bound(&output.join("runtime.json"))?,
        "source_pair":bound(&output.join("source-pair.json"))?,"review":bound(review_file)?}}),
    )
}
pub(super) fn scope(value: &Value, configuration: &Value) -> Result<Value> {
    crate::recovery::clone::native::json::exact(
        configuration,
        &["app_env", "directory", "files", "sha256"],
        "参考数据配置绑定",
    )?;
    let values = value["databases"].as_array().ok_or("审阅侧缺少四库")?;
    let keys = values
        .iter()
        .map(|v| text(&v["key"]))
        .collect::<Result<BTreeSet<_>>>()?;
    let expected = domain::TARGET_DATABASES
        .iter()
        .map(|v| v.0)
        .collect::<BTreeSet<_>>();
    check(
        values.len() == 4 && keys == expected,
        "审阅侧缺少精确四库或存在重复",
    )?;
    let mut databases = vec![];
    for key in expected {
        let db = values
            .iter()
            .find(|v| v["key"] == key)
            .ok_or("数据库键缺失")?;
        let defaults = path(&db["connection_file"])?;
        let hash = snapshot(&defaults, MAX_BYTES)?;
        let (kind, mode) = domain::mode_for_key(key)?;
        databases.push(json!({"key":key,"kind":kind,"mode":mode,"database":db["database"],
            "server_uuid":db["expected_server_uuid"],"defaults_file":defaults,"defaults_sha256":hash["descriptor"]["sha256"]}));
    }
    Ok(
        json!({"scope_id":value["scope_id"],"runtime_dir":value["runtime_dir"],"api_url":value["api_url"],"frontend_url":value["frontend_url"],
        "s3":{"endpoint":value["objects"]["endpoint"],"region":value["objects"]["region"],"configuration":configuration},"databases":databases}),
    )
}
