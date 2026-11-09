//! 初始化窗口只借用已核验固定输入，所有资源访问由真实Generation守卫包围。
use super::super::super::super::{export::transport::Transport, json as document};
use super::super::super::{
    guard::{Generation, Inputs},
    redis,
    storage::{StorageInputs, StorageObserver},
};
use super::super::{receipts, registration::Registration, selection};
use crate::Result;
use serde_json::Value;
use std::time::Duration;

pub(crate) struct Window<'a, T> {
    pub(crate) registration: &'a Registration,
    pub(crate) inputs: StorageInputs<'a>,
    pub(crate) transport: &'a T,
    pub(crate) observe_generation:
        &'a dyn Fn(bool) -> Result<(Value, selection::Selected, Option<Value>)>,
    pub(crate) observer: &'a dyn StorageObserver,
    pub(crate) owner: &'a Generation,
    pub(crate) protected: &'a Inputs,
}
impl<T: Transport> Window<'_, T> {
    pub(crate) fn checkpoint(&self) -> Result<()> {
        self.owner.checkpoint()?;
        self.protected.checkpoint()
    }
    pub(crate) fn generation(&self) -> Result<(Value, selection::Selected, Option<Value>)> {
        self.observe_generation(true)
    }
    fn observe_generation(
        &self,
        publish: bool,
    ) -> Result<(Value, selection::Selected, Option<Value>)> {
        self.checkpoint()?;
        let result = (self.observe_generation)(publish);
        super::super::registration::combine(result, self.checkpoint())
    }
    pub(crate) fn unchanged(&self, expected: &Value) -> Result<()> {
        self.require_unchanged(expected, true)
    }
    fn require_unchanged(&self, expected: &Value, publish: bool) -> Result<()> {
        if self.observe_generation(publish)?.0 != *expected {
            return Err("初始化来源、配置、维护工具或存储实际代次变化；保留现场，禁止重放".into());
        }
        Ok(())
    }
    pub(super) fn command(&self, label: &str, argv: &[Value], timeout: u64) -> Result<String> {
        let (program, arguments) = argv.split_first().ok_or("固定维护命令为空")?;
        let program = document::text(program, "维护产物")?.into();
        let args = arguments
            .iter()
            .map(|v| Ok(document::text(v, "固定维护参数")?.to_owned()))
            .collect::<Result<Vec<_>>>()?;
        self.checkpoint()?;
        let result = receipts::command_with_deadline(
            &self.inputs,
            self.transport,
            program,
            args,
            None,
            (label, Duration::from_secs(timeout)),
        );
        super::super::registration::combine(result, self.checkpoint())
    }
    pub(crate) fn mysql(&self, db: &Value, sql: &str) -> Result<String> {
        self.checkpoint()?;
        let result = super::super::resources::mysql(&self.inputs, self.transport, db, sql);
        super::super::registration::combine(result, self.checkpoint())
    }
    pub(crate) fn redis(&self, expected: &Value, parts: &[&str]) -> Result<Value> {
        self.redis_window(expected, |send| send(parts))
    }
    pub(crate) fn redis_state(
        &self,
        expected: &Value,
        initialized: bool,
        sentinel: bool,
    ) -> Result<Value> {
        self.redis_window(expected, |send| {
            redis::state(&self.inputs, initialized, sentinel, send)
        })
    }

    fn redis_window<R>(
        &self,
        expected: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<R>,
    ) -> Result<R> {
        let original = &self.inputs.request["storage"]["redis"];
        let selected = &expected["storage"]["redis"];
        let windows = original["platform"] == "windows";
        if windows != (selected["platform"] == "windows") {
            return Err("初始化 Redis 平台与固定请求不同".into());
        }
        if windows {
            #[cfg(windows)]
            return self.windows_redis_window(expected, selected, read);
            #[cfg(not(windows))]
            return Err("Windows Redis 初始化只能在本机 Windows 核验".into());
        }
        read(&mut |parts| {
            self.checkpoint()?;
            let result = self.observer.redis(&self.inputs, original, parts);
            super::super::registration::combine(result, self.checkpoint())
        })
    }

    #[cfg(windows)]
    fn windows_redis_window<R>(
        &self,
        expected: &Value,
        selected: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<R>,
    ) -> Result<R> {
        use super::super::registration::combine;

        // 连接窗口的前后检查不能向当前阶段追加未声明的完整观察轮次。
        self.require_unchanged(expected, false)?;
        let result = (|| {
            let mut client = redis::Client::authenticated(&self.inputs, selected)?;
            let result = read(&mut |parts| {
                self.checkpoint()?;
                combine(client.command(parts), self.checkpoint())
            });
            combine(result, client.recheck_windows_peer())
        })();
        combine(result, self.require_unchanged(expected, false))
    }
    pub(crate) fn objects(&self, initialized: bool) -> Result<Value> {
        self.checkpoint()?;
        let result = if initialized {
            super::super::initialized_objects::observe(&self.inputs, self.transport)
        } else {
            super::super::objects::absence(&self.inputs, self.transport)
        };
        super::super::registration::combine(result, self.checkpoint())
    }
}
