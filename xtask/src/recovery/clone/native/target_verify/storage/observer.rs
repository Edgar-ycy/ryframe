use serde_json::Value;

use super::{TargetContext, Transport, common, json_io, kernel};
use crate::Result;

/// 文件证据仍由核验器读取；端口只提供固定目标的外部进程与协议事实。
pub(crate) trait StorageObserver {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>>;
    fn arguments(&self, context: &TargetContext, identity: &Value) -> Result<Vec<String>>;
    fn listener(&self, context: &TargetContext, identity: &Value, url: &str) -> Result<()>;
    fn wsl(&self, context: &TargetContext, redis: &Value, args: &[String]) -> Result<String>;
    fn redis(&self, context: &TargetContext, redis: &Value, parts: &[&str]) -> Result<Value>;
}

pub(crate) struct SystemObserver<'a, T: Transport> {
    transport: &'a T,
}

impl<'a, T: Transport> SystemObserver<'a, T> {
    pub(crate) fn new(transport: &'a T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> StorageObserver for SystemObserver<'_, T> {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        common::process_identity(pid)
    }

    fn arguments(&self, context: &TargetContext, identity: &Value) -> Result<Vec<String>> {
        kernel::argv(context, identity, self.transport)
    }

    fn listener(&self, context: &TargetContext, identity: &Value, url: &str) -> Result<()> {
        kernel::listener(context, self.transport, identity, url)
    }

    fn wsl(&self, context: &TargetContext, redis: &Value, args: &[String]) -> Result<String> {
        let mut arguments = vec![
            "--distribution".into(),
            json_io::text(&redis["distribution"], "Redis distribution")?.into(),
            "--exec".into(),
        ];
        arguments.extend_from_slice(args);
        let program = json_io::text(&redis["wsl"]["path"], "Redis WSL 工具")?.into();
        kernel::observe(
            context,
            self.transport,
            program,
            arguments,
            "target-redis-kernel",
        )
        .map(|output| output.trim().to_owned())
    }

    fn redis(&self, context: &TargetContext, redis: &Value, parts: &[&str]) -> Result<Value> {
        let mut client = super::super::redis::Client::authenticated(context, redis)?;
        client.command(parts)
    }
}
