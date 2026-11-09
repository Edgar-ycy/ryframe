use std::path::PathBuf;

use serde_json::Value;

use super::super::{
    cache::handoff::locks::Storage,
    export::transport::{CommandOutput, CommandSpec, Transport},
    lifecycle::stage::StageRequest,
    source,
};
use super::{
    authority, fail, guard,
    input::{self, TargetContext},
    storage::StorageObserver,
    tree, verify_and_publish,
};
use crate::Result;

pub(crate) fn execute_context<T: Transport>(
    context: &TargetContext,
    request: &StageRequest<'_>,
    transport: &T,
    storage_observer: &impl StorageObserver,
    authority_observer: &impl authority::Observer,
) -> Result<Value> {
    if context.manifest["copy_stage"] != "seed_to_arm" {
        return verify_and_publish(
            context,
            request,
            transport,
            storage_observer,
            authority_observer,
            None,
        );
    }
    // 复制阶段已持外层运行锁。与交接保持同序，先持有外部存储，再进入目标 Generation。
    let storage = Storage::acquire(&context.backend, request.run_dir, &context.manifest)?;
    let result = verify_and_publish(
        context,
        request,
        transport,
        storage_observer,
        authority_observer,
        Some(&storage),
    );
    let published = result.is_ok();
    match storage.finish(result) {
        Err(error) if published => fail(context, error),
        result => result,
    }
}

pub(super) struct Frame<'a> {
    context: &'a TargetContext,
    inputs: guard::Inputs,
    generation: Option<guard::Generation>,
    authority: Value,
    source_root: PathBuf,
    source: Value,
}

impl<'a> Frame<'a> {
    pub(super) fn begin(context: &'a TargetContext, authority: Value) -> Result<Self> {
        let inputs = guard::Inputs::open(authority::protected_paths(context, &authority)?)?;
        let source_root = context.execution_backend()?;
        let source = source::current(&source_root)?;
        input::validate_inputs(&context.backend, &stage(context))?;
        let generation = guard::Generation::acquire(&context.backend, &context.target_dir)?;
        let mut frame = Self {
            context,
            inputs,
            generation: Some(generation),
            authority,
            source_root,
            source,
        };
        // 开始失败也要显式核对并释放本代次目录锁；未知锁保留给人工核对。
        if let Err(error) = frame.check() {
            let generation = frame.generation.take().ok_or("目标执行锁已经释放")?;
            return generation.finish::<Self>(Err(error));
        }
        Ok(frame)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.generation
            .as_ref()
            .ok_or("目标执行锁已经释放")?
            .check()?;
        self.check_inputs(true, true)?;
        input::validate_inputs(&self.context.backend, &stage(self.context))
    }

    pub(super) fn owner(&self) -> Result<&guard::Generation> {
        self.generation
            .as_ref()
            .ok_or_else(|| "目标执行锁已经释放".into())
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.generation
            .as_ref()
            .ok_or("目标执行锁已经释放")?
            .checkpoint()?;
        self.inputs.checkpoint()
    }

    fn check_inputs(&self, locked: bool, check_source: bool) -> Result<()> {
        self.inputs.check()?;
        if tree::snapshot(&self.context.backend, &self.context.target_dir, locked)?
            != self.authority["files"]
        {
            return Err("目标完整文件树在核验期间变化".into());
        }
        if check_source && source::current(&self.source_root)? != self.source {
            return Err("执行源码在核验期间变化".into());
        }
        Ok(())
    }

    pub(super) fn finish<T>(
        mut self,
        result: Result<T>,
        observer: &impl authority::Observer,
    ) -> Result<T> {
        let result = combine(result, self.check());
        let generation = self.generation.take().ok_or("目标执行锁已经释放")?;
        let result = generation.finish(result);
        let result = combine(result, self.check_inputs(false, true));
        match result {
            Ok(value) => {
                let observed =
                    authority::inspect(&self.context.backend, &stage(self.context), observer)?;
                if observed != self.authority {
                    return Err("目标锁释放后只读权威与初像不同".into());
                }
                self.check_inputs(false, true)?;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }
}

fn stage(context: &TargetContext) -> StageRequest<'_> {
    StageRequest {
        backend: &context.backend,
        run_dir: &context.run_dir,
        stage: "target-verify",
        mode: "run",
        attempt: None,
    }
}

pub(super) fn combine<T>(result: Result<T>, check: Result<()>) -> Result<T> {
    match (result, check) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(check)) => {
            Err(format!("{primary}；执行输入复核同时失败：{check}").into())
        }
    }
}

pub(super) struct CheckedTransport<'a, T> {
    pub(super) inner: &'a T,
    pub(super) frame: &'a Frame<'a>,
}

impl<T: Transport> Transport for CheckedTransport<'_, T> {
    fn s3(&self, request: super::super::s3::Request) -> Result<serde_json::Value> {
        self.frame.checkpoint()?;
        let result = self.inner.s3(request);
        combine(result, self.frame.checkpoint())
    }
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        self.frame.checkpoint()?;
        let result = self.inner.run(spec);
        combine(result, self.frame.checkpoint())
    }
}

pub(super) struct CheckedObserver<'a, O> {
    pub(super) inner: &'a O,
    pub(super) frame: &'a Frame<'a>,
}

impl<O: StorageObserver> StorageObserver for CheckedObserver<'_, O> {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        self.frame.checkpoint()?;
        combine(self.inner.process_identity(pid), self.frame.checkpoint())
    }

    fn arguments(
        &self,
        context: &super::storage::StorageInputs<'_>,
        identity: &Value,
    ) -> Result<Vec<String>> {
        self.frame.checkpoint()?;
        combine(
            self.inner.arguments(context, identity),
            self.frame.checkpoint(),
        )
    }

    fn listener(
        &self,
        context: &super::storage::StorageInputs<'_>,
        identity: &Value,
        url: &str,
    ) -> Result<()> {
        self.frame.checkpoint()?;
        combine(
            self.inner.listener(context, identity, url),
            self.frame.checkpoint(),
        )
    }

    #[cfg(test)]
    fn wsl(
        &self,
        context: &super::storage::StorageInputs<'_>,
        redis: &Value,
        args: &[String],
    ) -> Result<String> {
        self.frame.checkpoint()?;
        combine(
            self.inner.wsl(context, redis, args),
            self.frame.checkpoint(),
        )
    }

    fn redis(
        &self,
        context: &super::storage::StorageInputs<'_>,
        redis: &Value,
        parts: &[&str],
    ) -> Result<Value> {
        self.frame.checkpoint()?;
        combine(
            self.inner.redis(context, redis, parts),
            self.frame.checkpoint(),
        )
    }

    fn redis_window<T>(
        &self,
        context: &super::storage::StorageInputs<'_>,
        redis: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<T>,
    ) -> Result<T> {
        self.frame.checkpoint()?;
        let result = self.inner.redis_window(context, redis, |send| {
            read(&mut |parts| {
                self.frame.checkpoint()?;
                combine(send(parts), self.frame.checkpoint())
            })
        });
        combine(result, self.frame.checkpoint())
    }
}
