use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MonitoringCommand {
    Help,
    BindHelp,
    LifecycleHelp(MonitoringOperation),
    Bind(Box<MonitoringBindOptions>),
    Lifecycle {
        operation: MonitoringOperation,
        binding: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitoringOperation {
    Start,
    Observe,
    Close,
    Result,
    Status,
}

impl MonitoringOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Observe => "observe",
            Self::Close => "close",
            Self::Result => "result",
            Self::Status => "status",
        }
    }

    pub(crate) const fn writes(self) -> bool {
        !matches!(self, Self::Status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitoringBindOptions {
    pub(crate) runtime_receipt: PathBuf,
    pub(crate) target_plan: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) run_id: String,
    pub(crate) metrics_token_file: PathBuf,
    pub(crate) tools: MonitoringTools,
    pub(crate) ports: MonitoringPorts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitoringTools {
    pub(crate) prometheus: PathBuf,
    pub(crate) promtool: PathBuf,
    pub(crate) alertmanager: PathBuf,
    pub(crate) amtool: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MonitoringPorts {
    pub(crate) prometheus: u16,
    pub(crate) alertmanager: u16,
    pub(crate) webhook: u16,
}
