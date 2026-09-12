use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrontendSourceEvent {
    PullRequest,
    Push,
    Schedule,
    WorkflowDispatch,
}

impl FrontendSourceEvent {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PullRequest => "pull_request",
            Self::Push => "push",
            Self::Schedule => "schedule",
            Self::WorkflowDispatch => "workflow_dispatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontendSourceOptions {
    pub(crate) event_name: FrontendSourceEvent,
    pub(crate) event: Option<PathBuf>,
    pub(crate) base_sha: Option<String>,
    pub(crate) prefer_marker: bool,
    pub(crate) candidate_openapi: Option<PathBuf>,
    pub(crate) release_ref: Option<String>,
    pub(crate) fallback_main_on_invalid_base: bool,
}

pub(crate) fn valid_frontend_source_sha(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn valid_frontend_source_ref(value: &str) -> bool {
    value.starts_with("refs/")
        && !value.ends_with(['/', '.'])
        && !value.contains("//")
        && !value.contains("..")
        && !value.contains("@{")
        && !value.split('/').any(|component| {
            component.is_empty()
                || component.starts_with('.')
                || component.ends_with(".lock")
                || component == "@"
        })
        && value.bytes().all(|byte| {
            !byte.is_ascii_control()
                && !byte.is_ascii_whitespace()
                && !matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}
