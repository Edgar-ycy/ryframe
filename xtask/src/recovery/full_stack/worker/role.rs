//! 监督角色只由固定白名单选择，文件名不能由私有请求自由指定。
use crate::Result;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Role {
    Api,
    Worker,
    Frontend,
}

impl Role {
    pub(super) fn parse(value: &Value) -> Result<Self> {
        match value.as_str() {
            Some("api") => Ok(Self::Api),
            Some("worker") => Ok(Self::Worker),
            Some("frontend") => Ok(Self::Frontend),
            _ => Err("监督角色必须为明确的 API、Worker 或 frontend".into()),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
            Self::Frontend => "frontend",
        }
    }

    pub(in crate::recovery) fn label(self) -> &'static str {
        match self {
            Self::Api => "API",
            Self::Worker => "Worker",
            Self::Frontend => "前端命令",
        }
    }

    pub(super) fn owner(value: &Value) -> Result<Self> {
        match value["kind"].as_str() {
            Some("full-stack-api-control-owner") => Ok(Self::Api),
            Some("full-stack-worker-control-owner") => Ok(Self::Worker),
            Some("full-stack-frontend-control-owner") => Ok(Self::Frontend),
            _ => Err("监督 owner 未绑定受支持的角色".into()),
        }
    }

    pub(super) fn request(value: &Value) -> Result<Self> {
        match value["kind"].as_str() {
            Some("full-stack-api-control-request") => Ok(Self::Api),
            Some("full-stack-worker-control-request") => Ok(Self::Worker),
            Some("full-stack-frontend-control-request") => Ok(Self::Frontend),
            _ => Err("监督 request 未绑定受支持的角色".into()),
        }
    }

    pub(super) fn kind(self, suffix: &str) -> String {
        format!("full-stack-{}-control-{suffix}", self.name())
    }

    pub(super) fn failure(self) -> &'static str {
        match self {
            Self::Api => "ApiControlFailed",
            Self::Worker => "WorkerControlFailed",
            Self::Frontend => "FrontendCommandFailed",
        }
    }

    pub(super) fn lock(self) -> String {
        format!("{}-control.lock", self.name())
    }

    pub(super) fn archive(self) -> String {
        format!("{}-control-receipts", self.name())
    }

    pub(super) fn artifact(self) -> &'static str {
        match self {
            Self::Api => "api_artifact",
            Self::Worker => "worker_artifact",
            Self::Frontend => "frontend_artifact",
        }
    }

    pub(super) fn ready(self) -> &'static str {
        match self {
            Self::Api => "api_ready_url",
            Self::Worker => "worker_ready_url",
            Self::Frontend => "frontend_completion",
        }
    }

    pub(super) fn config(self) -> Result<&'static str> {
        match self {
            Self::Api => Ok("api_config_dir"),
            Self::Worker => Ok("worker_config_dir"),
            Self::Frontend => Err("前端有限命令不使用产品配置目录".into()),
        }
    }
}
