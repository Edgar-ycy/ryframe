use super::{super::json as document, evidence, model};
use crate::Result;
use serde_json::Value;
use std::{path::Path, process::Output, time::Duration};

#[derive(Debug)]
pub(super) struct ControlFailure(pub(super) Option<i32>);

impl std::fmt::Display for ControlFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Redis 精确身份操作失败，原始退出码 {:?}；保留现场，禁止重放",
            self.0
        )
    }
}

impl std::error::Error for ControlFailure {}

pub(super) fn call(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    action: &str,
) -> Result<Value> {
    model::validate(backend, request, true)?;
    call_with(request, runtime, action, |args, input| {
        crate::process::command_output_with_input_timeout(
            backend,
            &args[0],
            &args[1..].iter().map(String::as_str).collect::<Vec<_>>(),
            &[],
            Some(input),
            Duration::from_secs(20),
        )
    })
}

pub(super) fn call_with(
    request: &Value,
    runtime: &Value,
    action: &str,
    mut run: impl FnMut(&[String], &[u8]) -> Result<Output>,
) -> Result<Value> {
    if !matches!(action, "observe" | "check" | "stop") {
        return Err("缓存控制仅允许明确观察或精确停止".into());
    }
    let output = Path::new(document::text(&runtime["output"], "缓存运行证据目录")?);
    let observed = evidence::inspect(request, output)?;
    let fields = observed.as_object().ok_or("缓存运行缺少创建收据")?;
    if fields
        .iter()
        .any(|(key, value)| runtime.get(key) != Some(value))
    {
        return Err("Redis 运行收据与实际创建身份不同".into());
    }
    model::identity(&runtime["linux_identity"], true)?;
    let args = evidence::command(request, output, action)?;
    let input = serde_json::to_vec(&runtime["linux_identity"])?;
    let result = run(&args, &input)?;
    if !result.status.success() {
        return Err(ControlFailure(result.status.code()).into());
    }
    validate_response(&result.stdout, &runtime["linux_identity"])
}

pub(super) fn validate_response(bytes: &[u8], identity: &Value) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > 16384 {
        return Err("缓存进程响应为空或超过16KiB".into());
    }
    let value = document::parse_fragment(std::str::from_utf8(bytes)?, "缓存精确进程响应")?;
    document::exact(
        &value,
        &["identity", "alive", "boot_id", "terminated"],
        "缓存进程响应",
    )?;
    if value["identity"] != *identity
        || value["alive"].as_bool().is_none()
        || value["terminated"].as_bool().is_none()
        || !model::boot(&value["boot_id"])?
    {
        return Err("缓存控制返回其他 Redis 进程身份".into());
    }
    Ok(value)
}
