use super::{
    super::{digest, json as document, resources::common},
    model,
};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn command(request: &Value, output: &Path, action: &str) -> Result<Vec<String>> {
    if request["format_version"] == 2 {
        return super::native_tool::from_request(request)?.arguments(request, output, action);
    }
    let payload = document::read(&output.join("launch-request.json"))?;
    Ok(vec![
        document::text(&request["wsl"]["path"], "WSL 工具")?.into(),
        "--distribution".into(),
        document::text(&request["distribution"], "WSL 发行版")?.into(),
        "--exec".into(),
        document::text(&request["python"]["path"], "Linux 解释器")?.into(),
        document::text(&payload["helpers"]["cache"]["path"], "冻结缓存辅助工具")?.into(),
        action.into(),
        "--request".into(),
        model::linux_path(document::text(
            &common::binding(&output.join("launch-request.json"))?["path"],
            "冻结请求路径",
        )?)?,
        "--sha256".into(),
        digest::file(&output.join("launch-request.json"))?.sha256,
    ])
}

fn frozen(request: &Value, output: &Path) -> Result<Value> {
    model::plain(output)?;
    let payload = document::read(&output.join("launch-request.json"))?;
    if payload["request"] != *request
        || payload["request_sha256"] != digest::plan_hash(request)?
        || payload["windows_output"] != output.to_str().ok_or("缓存输出路径不是UTF-8")?
    {
        return Err("Redis 冻结运行不属于明确请求及输出目录".into());
    }
    if request["format_version"] == 2 {
        let launch: super::linux_write::Launch = serde_json::from_value(payload.clone())?;
        if launch.format_version != 2
            || serde_json::to_value(&launch.native_tool)? != request["native_tool"]
        {
            return Err("缓存原生 launch 工具绑定不同".into());
        }
        launch.native_tool.validate()?;
        return Ok(payload);
    }
    document::exact(&payload["helpers"], &["cache", "process"], "缓存冻结工具")?;
    for value in payload["helpers"].as_object().unwrap().values() {
        document::exact(value, &["path", "bytes", "sha256"], "缓存冻结工具绑定")?;
        let path = document::text(&value["path"], "缓存冻结工具路径")?;
        let name = path
            .rsplit('/')
            .next()
            .filter(|v| !v.is_empty())
            .ok_or("冻结工具路径无效")?;
        let local = output.join("helpers").join(name);
        let mut expected = value.clone();
        expected["path"] = json!(local);
        if model::linux_path(local.to_str().ok_or("冻结工具路径不是UTF-8")?)? != path {
            return Err("缓存冻结工具跨系统路径不同".into());
        }
        model::file_bound(&expected)?;
    }
    Ok(payload)
}

pub(super) fn inspect(request: &Value, output: &Path) -> Result<Value> {
    model::plain(output)?;
    if !output.join("intent.json").exists() {
        if [
            "launcher.json",
            "linux-intent.json",
            "linux-process.json",
            "runtime.json",
        ]
        .iter()
        .any(|name| output.join(name).exists())
        {
            return Err("Redis 进程证据缺少先行意图".into());
        }
        return Ok(Value::Null);
    }
    let payload = frozen(request, output)?;
    if document::read(&output.join("intent.json"))?
        != json!({
            "request_sha256":digest::plan_hash(request)?, "arguments":command(request, output, "serve")?,
            "launch":common::binding(&output.join("launch-request.json"))?,
        })
    {
        return Err("缓存启动意图与冻结请求不同".into());
    }
    if output.join("partial.json").exists() {
        partial(request, output)
    } else {
        recorded(request, output, &payload)
    }
}

