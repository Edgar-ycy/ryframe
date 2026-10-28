//! 缓存原生控制的固定诊断；不转发配置、凭据或远端 stderr。
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Category {
    RequestRejected,
    ToolUidMismatch,
    ToolImageMismatch,
    ToolSourceMismatch,
    ConfigurationDrift,
    IdentityMismatch,
    ToolConfigurationRejected,
    ParentAuthorizationRejected,
    ExecFailed,
    PidfdUnsupported,
    PidfdPermissionDenied,
    PidfdOpenFailed,
    SignalFailed,
    ExitWaitFailed,
    ExecWaitTimeout,
    AcknowledgementTimeout,
    SupervisorInterrupted,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    NotStarted,
    Unknown,
    Stopped,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Diagnostic {
    pub category: Category,
    pub write_outcome: Outcome,
    pub os_code: Option<i32>,
}

impl Diagnostic {
    pub(super) fn decode_output(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > 16384 {
            return Err("缓存失败诊断为空或超限".into());
        }
        let value =
            super::super::json::parse_fragment(std::str::from_utf8(bytes)?, "缓存固定失败响应")?;
        super::super::json::exact(&value, &["ok", "error", "diagnostic"], "缓存固定失败响应")?;
        if value["ok"] != false || value["error"] != "NativeCacheRejected" {
            return Err("缓存失败诊断类型无效".into());
        }
        Self::decode(&value["diagnostic"])
    }
    fn fields(&self) -> (&'static str, &'static str, &'static str) {
        use Category::*;
        match self.category {
            RequestRejected => (
                "request",
                "request",
                "核对v2完整字段、文件摘要与原请求；不重放",
            ),
            ToolUidMismatch => (
                "tool-binding",
                "native_tool.uid",
                "核对明确WSL用户及实际UID",
            ),
            ToolImageMismatch => (
                "tool-binding",
                "native_tool.sha256",
                "核对双端实际ELF路径和摘要",
            ),
            ToolSourceMismatch => (
                "tool-binding",
                "native_tool.source_sha256",
                "核对独立clean来源、编译根与完整清单",
            ),
            ConfigurationDrift => (
                "configuration",
                "configuration",
                "核对原配置字节和完整目录身份；保留现场",
            ),
            IdentityMismatch => (
                "identity",
                "linux_identity",
                "核对原boot、创建时间、映像与PID复用；禁止向未知身份发信号",
            ),
            ToolConfigurationRejected => (
                "tool-configuration",
                "verification.linux_observer",
                "核对工具配置文件、完整来源和实际ELF；禁止回退解释器",
            ),
            ParentAuthorizationRejected => (
                "exec-authorization",
                "linux-intent.supervisor",
                "从正式stdin授权监督器启动；核对父身份和完整参数",
            ),
            ExecFailed => (
                "exec",
                "launcher",
                "核对绑定Redis入口、文件权限及配置；检查部分启动收据",
            ),
            PidfdUnsupported => (
                "pidfd-open",
                "pidfd",
                "核对Linux内核与pidfd支持；禁止裸PID回退",
            ),
            PidfdPermissionDenied => (
                "pidfd-open",
                "pidfd",
                "核对UID和内核安全策略；禁止裸PID回退",
            ),
            PidfdOpenFailed => ("pidfd-open", "pidfd", "核对自有子进程、资源限制及失败收据"),
            SignalFailed => (
                "pidfd-signal",
                "linux_identity",
                "核对已登记身份和权限；保留unknown，禁止重发",
            ),
            ExitWaitFailed => (
                "pidfd-wait",
                "linux_identity",
                "核对退出与端口证明；保留现场，禁止重放",
            ),
            ExecWaitTimeout => (
                "exec-wait",
                "timeout_seconds",
                "核对监督器日志、最终Redis映像与失败回收证明",
            ),
            AcknowledgementTimeout => (
                "acknowledgement",
                "acknowledged",
                "核对认证、INFO、CONFIG及本次确认收据",
            ),
            SupervisorInterrupted => (
                "supervision",
                "linux-intent.supervisor",
                "检查中断和精确清理收据；未确认时保持unknown",
            ),
        }
    }

    pub(super) fn value(&self) -> Value {
        let (phase, field, advice) = self.fields();
        let mut value = serde_json::to_value(self).expect("固定缓存诊断可序列化");
        value["phase"] = json!(phase);
        value["field"] = json!(field);
        value["advice"] = json!(advice);
        value
    }

    pub(super) fn decode(value: &Value) -> Result<Self> {
        super::super::json::exact(
            value,
            &[
                "category",
                "write_outcome",
                "os_code",
                "phase",
                "field",
                "advice",
            ],
            "缓存固定诊断",
        )?;
        let diagnostic: Self = serde_json::from_value(json!({"category":value["category"],
            "write_outcome":value["write_outcome"],"os_code":value["os_code"]}))
        .map_err(|_| "缓存远端诊断类别、写入状态或系统码无效")?;
        if diagnostic.value() != *value {
            return Err("缓存远端诊断字段不是固定值".into());
        }
        Ok(diagnostic)
    }
}
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (phase, field, advice) = self.fields();
        write!(
            f,
            "缓存阶段={phase} 类别={:?} 字段={field} 写入结果={:?} 系统码={:?}；{advice}",
            self.category, self.write_outcome, self.os_code
        )
    }
}
impl std::error::Error for Diagnostic {}

