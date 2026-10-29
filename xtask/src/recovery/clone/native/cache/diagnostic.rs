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
    WslEntryRejected,
    ParentAuthorizationRejected,
    ExecFailed,
    PidfdUnsupported,
    PidfdPermissionDenied,
    PidfdOpenFailed,
    SignalFailed,
    ExitWaitFailed,
    ExecWaitTimeout,
    ReadinessTimeout,
    ProcessReceiptTimeout,
    AcceptanceTimeout,
    AcknowledgementTimeout,
    SupervisorInterrupted,
    PrelaunchGuardRejected,
    PrelaunchSocketFailed,
    PrelaunchSocketOptionsFailed,
    PrelaunchPortBindFailed,
    PrelaunchPortReleaseTimeout,
    PrelaunchCommandRejected,
    PrelaunchIntentFailed,
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
            PrelaunchGuardRejected
            | PrelaunchSocketFailed
            | PrelaunchSocketOptionsFailed
            | PrelaunchPortBindFailed
            | PrelaunchPortReleaseTimeout
            | PrelaunchCommandRejected
            | PrelaunchIntentFailed => prelaunch_fields(self.category),
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
            WslEntryRejected => (
                "tool-configuration",
                "PATH.wsl",
                "核对标准OS PATH的绝对目录顺序及首个WSL入口，必须是无链接普通文件",
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
            ExecWaitTimeout
            | ReadinessTimeout
            | ProcessReceiptTimeout
            | AcceptanceTimeout
            | AcknowledgementTimeout => timeout_fields(self.category),
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
fn timeout_fields(category: Category) -> (&'static str, &'static str, &'static str) {
    use Category::*;
    match category {
        ReadinessTimeout => (
            "readiness",
            "timeout_seconds",
            "核对认证就绪、完整观察及guard是否在原就绪预算内完成；保留现场，禁止重放",
        ),
        ExecWaitTimeout => (
            "exec-wait",
            "timeout_seconds",
            "核对监督器日志、最终Redis映像与失败回收证明",
        ),
        ProcessReceiptTimeout => (
            "process-receipt",
            "linux-process.json",
            "核对启动、创建身份与失败清理证明；保留现场，禁止重放",
        ),
        AcceptanceTimeout => (
            "acceptance",
            "accepted.json",
            "核对 acknowledged.json、accepted.json 与接受确认15秒期限；保留现场，禁止重放",
        ),
        AcknowledgementTimeout => (
            "acknowledgement",
            "acknowledged",
            "核对认证、INFO、CONFIG及本次确认收据",
        ),
        _ => unreachable!("超时字段映射仅接收超时诊断"),
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
    if let Some(failure) = error.downcast_ref::<PrelaunchFailure>() {
        return (failure.value(), Value::Null);
    }
    if let Some(failure) = error.downcast_ref::<super::cleanup_diagnostic::Failure>() {
        return primitive_detail(failure.primary.as_ref());
    }
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
    if detail.get("cause").is_some()
        || detail.get("cause_sha256").is_some()
        || detail["category"]
            .as_str()
            .is_some_and(|name| name.starts_with("prelaunch_"))
    {
        if code.is_some() {
            return Err("缓存prelaunch失败不得携带尚未创建的子进程退出码".into());
        }
        return Ok(Box::new(PrelaunchFailure::decode(detail)?));
    }
    if detail.is_null() {
        return Ok(Box::new(super::process::ControlFailure(code)));
    }
    Ok(Box::new(super::process::RemoteFailure {
        code,
        diagnostic: Diagnostic::decode(detail)?,
    }))
}

pub(crate) fn primitive_message(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    if let Some(failure) = error.downcast_ref::<PrelaunchFailure>() {
        return Some(failure.to_string());
    }
    if let Some(failure) = error.downcast_ref::<super::cleanup_diagnostic::Failure>() {
        return Some(failure.to_string());
    }
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PrelaunchCause {
    PermissionDenied,
    AddressInUse,
    AddressNotAvailable,
    NotFound,
    AlreadyExists,
    TimedOut,
    OtherIo,
    SourcesChanged,
    DocumentsChanged,
    StateChanged,
    EnvironmentChanged,
    ForeignBackend,
    Rejected,
}

struct PrelaunchFailure {
    diagnostic: Diagnostic,
    cause: PrelaunchCause,
    sha256: String,
}
impl PrelaunchFailure {
    fn value(&self) -> Value {
        let mut value = self.diagnostic.value();
        value["cause"] = json!(self.cause);
        value["cause_sha256"] = json!(self.sha256);
        value
    }
    fn decode(value: &Value) -> Result<Self> {
        let mut base = value.clone();
        let fields = base.as_object_mut().ok_or("缓存prelaunch诊断必须为对象")?;
        let cause = fields.remove("cause").ok_or("缓存prelaunch缺少固定原因")?;
        let sha256 = fields
            .remove("cause_sha256")
            .ok_or("缓存prelaunch缺少原因摘要")?;
        let diagnostic = Diagnostic::decode(&base)?;
        if !prelaunch_category(diagnostic.category)
            || !matches!(diagnostic.write_outcome, Outcome::NotStarted)
        {
            return Err("缓存prelaunch阶段或启动结果无效".into());
        }
        let cause: PrelaunchCause =
            serde_json::from_value(cause).map_err(|_| "缓存prelaunch原因不是固定安全类别")?;
        let sha256 = sha256
            .as_str()
            .filter(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or("缓存prelaunch原因摘要无效")?
            .to_owned();
        let io = matches!(
            cause,
            PrelaunchCause::PermissionDenied
                | PrelaunchCause::AddressInUse
                | PrelaunchCause::AddressNotAvailable
                | PrelaunchCause::NotFound
                | PrelaunchCause::AlreadyExists
                | PrelaunchCause::TimedOut
                | PrelaunchCause::OtherIo
        );
        let guard = matches!(
            cause,
            PrelaunchCause::SourcesChanged
                | PrelaunchCause::DocumentsChanged
                | PrelaunchCause::StateChanged
                | PrelaunchCause::EnvironmentChanged
                | PrelaunchCause::ForeignBackend
        );
        let socket = matches!(
            diagnostic.category,
            Category::PrelaunchSocketFailed
                | Category::PrelaunchSocketOptionsFailed
                | Category::PrelaunchPortBindFailed
                | Category::PrelaunchPortReleaseTimeout
        );
        if matches!(diagnostic.category, Category::PrelaunchPortReleaseTimeout)
            && !(matches!(cause, PrelaunchCause::AddressInUse)
                || (matches!(cause, PrelaunchCause::TimedOut) && diagnostic.os_code.is_none()))
        {
            return Err("缓存端口释放超时原因无效".into());
        }
        if ((diagnostic.os_code.is_some() || socket) && !io)
            || (guard && !matches!(diagnostic.category, Category::PrelaunchGuardRejected))
        {
            return Err("缓存prelaunch系统码与原因不一致".into());
        }
        Ok(Self {
            diagnostic,
            cause,
            sha256,
        })
    }
}
impl std::fmt::Display for PrelaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}；固定原因={:?} 原因摘要={}",
            self.diagnostic, self.cause, self.sha256
        )
    }
}
impl std::fmt::Debug for PrelaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for PrelaunchFailure {}

