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
        &'a dyn Fn() -> Result<(Value, selection::Selected, Option<Value>)>,
    pub(crate) observer: &'a dyn StorageObserver,
    pub(crate) owner: &'a Generation,
    pub(crate) protected: &'a Inputs,
}
impl<T: Transport> Window<'_, T> {
    pub(super) fn checkpoint(&self) -> Result<()> {
        self.owner.checkpoint()?;
        self.protected.checkpoint()
    }
    pub(super) fn generation(&self) -> Result<(Value, selection::Selected, Option<Value>)> {
        self.checkpoint()?;
        let result = (self.observe_generation)();
        super::super::registration::combine(result, self.checkpoint())
    }
    pub(super) fn unchanged(&self, expected: &Value) -> Result<()> {
        if self.generation()?.0 != *expected {
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
    pub(super) fn mysql(&self, db: &Value, sql: &str) -> Result<String> {
        self.checkpoint()?;
        let result = super::super::resources::mysql(&self.inputs, self.transport, db, sql);
        super::super::registration::combine(result, self.checkpoint())
    }
    pub(super) fn redis(&self, parts: &[&str]) -> Result<Value> {
        self.checkpoint()?;
        let result = self.observer.redis(
            &self.inputs,
            &self.inputs.request["storage"]["redis"],
            parts,
        );
        super::super::registration::combine(result, self.checkpoint())
    }
    pub(super) fn redis_state(&self, initialized: bool, sentinel: bool) -> Result<Value> {
        redis::state(&self.inputs, initialized, sentinel, &mut |parts| {
            self.redis(parts)
        })
    }
    pub(super) fn objects(&self, initialized: bool) -> Result<Value> {
        self.checkpoint()?;
        let result = if initialized {
            super::super::initialized_objects::observe(&self.inputs, self.transport)
        } else {
            super::super::objects::absence(&self.inputs, self.transport)
        };
        super::super::registration::combine(result, self.checkpoint())
    }
}
