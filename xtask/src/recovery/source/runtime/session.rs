//! 来源父执行器的真实控制区间；历史事实和当前恢复工具分别绑定，退出不抢锁。
use super::{environment, images, producer, ready, resources::Resources, running};
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        export::transport::Transport,
        json as document,
        lifecycle::lock::{ProcessGuard, RunLock},
        resources::common,
        source,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) struct Session<'a, T> {
    pub backend: &'a Path,
    pub start: Value,
    pub output: PathBuf,
    pub directory: PathBuf,
    pub facts: Value,
    pub environment: BTreeMap<String, String>,
    pub tool: PathBuf,
    pub recover: bool,
    pub scope: Scope,
    pub transport: &'a T,
    original_environment: BTreeMap<String, String>,
    recovery_tools: Value,
    owner: RunLock,
    guard: ProcessGuard,
}

impl<'a, T: Transport> Session<'a, T> {
    pub(super) fn open(
        backend: &'a Path,
        start_path: &Path,
        output: &Path,
        recover: bool,
        transport: &'a T,
    ) -> Result<Self> {
        policy(
            "paths",
            json!({"backend":backend,"start":start_path,"output":output}),
        )?;
        let start = common::binding(start_path)?;
        let facts = running::load(backend, &start, true, recover)?;
        let run = PathBuf::from(document::text(&facts["directory"], "来源复制账本")?);
        let generation = PathBuf::from(document::text(&facts["output"], "来源运行代次")?);
        let directory = generation.join("verification");
        if output != directory.join("source-runtime.json")
            || output.try_exists()?
            || recover != directory.try_exists()?
            || recover && !directory.is_dir()
        {
            return Err(
                "来源验证必须使用同代新 verification；恢复必须保留原失败目录且尚未封口".into(),
            );
        }
        let private: BTreeMap<String, String> =
            serde_json::from_value(facts["environment"]["environment"].clone())?;
        let environment = environment::configured(&private)?;
        let original_environment = std::env::vars().collect();
        let recovery_tools = if recover {
            source::current(backend)?
        } else {
            Value::Null
        };
        if recover && recovery_tools["snapshot"]["clean"] != true {
            return Err("来源恢复工具必须先提交为干净完整源码".into());
        }
        let scope = Scope::load(
            backend,
            document::array(
                &facts["source"]["manifest"]["build_bridges"],
                "来源构建桥接",
            )?,
            &[],
        )?;
        let binding = crate::check::python_suite::native_tool_binding()?;
        let tool = PathBuf::from(document::text(&binding["path"], "当前原生协调工具")?);
        let guard = ProcessGuard::acquire(&run)?;
        let owner = RunLock::claim(&run)?;
        let session = Self {
            backend,
            start,
            output: output.to_owned(),
            directory,
            facts,
            environment,
            tool,
            recover,
            scope,
            transport,
            original_environment,
            recovery_tools,
            owner,
            guard,
        };
        if let Err(error) = session.checkpoint() {
            return session.finish(Err(error));
        }
        Ok(session)
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.guard.check()?;
        self.owner.verify()?;
        if std::env::vars().collect::<BTreeMap<_, _>>() != self.original_environment {
            return Err("来源执行器的控制环境发生变化；保留现场，禁止重放".into());
        }
        if self.recover && source::current(self.backend)? != self.recovery_tools {
            return Err("来源恢复工具的当前完整来源发生变化".into());
        }
        if running::load(self.backend, &self.start, true, self.recover)? != self.facts {
            return Err("来源运行代次、完整历史或前像在控制区间内变化".into());
        }
        self.scope.assert_unchanged()?;
        self.owner.verify()?;
        self.guard.check()
    }

    pub(super) fn finish<R>(self, result: Result<R>) -> Result<R> {
        let checked = self.checkpoint();
        let released = self.owner.release();
        match (result,checked,released) {
            (Ok(value),Ok(()),Ok(())) => Ok(value),
            (result,checked,released) => Err(format!("来源执行未确认完成；原结果：{}；退出核验：{}；控制锁释放：{}。保留完整现场，禁止自动重放",
                diagnostic(result.err()),diagnostic(checked.err()),diagnostic(released.err())).into()),
        }
    }

    pub(super) fn resources<'s>(
        &'s self,
        output: &'s Path,
        checkpoint: &'s dyn Fn() -> Result<()>,
    ) -> Result<Resources<'s, T>> {
        Ok(Resources {
            backend: self.backend,
            execution: self.execution()?,
            request: &self.facts["source"]["seed_target"],
            environment: &self.environment,
            output,
            transport: self.transport,
            checkpoint,
        })
    }

    pub(super) fn execution(&self) -> Result<&Path> {
        Ok(Path::new(document::text(
            &self.facts["execution"],
            "来源实际执行树",
        )?))
    }

    pub(super) fn producer(&self) -> Result<producer::Inputs<'_>> {
        Ok(producer::Inputs {
            backend: self.backend,
            execution: self.execution()?,
            directory: &self.directory,
            start: &self.start,
            lineage: &self.facts["receipt"]["dataset_lineage"],
            environment: &self.environment,
            tool: &self.tool,
            coordinator: &self.facts["coordinator_source"],
            execution_sha: document::text(
                &self.facts["request"]["expected_backend_sha"],
                "来源执行 SHA",
            )?,
            historical: self.recover,
        })
    }

    pub(super) fn successor(&self) -> &Value {
        &self.recovery_tools
    }
}

impl<T: Transport + Sync> Session<'_, T> {
    pub(super) fn capture(&self, phase: &str) -> Result<(Value, Value)> {
        if !matches!(phase, "before" | "after") {
            return Err("来源采集阶段无效".into());
        }
        let output = self.directory.join(phase);
        let checkpoint = || self.checkpoint();
        let resources = self.resources(&output, &checkpoint)?;
        let context = images::capture::context(&resources, &self.facts)?;
        let descriptor =
            images::capture::run(&resources, &context, &self.facts["source"], &self.scope)?;
        let value = images::verify(
            self.backend,
            &descriptor,
            &self.facts["selected"],
            &self.facts["source"]["request"],
            &self.facts["receipt"]["source_registration"],
        )?;
        Ok((descriptor, value))
    }
}

pub(super) fn policy(action: &str, arguments: Value) -> Result<Value> {
    ready::verify(&json!({"action":action,"arguments":arguments}))
}

fn diagnostic(error: Option<Box<dyn std::error::Error>>) -> String {
    error.map_or_else(|| "无".into(), |error| error.to_string())
}