fn prelaunch_category(category: Category) -> bool {
    matches!(
        category,
        Category::PrelaunchGuardRejected
            | Category::PrelaunchSocketFailed
            | Category::PrelaunchSocketOptionsFailed
            | Category::PrelaunchPortBindFailed
            | Category::PrelaunchPortReleaseTimeout
            | Category::PrelaunchCommandRejected
            | Category::PrelaunchIntentFailed
    )
}

fn prelaunch_fields(category: Category) -> (&'static str, &'static str, &'static str) {
    use Category::*;
    match category {
        PrelaunchGuardRejected => (
            "prelaunch-guard",
            "sources",
            "核对原来源、运行账本和私有环境绑定；保留现场，不重放",
        ),
        PrelaunchSocketFailed => (
            "prelaunch-socket",
            "port",
            "核对本机套接字资源和系统权限；不重放",
        ),
        PrelaunchSocketOptionsFailed => (
            "prelaunch-socket-options",
            "port",
            "核对本机套接字独占选项和系统权限；不重放",
        ),
        PrelaunchPortBindFailed => (
            "prelaunch-bind",
            "port",
            "核对当次绑定端口的独占性与系统权限；当前端口空闲不能证明原失败原因",
        ),
        PrelaunchPortReleaseTimeout => (
            "prelaunch-port-release",
            "port",
            "启动前端口释放等待已达到固定60秒期限；核对Windows独占端口与WSL mirrored映射释放，保留现场并显式核对，不自动重启或重放",
        ),
        PrelaunchCommandRejected => (
            "prelaunch-command",
            "native_tool",
            "核对绑定工具与完整启动参数；保留现场，不重放",
        ),
        PrelaunchIntentFailed => (
            "prelaunch-intent",
            "intent.json",
            "核对原输出目录、权限和已存在证据；不覆盖，不重放",
        ),
        _ => unreachable!("仅prelaunch类别调用"),
    }
}

pub(super) fn prelaunch(
    category: Category,
    error: Box<dyn std::error::Error>,
) -> Box<dyn std::error::Error> {
    use sha2::{Digest, Sha256};
    use std::io::ErrorKind;
    let text = error.to_string();
    let io = error.downcast_ref::<std::io::Error>();
    let cause = match io.map(std::io::Error::kind) {
        Some(ErrorKind::PermissionDenied) => PrelaunchCause::PermissionDenied,
        Some(ErrorKind::AddrInUse) => PrelaunchCause::AddressInUse,
        Some(ErrorKind::AddrNotAvailable) => PrelaunchCause::AddressNotAvailable,
        Some(ErrorKind::NotFound) => PrelaunchCause::NotFound,
        Some(ErrorKind::AlreadyExists) => PrelaunchCause::AlreadyExists,
        Some(ErrorKind::TimedOut) => PrelaunchCause::TimedOut,
        Some(_) => PrelaunchCause::OtherIo,
        None => match text.as_str() {
            "生命周期来源在执行期间发生变化" => PrelaunchCause::SourcesChanged,
            "生命周期输入内容与采样摘要不一致" => PrelaunchCause::DocumentsChanged,
            "生命周期账本在采样期间变化" => PrelaunchCause::StateChanged,
            "生命周期私有环境在执行期间变化" => PrelaunchCause::EnvironmentChanged,
            "服务上下文属于其他后端" => PrelaunchCause::ForeignBackend,
            _ => PrelaunchCause::Rejected,
        },
    };
    Box::new(PrelaunchFailure {
        diagnostic: Diagnostic {
            category,
            write_outcome: Outcome::NotStarted,
            os_code: io.and_then(std::io::Error::raw_os_error),
        },
        cause,
        sha256: hex::encode(Sha256::digest(text.as_bytes())),
    })
}
