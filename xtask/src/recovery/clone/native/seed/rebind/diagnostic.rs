//! 固定职责分类供既有阶段账本和 CLI 消费；原始错误不进入公开诊断。
use std::{error::Error, fmt};

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Preflight,
    Authority,
    Source,
    History,
    Cache,
    Quiet,
    Storage,
    Transition,
    Comparison,
    Checkpoint,
}

impl Phase {
    fn details(self) -> (&'static str, &'static str) {
        match self {
            Self::Preflight => (
                "SourceRebindPreflightRejected",
                "预检：核对显式 successor 请求、运行目录和 source_to_seed 清单",
            ),
            Self::Authority => (
                "SourceRebindAuthorityRejected",
                "控制授权：核对原 RunLock owner、同进程身份、最后运行 attempt 和控制器绑定",
            ),
            Self::Source => (
                "SourceRebindSourceRejected",
                "来源恢复：核对 successor、来源登记、初始化请求及完整绑定文件",
            ),
            Self::History => (
                "SourceRebindHistoryRejected",
                "完整历史：核对相邻阶段与全部缓存、封存、零输出和导出协调证明",
            ),
            Self::Cache => (
                "SourceRebindCacheRejected",
                "缓存后继：核对最新缓存创建证明与同一 successor 请求",
            ),
            Self::Quiet => (
                "SourceRebindQuietRejected",
                "生产者静默：核对完整谱系身份已退出及 API、Worker 双端口关闭证明",
            ),
            Self::Storage => (
                "SourceRebindStorageRejected",
                "当前存储：核对最新已发布代次、创建身份、启动及 ready 收据",
            ),
            Self::Transition => (
                "SourceRebindTransitionRejected",
                "存储换代：核对原进程已退出、固定请求、目录身份、端点与同二进制后继",
            ),
            Self::Comparison => (
                "SourceRebindObservationChanged",
                "双次观察：核对来源、当前存储及完整历史前后像漂移",
            ),
            Self::Checkpoint => (
                "SourceRebindCheckpointChanged",
                "后置核验：核对完整账本、清单、锁目录、owner、请求和固定原生工具来源",
            ),
        }
    }
}

pub(super) struct Failure {
    phase: Phase,
    primary: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (category, guidance) = self.phase.details();
        write!(f, "来源重绑定 {category}；{guidance}")?;
        if let Some(cause) = safe_cim_cause(self.primary.as_ref()) {
            write!(f, "；{cause}")?;
        }
        if self.secondary.is_some() {
            write!(f, "；后置核验同时失败，须复核当前控制器及完整前后像")?;
        }
        write!(
            f,
            "；保留原始来源、阶段、收据与失败现场，禁止自动重放或抢锁"
        )
    }
}

fn safe_cim_cause(error: &(dyn Error + 'static)) -> Option<String> {
    if let Some(error) =
        error.downcast_ref::<super::super::source_registration::quiet::CimUnavailable>()
    {
        return Some(error.to_string());
    }
    error.source().and_then(safe_cim_cause)
}

impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

pub(super) fn failure(
    phase: Phase,
    primary: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
) -> Box<dyn Error> {
    if secondary.is_none() && primary.is::<Failure>() {
        return primary;
    }
    let phase = primary
        .downcast_ref::<Failure>()
        .map(|error| error.phase)
        .unwrap_or(phase);
    Box::new(Failure {
        phase,
        primary,
        secondary,
    })
}

pub(crate) fn diagnostic(error: &(dyn Error + 'static)) -> Option<(String, String)> {
    if let Some(error) = error.downcast_ref::<Failure>() {
        return Some((error.phase.details().0.to_owned(), error.to_string()));
    }
    error.source().and_then(diagnostic)
}
