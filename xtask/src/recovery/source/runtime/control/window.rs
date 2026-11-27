//! 源停止与回收的文件配置和完整像区间；资源采集仍使用现有原生客户端。
use super::{existing, frame::Frame};
use crate::{
    Result,
    recovery::{
        clone::native::{
            export::{input::Context, transport::Transport},
            json as document,
            resources::common,
        },
        source::evidence::{environment, images, resources::Resources, running},
    },
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(windows)]
#[path = "window/held.rs"]
mod held;

pub(super) struct Window<'a, 'b, T> {
    pub frame: &'a Frame<'b>,
    pub facts: Value,
    pub environment: BTreeMap<String, String>,
    pub transport: &'a T,
    origin: Origin,
    original_environment: BTreeMap<String, String>,
}

enum Origin {
    Published(Value),
    Unpublished { start: Value, request: Value },
}

impl<'a, 'b, T: Transport + Sync> Window<'a, 'b, T> {
    pub(super) fn new(frame: &'a Frame<'b>, facts: Value, transport: &'a T) -> Result<Self> {
        let origin = Origin::Published(facts["start_descriptor"].clone());
        Self::with_origin(frame, facts, transport, origin)
    }

    pub(super) fn unpublished(
        frame: &'a Frame<'b>,
        start: &Value,
        request: &Value,
        transport: &'a T,
    ) -> Result<Self> {
        let facts = existing::unpublished(frame, start, request)?;
        Self::with_origin(
            frame,
            facts,
            transport,
            Origin::Unpublished {
                start: start.clone(),
                request: request.clone(),
            },
        )
    }

    fn with_origin(
        frame: &'a Frame<'b>,
        facts: Value,
        transport: &'a T,
        origin: Origin,
    ) -> Result<Self> {
        let private = serde_json::from_value(facts["environment"]["environment"].clone())
            .map_err(|_| "来源私有运行环境必须为字符串映射；检查已绑定环境文件，不回显字段值")?;
        let window = Self {
            frame,
            facts,
            environment: environment::configured(&private)?,
            transport,
            origin,
            original_environment: std::env::vars().collect(),
        };
        if window.facts["directory"] != serde_json::json!(frame.run) {
            return Err("源停止或回收的运行事实不属于当前持锁账本".into());
        }
        window.checkpoint()?;
        Ok(window)
    }

    pub(super) fn output(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(document::text(
            &self.facts["output"],
            "源实际代次",
        )?))
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.checkpoint_with(|| match &self.origin {
            Origin::Published(start) => running::load(self.frame.backend, start, false, true),
            Origin::Unpublished { start, request } => {
                existing::unpublished(self.frame, start, request)
            }
        })
    }

    fn checkpoint_with(&self, observe: impl FnOnce() -> Result<Value>) -> Result<()> {
        self.frame.checkpoint()?;
        if std::env::vars().collect::<BTreeMap<_, _>>() != self.original_environment {
            return Err("源停止或恢复的控制环境发生变化，禁止继续操作或发布".into());
        }
        let actual = observe()?;
        if actual != self.facts {
            return Err("源停止或恢复期间原 START、完整历史、文件配置或已登记资源变化".into());
        }
        if serde_json::to_value(
            crate::recovery::clone::native::resources::storage::registered::current(
                self.frame.backend,
                self.frame.run,
                crate::cli::CloneSide::Target,
            )?,
        )? != self.facts["request"]["current_storage"]
        {
            return Err("源控制区间的实际当前存储代次与冻结请求不同；保留现场，禁止重放".into());
        }
        Ok(())
    }

    pub(super) fn capture(&self, phase: &str) -> Result<(Value, Value)> {
        if !matches!(
            phase,
            "stop-before" | "after" | "recover-before" | "recover-after"
        ) {
            return Err("源控制完整像阶段无效".into());
        }
        #[cfg(windows)]
        if let Origin::Published(start) = &self.origin
            && self.frame.mode == "source-generation-stop"
            && matches!(phase, "stop-before" | "after")
            && crate::recovery::source::evidence::resources::warm_redis::applies(
                &self.facts["request"],
            )
        {
            return held::capture(self, start, phase);
        }
        let output = self.output()?.join(phase);
        let checkpoint = || self.checkpoint();
        let result = self.capture_with(&output, &checkpoint)?;
        self.checkpoint()?;
        Ok(result)
    }

    fn capture_with(
        &self,
        output: &Path,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<(Value, Value)> {
        let resources = self.resources(output, checkpoint)?;
        let context = images::capture::context(&resources, &self.facts)?;
        let descriptor = images::capture::run(
            &resources,
            &context,
            &self.facts["source"],
            &self.frame.scope,
        )?;
        let value = images::verify(
            self.frame.backend,
            &descriptor,
            &self.facts["selected"],
            &self.facts["source"]["request"],
            &self.facts["request"]["source_registration"],
        )?;
        Ok((descriptor, value))
    }

    pub(super) fn context(&self, output: &Path) -> Result<Context> {
        let checkpoint = || self.checkpoint();
        images::capture::context(&self.resources(output, &checkpoint)?, &self.facts)
    }

    fn resources<'s>(
        &'s self,
        output: &'s Path,
        checkpoint: &'s dyn Fn() -> Result<()>,
    ) -> Result<Resources<'s, T>> {
        common::bound_file(
            self.frame.backend,
            &self.facts["request"]["source_environment"],
            "源私有运行环境",
        )?;
        Ok(Resources {
            backend: self.frame.backend,
            execution: Path::new(document::text(&self.facts["execution"], "源执行后端")?),
            request: &self.facts["source"]["seed_target"],
            generation: &self.facts["request"],
            environment: &self.environment,
            output,
            transport: self.transport,
            checkpoint,
        })
    }
}
