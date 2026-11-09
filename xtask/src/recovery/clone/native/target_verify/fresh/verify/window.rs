//! 两轮完整库存和当前对象、缓存初像必须与已发布初始化一致。
use super::super::super::super::{export::transport::Transport, resources::common};
use super::super::super::{
    database,
    guard::Generation,
    published, redis,
    storage::{self, StorageInputs, StorageObserver},
    tree,
};
use super::super::{failure, initialized_objects, selection::Selected};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct Window<'a, T, O, F> {
    pub(crate) inputs: StorageInputs<'a>,
    pub(crate) target: &'a Path,
    pub(crate) descriptor: &'a Value,
    pub(crate) initialized: &'a Value,
    pub(crate) owner: &'a Generation,
    pub(crate) transport: &'a T,
    pub(crate) observer: &'a O,
    pub(crate) generation: &'a F,
    pub(crate) temporary: &'a Value,
}

impl<T: Transport, O: StorageObserver, F: Fn() -> Result<(Value, Selected, Option<Value>)>>
    Window<'_, T, O, F>
{
    pub(crate) fn observe(&self) -> Result<(Value, super::super::super::guard::Inputs)> {
        let inputs = &self.inputs;
        common::directory_identity(inputs.backend, self.temporary)?;
        let before = tree::snapshot(inputs.backend, self.target, true)?;
        let protected = super::inputs::protect(inputs, self.target, &before)?;
        let original = (self.generation)()?;
        storage::generation::validate_complete(
            &self.initialized["generation"],
            &original.0,
            original.1.runtime.as_ref(),
            original.2.as_ref(),
        )?;
        let checkpoint = || {
            common::directory_identity(inputs.backend, self.temporary)?;
            self.owner.checkpoint()?;
            protected.checkpoint()
        };
        let captured = failure::capture(database::capture_at(
            inputs,
            &inputs.observation_dir.join("inventory"),
            self.transport,
            &checkpoint,
        ))?;
        let objects = initialized_objects::observe(inputs, self.transport)?;
        let cache = redis::state_with_observer(
            inputs,
            &original.1.value["redis"],
            true,
            true,
            self.observer,
        )?;
        if serde_json::to_value(&captured.observations)?
            != self.initialized["inventory"]["observations"]
            || objects != self.initialized["objects"]
            || cache != self.initialized["redis"]
        {
            return Err("目标当前完整前像与初始化像不同".into());
        }
        self.unchanged(&original, &before, &protected)?;
        Ok((
            json!({"status":"fresh_target_reverified","initialized":self.descriptor,
            "inventory":{"receipt":captured.receipt,"observations":captured.observations},
            "remote_writes":0,"target_ready":false,"clone_verified":false,"restore_qualified":false}),
            protected,
        ))
    }

    fn unchanged(
        &self,
        original: &(Value, Selected, Option<Value>),
        before: &Value,
        protected: &super::super::super::guard::Inputs,
    ) -> Result<()> {
        let inputs = &self.inputs;
        let actual = (self.generation)()?;
        if actual.0 != original.0
            || actual.1.value != original.1.value
            || actual.1.runtime != original.1.runtime
            || actual.2 != original.2
        {
            return Err("目标源码、配置、凭据、维护产物或存储进程代次变化".into());
        }
        let history = published::evidence_history(
            inputs.backend,
            &self.target.join("initialized.json"),
            Some(self.owner),
        )?;
        if history.0 != *self.initialized
            || history.1 != *inputs.request
            || common::binding(&self.target.join("initialized.json"))? != *self.descriptor
            || tree::snapshot(inputs.backend, self.target, true)? != *before
        {
            return Err("目标完整初始化前像在只读复验期间变化".into());
        }
        protected.check()?;
        common::directory_identity(inputs.backend, self.temporary)?;
        self.owner.check()?;
        Ok(())
    }
}