fn partial(request: &Value, output: &Path) -> Result<Value> {
    let proof = document::read(&output.join("partial.json"))?;
    document::exact(
        &proof,
        &[
            "state",
            "request_sha256",
            "intent",
            "launcher_pid",
            "launcher_returncode",
            "authorization_sent",
            "linux_cleanup",
        ],
        "缓存部分启动收据",
    )?;
    if !matches!(proof["state"].as_str(), Some("not_started" | "stopped"))
        || proof["request_sha256"] != digest::plan_hash(request)?
        || proof["intent"] != common::binding(&output.join("intent.json"))?
        || proof["launcher_pid"].as_u64().filter(|v| *v > 1).is_none()
        || proof["launcher_returncode"].as_i64().is_none()
    {
        return Err("Redis 部分启动退出证据不完整".into());
    }
    if proof["state"] == "not_started" {
        if proof["authorization_sent"] != false
            || !proof["linux_cleanup"].is_null()
            || output.join("linux-intent.json").exists()
        {
            return Err("Redis 已授权或存在启动意图，不能宣称未启动".into());
        }
    } else {
        let path = model::file_bound(&proof["linux_cleanup"])?;
        let failure = document::read(&path)?;
        let mut intent = common::binding(&output.join("linux-intent.json"))?;
        intent["path"] = json!(model::linux_path(document::text(
            &intent["path"],
            "Linux 意图路径"
        )?)?);
        if path != output.join("linux-failure.json")
            || failure["request_sha256"] != digest::plan_hash(request)?
            || failure["cleanup_completed"] != true
            || !failure["cleanup_error_type"].is_null()
            || failure["intent"] != intent
        {
            return Err("Redis Linux 监督器未证明完整回收".into());
        }
    }
    Ok(
        json!({"state":proof["state"],"request_sha256":digest::plan_hash(request)?,"output":output,
        "launch":common::binding(&output.join("launch-request.json"))?,"intent":common::binding(&output.join("intent.json"))?,
        "partial_receipt":common::binding(&output.join("partial.json"))?}),
    )
}

fn recorded(request: &Value, output: &Path, payload: &Value) -> Result<Value> {
    let launcher = document::read(&output.join("launcher.json"))?;
    document::exact(&launcher, &["identity", "intent"], "WSL 启动收据")?;
    model::identity(&launcher["identity"], false)?;
    if launcher["intent"] != common::binding(&output.join("intent.json"))?
        || !document::text(&launcher["identity"]["executable"], "WSL 启动产物")?
            .eq_ignore_ascii_case(document::text(&request["wsl"]["path"], "WSL 绑定工具")?)
    {
        return Err("WSL 启动进程缺少明确创建身份".into());
    }
    let linux = document::read(&output.join("linux-process.json"))?;
    let intent = document::read(&output.join("linux-intent.json"))?;
    let mut binding = common::binding(&output.join("linux-intent.json"))?;
    binding["path"] = json!(model::linux_path(document::text(
        &binding["path"],
        "Linux 意图路径"
    )?)?);
    if linux["request_sha256"] != digest::plan_hash(request)?
        || linux["intent"] != binding
        || intent["request_sha256"] != digest::plan_hash(request)?
        || intent["arguments"]
            != json!([
                request["launcher"],
                model::linux_path(document::text(
                    &request["configuration"]["path"],
                    "Redis 配置路径"
                )?)?
            ])
    {
        return Err("Redis Linux 身份不是同一启动意图".into());
    }
    model::identity(&linux["identity"], true)?;
    if linux["identity"]["executable"] != request["executable"] {
        return Err("Redis Linux 创建产物不符".into());
    }
    let mut value = json!({"format_version":1,"request_sha256":digest::plan_hash(request)?,"output":output,
        "launch":common::binding(&output.join("launch-request.json"))?,"launcher":launcher["identity"],
        "linux_identity":linux["identity"],"boot_id":linux["identity"]["boot_id"],
        "launcher_receipt":common::binding(&output.join("launcher.json"))?,
        "process_receipt":common::binding(&output.join("linux-process.json"))?});
    if request["format_version"] == 2 {
        value["format_version"] = json!(2);
        value["native_tool_sha256"] = json!(digest::plan_hash(&payload["native_tool"])?);
    } else {
        value["helpers_sha256"] = json!(digest::plan_hash(&payload["helpers"])?);
    }
    Ok(value)
}
