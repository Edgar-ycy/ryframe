//! 关闭阶段仅公开固定分类、系统码与已知执行事实，不输出原始 cause。
use super::*;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::recovery::clone::native::cache) enum Stage {
    Inputs,
    Stop,
    Exit,
    Launcher,
    Port,
    PortReleaseTimeout,
    Guard,
    Publish,
    PartialPublish,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Failure {
    kind: String,
    stage: Stage,
    stop_requested: bool,
    exit_confirmed: bool,
    cause: String,
    os_code: Option<i32>,
    exit_code: Option<i32>,
}
impl Failure {
    pub(super) fn exit_code(&self) -> Value {
        json!(self.exit_code)
    }
    pub(super) fn value(&self) -> Value {
        serde_json::to_value(self).expect("固定关闭诊断")
    }
    pub(super) fn decode(value: &Value) -> Result<Self> {
        super::super::super::json::exact(
            value,
            &[
                "kind",
                "stage",
                "stop_requested",
                "exit_confirmed",
                "cause",
                "os_code",
                "exit_code",
            ],
            "关闭固定诊断",
        )?;
        let result: Self = serde_json::from_value(value.clone()).map_err(|_| "关闭诊断字段无效")?;
        if result.kind != "cache-stop-diagnostic"
            || ![
                "permission_denied",
                "address_in_use",
                "timed_out",
                "other_io",
                "rejected",
                "remote",
            ]
            .contains(&result.cause.as_str())
            || (matches!(result.stage, Stage::Inputs) && result.stop_requested)
            || (matches!(result.stage, Stage::PartialPublish)
                && (result.stop_requested || result.exit_confirmed))
            || (matches!(
                result.stage,
                Stage::Launcher | Stage::Port | Stage::PortReleaseTimeout | Stage::Publish
            ) && !result.exit_confirmed)
            || (result.cause == "rejected" && result.os_code.is_some())
            || (result.cause != "remote" && result.exit_code.is_some())
            || result.exit_code == Some(0)
        {
            return Err("关闭诊断阶段或执行事实无效".into());
        }
        Ok(result)
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Redis关闭阶段={:?} stop已调用={} 退出已确认={} 类别={} 系统码={:?} 原退出码={:?}；保留原失败和收据，核对原身份、完整前后像与端口，仅显式reconcile，不重发stop",
            self.stage,
            self.stop_requested,
            self.exit_confirmed,
            self.cause,
            self.os_code,
            self.exit_code
        )
    }
}
impl Error for Failure {}
pub(in crate::recovery::clone::native::cache) fn capture(
    stage: Stage,
    stop_requested: bool,
    exit_confirmed: bool,
    error: Box<dyn Error>,
) -> Box<dyn Error> {
    if error.is::<Failure>() {
        return error;
    }
    let mut current: &(dyn Error + 'static) = error.as_ref();
    let io = loop {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            break Some(io);
        }
        match current.source() {
            Some(source) => current = source,
            None => break None,
        }
    };
    let (remote, exit_code) = super::primitive_detail(error.as_ref());
    let cause = match io.map(std::io::Error::kind) {
        Some(std::io::ErrorKind::PermissionDenied) => "permission_denied",
        Some(std::io::ErrorKind::AddrInUse) => "address_in_use",
        Some(std::io::ErrorKind::TimedOut) => "timed_out",
        Some(_) => "other_io",
        None if !remote.is_null() || !exit_code.is_null() => "remote",
        None => "rejected",
    };
    Box::new(Failure {
        kind: "cache-stop-diagnostic".into(),
        stage,
        stop_requested,
        exit_confirmed,
        cause: cause.into(),
        os_code: io.and_then(std::io::Error::raw_os_error).or_else(|| {
            remote["os_code"]
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
        }),
        exit_code: exit_code.as_i64().and_then(|n| i32::try_from(n).ok()),
    })
}
