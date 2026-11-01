//! 已准备环境只读核验，凭据固定到普通文件与精确摘要。
use super::*;
use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};
use crate::recovery::clone::native::{digest, lifecycle::manifest};

pub(super) fn load(backend: &Path, selected: &Path, names: &Value) -> Result<Value> {
    let (receipt_path, receipt) = context::document(backend, selected)?;
    let names = names.as_array().ok_or("秘密文件清单必须为数组")?;
    check(
        receipt["format_version"] == 1
            && receipt["kind"] == "reference-fixture-environment"
            && receipt["status"] == "prepared"
            && receipt["services_started"] == false
            && receipt["remote_writes"] == 0
            && receipt["historical_data_used"] == false
            && receipt["execution_backend"].is_string()
            && receipt["environment_sha256"].is_string(),
        "夹具运行时只能使用尚未启动服务的已准备私有环境",
    )?;
    let secrets = receipt["secret_files"]
        .as_object()
        .ok_or("夹具秘密文件描述不完整")?;
    check(
        names.len() == secrets.len()
            && names
                .iter()
                .all(|name| name.as_str().is_some_and(|name| secrets.contains_key(name))),
        "夹具秘密文件清单与 bootstrap 不同",
    )?;
    let execution = PathBuf::from(text(&receipt, "execution_backend")?);
    validate_local_test_path(&execution, backend, LocalTestPathKind::ExistingDirectory)?;
    check(execution.join("Cargo.toml").is_file(), "夹具执行工作树无效")?;
    let source = publication(backend, &receipt)?;
    configuration(&execution, &receipt)?;
    let private = private_environment(backend, &receipt_path, &receipt)?;
    for name in names {
        let name = name.as_str().ok_or("秘密文件名必须为字符串")?;
        let descriptor = &secrets[name];
        let path = manifest::bound_file(backend, descriptor, "夹具秘密文件", true)
            .map_err(|_| model::error("夹具秘密文件与 bootstrap 绑定不一致"))?;
        check(
            path.file_name().is_some_and(|file| file == name) && binding(&path)? == *descriptor,
            "夹具秘密文件与 bootstrap 绑定不一致",
        )?;
    }
    check(
        publication(backend, &receipt)? == source,
        "prepared 读取期间完整执行来源变化",
    )?;
    check(
        context::document(backend, selected)?.1 == receipt,
        "prepared 收据在核验期间变化",
    )?;
    Ok(json!([receipt_path, execution, private, receipt]))
}

fn publication(backend: &Path, receipt: &Value) -> Result<Value> {
    check(
        receipt["plan"].get("execution_source").is_some(),
        "prepared 缺少执行源码发布绑定；使用正式 execution-source 准备新环境，不升级或改写旧收据",
    )?;
    let inputs = super::super::super::environment::plan::inputs(
        &json!({"backend":backend,"plan":receipt["plan"]}),
    )?;
    check(
        receipt["plan"]["execution_backend"] == receipt["execution_backend"],
        "已发布环境执行根与计划不一致",
    )?;
    Ok(inputs)
}

fn configuration(execution: &Path, receipt: &Value) -> Result<()> {
    let config = PathBuf::from(text(receipt, "config_dir")?);
    validate_local_test_path(&config, execution, LocalTestPathKind::ExistingDirectory)?;
    check(
        super::super::super::artifact::runtime::configuration_digest(&config)?
            == receipt["configuration_sha256"],
        "夹具配置目录或配置文件与 bootstrap 绑定不一致",
    )?;
    Ok(())
}

fn private_environment(backend: &Path, receipt_path: &Path, receipt: &Value) -> Result<Value> {
    let (_, environment) = context::document(
        backend,
        &receipt_path
            .parent()
            .ok_or("环境收据没有父目录")?
            .join("environment.json"),
    )?;
    json_io::exact(&environment, &["environment"], "夹具私有环境文件")?;
    let private = &environment["environment"];
    let values = private.as_object().ok_or("夹具私有环境文件字段无效")?;
    check(
        values.values().all(Value::is_string)
            && digest::plan_hash(private)? == receipt["environment_sha256"],
        "夹具私有环境与 bootstrap 摘要不一致",
    )?;
    check(
        values
            .keys()
            .all(|key| ["APP_ENV", "TEMP", "TMP"].contains(&key.as_str()))
            && private["APP_ENV"] == "test",
        "夹具进程环境只允许 APP_ENV 与受管临时目录",
    )?;
    Ok(private.clone())
}