pub(super) fn failure(category: Category, outcome: Outcome) -> Box<dyn std::error::Error> {
    Box::new(Diagnostic {
        category,
        write_outcome: outcome,
        os_code: None,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn operating_system(
    category: Category,
    error: rustix::io::Errno,
) -> Box<dyn std::error::Error> {
    Box::new(Diagnostic {
        category,
        write_outcome: Outcome::Unknown,
        os_code: Some(error.raw_os_error()),
    })
}

pub(super) fn from_error(error: &(dyn std::error::Error + 'static)) -> Value {
    error.downcast_ref::<Diagnostic>().map_or_else(
        || {
            Diagnostic {
                category: Category::RequestRejected,
                write_outcome: Outcome::Unknown,
                os_code: None,
            }
            .value()
        },
        Diagnostic::value,
    )
}

pub(crate) fn primitive_detail(error: &(dyn std::error::Error + 'static)) -> (Value, Value) {
    if let Some(remote) = error.downcast_ref::<super::process::RemoteFailure>() {
        return (remote.diagnostic.value(), json!(remote.code));
    }
    if let Some(detail) = error.downcast_ref::<Diagnostic>() {
        return (detail.value(), Value::Null);
    }
    let code = error
        .downcast_ref::<super::process::ControlFailure>()
        .and_then(|failure| failure.0);
    (Value::Null, json!(code))
}

pub(crate) fn primitive_error(detail: &Value, code: &Value) -> Result<Box<dyn std::error::Error>> {
    let code = if code.is_null() {
        None
    } else {
        Some(
            code.as_i64()
                .and_then(|code| i32::try_from(code).ok())
                .filter(|code| *code != 0)
                .ok_or("缓存原语退出码无效")?,
        )
    };
    if detail.is_null() {
        return Ok(Box::new(super::process::ControlFailure(code)));
    }
    Ok(Box::new(super::process::RemoteFailure {
        code,
        diagnostic: Diagnostic::decode(detail)?,
    }))
}

pub(crate) fn primitive_message(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    error
        .downcast_ref::<super::process::RemoteFailure>()
        .map(ToString::to_string)
        .or_else(|| error.downcast_ref::<Diagnostic>().map(ToString::to_string))
        .or_else(|| {
            error
                .downcast_ref::<super::process::ControlFailure>()
                .map(ToString::to_string)
        })
}
