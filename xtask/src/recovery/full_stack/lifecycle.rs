use super::{
    Port,
    context::{Context, journal},
};
use crate::{Result, recovery::clone::native::json as documents};
use serde_json::json;
use std::fs;

pub(super) fn start(context: &Context, port: &mut impl Port) -> Result<()> {
    let url = context.api_url()?;
    let runtime = port.leaf("register")?;
    if runtime["worker_ready_url"] == url {
        return Err("API 和 Worker 必须使用不同的监听端口".into());
    }
    let receipt = context.output.join("ci-start.json");
    documents::write_new(
        &receipt,
        &json!({"format_version":1,"state":"starting","automatic_retry_permitted":false}),
    )?;
    let record_failure = |api_cleanup_confirmed| {
        journal(
            &receipt,
            &json!({"format_version":1,"state":"start-failed","api_cleanup_confirmed":api_cleanup_confirmed,"automatic_retry_permitted":false}),
        )
    };
    if let Err(error) = port.leaf("api-launch") {
        record_failure(false)?;
        return Err(format!(
            "全栈 API 启动未确认：{error}；保留现场，核对原启动收据后显式执行 collect，不自动重放"
        )
        .into());
    }
    if let Err(error) = port
        .leaf("api-probe")
        .and_then(|_| port.leaf("worker-start"))
    {
        let cleanup = port.leaf("api-stop");
        record_failure(cleanup.is_ok())?;
        return match cleanup {
            Ok(cleanup) => {
                let status = if cleanup["state"] == "stopped" {
                    "API 已按归属回收"
                } else {
                    "未发现已登记的 API 进程"
                };
                Err(format!("全栈启动失败，{status}：{error}；核对 Worker 收据，不自动重试").into())
            }
            Err(cleanup) => {
                Err(format!("全栈启动失败：{error}；API 回收未确认：{cleanup}；保留现场").into())
            }
        };
    }
    journal(&receipt, &json!({"format_version":1,"state":"running"}))
}

pub(super) fn collect(context: &Context, port: &mut impl Port) -> Result<()> {
    fs::create_dir_all(&context.output)?;
    let mut failures = Vec::new();
    if context.output.join("runtime.json").is_file() {
        if port.leaf("worker-stop").is_err() {
            failures.push("Worker 回收未确认");
        }
    } else if ["worker.json", "worker-control.lock"]
        .iter()
        .any(|v| context.output.join(v).exists())
    {
        failures.push("Worker 存在控制收据但缺少可信 runtime");
    }
    if port.leaf("api-stop").is_err() {
        failures.push("API 回收未确认");
    }
    match port.leaf("api-port-free") {
        Ok(value) if value["state"] == "free" => {}
        Ok(value) if value["state"] == "configuration-unavailable" => {
            failures.push("无法从绑定配置确定 API 端口，未执行端口检查");
        }
        _ => failures.push("API 端口未确认释放"),
    }
    for (role, name) in [
        ("mysql", "mysql.log"),
        ("redis", "redis.log"),
        ("s3", "rustfs.log"),
    ] {
        match context.container(role) {
            Ok(container) => {
                let _ = port.command(
                    "docker",
                    &["logs".into(), container.into()],
                    Some(&context.output.join(name)),
                );
            }
            Err(_) => failures.push("诊断容器绑定无效"),
        }
    }
    for operation in ["--show-stats", "--stop-server"] {
        let _ = port.command(
            "sccache",
            &[operation.into()],
            Some(&context.output.join("sccache.log")),
        );
    }
    if !failures.is_empty() {
        fs::write(
            context.output.join("cleanup-errors.log"),
            failures.join("\n"),
        )?;
        return Err(format!(
            "全栈进程未安全回收：{}；保留收据，不按 PID 或端口猜测归属",
            failures.join("；")
        )
        .into());
    }
    Ok(())
}
