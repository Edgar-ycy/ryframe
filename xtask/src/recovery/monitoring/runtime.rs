use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde_json::Value;

use crate::Result;

use super::{config, supervisor};

pub(crate) type Paths = BTreeMap<String, PathBuf>;
pub(crate) type Environment = BTreeMap<String, String>;
pub(crate) type Commands = BTreeMap<&'static str, Vec<String>>;

/// 显式注入外部执行边界；产品实现始终验证工具配置、就绪端点和实际监听进程。
pub(crate) trait Runtime {
    fn validate(&self, binding: &Value, paths: &Paths, environment: &Environment) -> Result<Value> {
        config::validate_configs(binding, paths, environment)
    }

    fn commands(&self, binding: &Value, paths: &Paths) -> Result<Commands> {
        config::role_commands(binding, paths)
    }

    fn ready(&self, tree: &Value, url: &str) -> Result<()> {
        config::wait_ready(url, Duration::from_secs(30))?;
        config::require_listening(
            tree["process"]["pid"].as_u64().ok_or("监控进程 PID 无效")?,
            url,
        )
    }

    fn invocation(&self, protocol: &supervisor::Protocol) -> Result<supervisor::Invocation> {
        supervisor::supervisor_invocation(protocol)
    }
}

pub(crate) struct LocalRuntime;

impl Runtime for LocalRuntime {}
