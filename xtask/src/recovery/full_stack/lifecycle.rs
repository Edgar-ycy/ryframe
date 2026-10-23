use super::{
    Port,
    context::{Context, journal},
};
use crate::{Result, recovery::clone::native::json as documents};
use serde_json::json;
use std::{fs, time::Duration};

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
    let result: Result<()> = (|| {
        port.leaf("api-launch")?;
        wait_ready(port, &url)?;
        port.leaf("api-probe")?;
        port.leaf("worker-start")?;
        Ok(())
    })();
    if result.is_err() {
        let cleanup = port.leaf("api-stop");
        journal(
            &receipt,
            &json!({"format_version":1,"state":"start-failed","api_cleanup_confirmed":cleanup.is_ok(),"automatic_retry_permitted":false}),
        )?;
        return match (result, cleanup) {
            (Err(error), Ok(_)) => Err(format!(
                "全栈启动失败，API 已按归属回收：{error}；核对 Worker 收据，不自动重试"
            )
            .into()),
            (Err(error), Err(cleanup)) => {
                Err(format!("全栈启动失败：{error}；API 回收未确认：{cleanup}；保留现场").into())
            }
            _ => unreachable!(),
        };
    }
    journal(&receipt, &json!({"format_version":1,"state":"running"}))
}

pub(super) fn wait_ready(port: &mut impl Port, url: &str) -> Result<()> {
    for _ in 0..90 {
        port.leaf("api-alive")?;
        if port.ready(url)? {
            return Ok(());
        }
        // 探测同时验证创建身份与监听归属，早退或 PID 复用立即停止等待。
        // HTTP 尚未就绪时 listener 可能不存在，因此仅在成功后核验 listener。
        port.sleep(Duration::from_secs(2))?;
    }
    Err("API 就绪等待超时".into())
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
    if ["api.json", "api-tree.json"]
        .iter()
        .any(|v| context.output.join(v).exists())
        && port.leaf("api-stop").is_err()
    {
        failures.push("API 回收未确认");
    }
    if port.leaf("api-port-free").is_err() {
        failures.push("API 端口未确认释放");
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
