use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{StorageObserver, TargetContext, common, json_io, kernel};
use crate::Result;

pub(crate) fn verify<O: StorageObserver>(
    context: &TargetContext,
    review: &Value,
    rustfs: &Value,
    runtime: Option<&Value>,
    observer: &O,
) -> Result<()> {
    json_io::exact(
        rustfs,
        &["identity", "sha256", "process_receipt", "launch_receipt"],
        "目标 RustFS",
    )?;
    let identity = common::validate_process_identity(&rustfs["identity"])?;
    let executable = Path::new(json_io::text(&identity["executable"], "RustFS 产物路径")?);
    let expected = review
        .pointer("/services/rustfs")
        .ok_or("目标审阅缺少 RustFS 服务")?;
    common::scope_id(json_io::text(&expected["scope_id"], "RustFS scope")?)?;
    if review.pointer("/tools/rustfs/path") != identity.get("executable")
        || review.pointer("/tools/rustfs/sha256") != rustfs.get("sha256")
        || super::super::super::digest::linked_path(executable)?
            .into_iter()
            .any(|linked| linked)
        || super::super::super::digest::file(executable)?.sha256
            != json_io::text(&rustfs["sha256"], "RustFS 摘要")?
    {
        return Err("RustFS 实际二进制不同于登记工具".into());
    }
    kernel::require_observed_identity(&identity, observer)?;
    let directory = common::local_directory(
        &context.backend,
        Path::new(json_io::text(&expected["data_dir"], "RustFS 数据目录")?),
        "RustFS 数据目录",
    )?;
    validate_directory(&context.backend, expected, rustfs, runtime, &directory)?;
    let receipt = receipt(context, expected, rustfs, runtime)?;
    let launch_path =
        common::bound_file(&context.backend, &rustfs["launch_receipt"], "RustFS launch")?;
    if launch_path
        != receipt
            .parent()
            .ok_or("RustFS 收据缺少父目录")?
            .join("launch.json")
    {
        return Err("RustFS launch 必须属于同一存储目录".into());
    }
    let launch = json_io::read(&launch_path)?;
    let arguments = arguments(expected, &identity, &directory)?;
    validate_launch(context, expected, &identity, &launch, &arguments)?;
    kernel::require_observed_identity(&identity, observer)?;
    let actual_arguments = observer.arguments(context, &identity)?;
    kernel::require_observed_identity(&identity, observer)?;
    if actual_arguments != arguments {
        return Err("RustFS 当前完整参数不同于受控启动声明".into());
    }
    for field in ["api", "console"] {
        observer.listener(
            context,
            &identity,
            json_io::text(&expected[field], "RustFS 监听地址")?,
        )?;
    }
    kernel::require_observed_identity(&identity, observer)?;
    let mut evidence = json!({
        "process_receipt": rustfs["process_receipt"], "launch_receipt": rustfs["launch_receipt"],
        "actual_arguments_sha256": super::super::super::digest::plan_hash(&json!(arguments))?,
        "data_dir": directory,
        "environment_proof": "受控启动收据与凭据文件；不声称从内核枚举进程环境",
    });
    if let Some(runtime) = runtime {
        evidence["runtime_transition"] = runtime.clone();
    }
    json_io::write_new(
        &context.observation_dir.join(format!(
            "storage-layout-{}.json",
            uuid::Uuid::new_v4().simple()
        )),
        &evidence,
    )
}

fn receipt(
    context: &TargetContext,
    expected: &Value,
    rustfs: &Value,
    runtime: Option<&Value>,
) -> Result<PathBuf> {
    let path = common::bound_file(
        &context.backend,
        &rustfs["process_receipt"],
        "RustFS 进程收据",
    )?;
    let expected_path = runtime
        .map(|value| &value["storage"]["process_receipt"]["path"])
        .unwrap_or(&expected["process_receipt"]);
    if path != Path::new(json_io::text(expected_path, "RustFS 进程收据路径")?) {
        return Err("RustFS 进程收据不是审阅或重启登记路径".into());
    }
    validate_receipt(&json_io::read(&path)?, expected, &rustfs["identity"])?;
    Ok(path)
}

pub(crate) fn validate_receipt(receipt: &Value, expected: &Value, identity: &Value) -> Result<()> {
    let stable = json!({
        "format_version": 1, "role": "rustfs", "scope_id": expected["scope_id"],
        "lifecycle": "running", "identity": identity, "api_url": expected["api"],
        "console_url": expected["console"], "data_dir": expected["data_dir"],
    });
    if stable
        .as_object()
        .ok_or("RustFS 固定字段无效")?
        .iter()
        .any(|(key, value)| receipt.get(key) != Some(value))
    {
        return Err("RustFS 实际启动目录或进程收据不同".into());
    }
    Ok(())
}

pub(crate) fn validate_directory(
    backend: &Path,
    expected: &Value,
    rustfs: &Value,
    runtime: Option<&Value>,
    directory: &Path,
) -> Result<()> {
    if let Some(runtime) = runtime
        && (runtime.get("storage") != Some(rustfs)
            || runtime.get("api_url") != expected.get("api")
            || runtime.get("console_url") != expected.get("console")
            || common::directory_identity(backend, &runtime["data_directory"])? != directory)
    {
        return Err("RustFS 重启不是原物理目录、端口或本次运行身份".into());
    }
    Ok(())
}

pub(crate) fn arguments(
    expected: &Value,
    identity: &Value,
    directory: &Path,
) -> Result<Vec<String>> {
    let (_, api) = common::endpoint(json_io::text(&expected["api"], "RustFS API 地址")?)?;
    let (_, console) =
        common::endpoint(json_io::text(&expected["console"], "RustFS console 地址")?)?;
    Ok(vec![
        json_io::text(&identity["executable"], "RustFS executable")?.into(),
        "server".into(),
        "--address".into(),
        api,
        "--console-address".into(),
        console,
        common::path_text(directory, "RustFS 数据目录")?,
    ])
}

pub(crate) fn validate_launch(
    context: &TargetContext,
    expected: &Value,
    identity: &Value,
    launch: &Value,
    arguments: &[String],
) -> Result<()> {
    json_io::exact(
        launch,
        &[
            "format_version",
            "scope_id",
            "identity",
            "arguments",
            "environment",
            "credential_files",
        ],
        "RustFS launch",
    )?;
    let credentials = json_io::exact(
        &launch["credential_files"],
        &["access_key", "secret_key"],
        "RustFS 凭据文件",
    )?;
    let mut environment = json!({"RUSTFS_CONSOLE_ENABLE": "true"});
    for (field, binding) in credentials {
        let credential = common::bound_file(&context.backend, binding, "RustFS 凭据文件")?;
        let expected_credential = context
            .request
            .pointer(&format!("/target/s3/{field}"))
            .and_then(Value::as_str)
            .ok_or("RustFS 凭据缺少已绑定文件中的明确值")?;
        let actual =
            fs::read_to_string(&credential).map_err(|_| "无法读取 RustFS 凭据文件 UTF-8 文本")?;
        if expected_credential != actual.trim() {
            return Err("RustFS 启动凭据文件与本次认证不同".into());
        }
        environment[format!("RUSTFS_{}_FILE", field.to_uppercase())] = binding["path"].clone();
    }
    if launch["format_version"].as_u64() != Some(1)
        || launch.get("scope_id") != expected.get("scope_id")
        || launch.get("identity") != Some(identity)
        || launch["arguments"] != json!(arguments)
        || launch["environment"] != environment
    {
        return Err("RustFS 受控启动参数或环境声明不同".into());
    }
    Ok(())
}
