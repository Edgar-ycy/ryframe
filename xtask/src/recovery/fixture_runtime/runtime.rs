//! 正式夹具运行态从冻结环境和真实构建装配，不依赖 Python 或 CI Context。
use super::{model::*, plan};
use crate::{
    Result,
    recovery::{
        clone::native::{bridge, json as strict},
        full_stack,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) const SECRET_FILES: &[&str] = &[
    "mysql-client.cnf",
    "rustfs-access-key.txt",
    "rustfs-secret-key.txt",
    "redis-password.txt",
    "reset-admin-password.txt",
    "jwt-secret.txt",
    "metrics-token.txt",
];

pub(super) fn bootstrap(backend: &Path, environment: &Path) -> Result<Value> {
    let value = super::super::fixture_control::services::lifecycle::query::dispatch(
        backend,
        "service-prepared",
        &json!({"path":environment,"secret_files":SECRET_FILES}),
    )?;
    check(
        value.as_array().is_some_and(|items| items.len() == 4),
        "冻结环境核验返回值不完整",
    )?;
    Ok(json!({"bootstrap":value[0],"execution":value[1],"private":value[2]}))
}

pub(super) struct Runtime {
    pub execution: PathBuf,
    pub output: PathBuf,
    pub receipt: Value,
    pub build: Value,
    guards: Vec<Value>,
    sources: Vec<plan::SourceGuard>,
    private: Value,
    environment_guard: Value,
}

impl Runtime {
    pub(super) fn protect(
        &self,
    ) -> Result<Vec<crate::recovery::clone::native::target_verify::guard::Inputs>> {
        use crate::recovery::clone::native::target_verify::guard::Inputs;
        let mut protected = self
            .sources
            .iter()
            .map(plan::SourceGuard::protect)
            .collect::<Result<Vec<_>>>()?;
        let paths = self
            .guards
            .iter()
            .chain(std::iter::once(&self.environment_guard))
            .map(|guard| path(&guard["descriptor"]["path"]))
            .collect::<Result<Vec<_>>>()?;
        protected.push(Inputs::open(paths)?);
        self.verify()?;
        Ok(protected)
    }
    pub(super) fn load(backend: &Path, environment: &Path, output: &Path) -> Result<Self> {
        let boot = bootstrap(backend, environment)?;
        let execution = path(&boot["execution"])?;
        let allowed = execution.join(".local-tests/reference-fixture");
        directory(output)?;
        check(
            output.starts_with(&allowed) && output != allowed,
            "夹具运行目录越界",
        )?;
        let receipt = super::super::fixture_control::verify_full_stack_runtime(&execution, output)?;
        let build = document(&output.join("build-evidence.json"))?.0;
        check(
            full_stack::provenance::current(&execution, output)? == build["source"],
            "构建证据来源不匹配",
        )?;
        let backend_build = document(&output.join("backend-build.json"))?.0;
        bridge::verify_build(
            &execution,
            &output.join("backend-build.json"),
            &backend_build,
        )?;
        let mut guards = plan::input_guards(&boot)?;
        let bootstrap_file = path(&boot["bootstrap"])?;
        let bootstrap_value = document(&bootstrap_file)?.0;
        guards.push(snapshot(&bootstrap_file, MAX_BYTES)?);
        let (environment, environment_guard) = document(
            &bootstrap_file
                .parent()
                .ok_or("环境缺少父目录")?
                .join("environment.json"),
        )?;
        strict::exact(&environment, &["environment"], "夹具私有环境文件")?;
        check(
            environment["environment"] == boot["private"],
            "环境文件与 prepared 核验结果不一致",
        )?;
        guards.extend(plan::input_guards(&bootstrap_value)?);
        for name in [
            "source-pair.json",
            "build-evidence.json",
            "backend-build.json",
            "runtime.json",
            "runtime-evidence.json",
            "binaries.json",
        ] {
            guards.push(snapshot(&output.join(name), MAX_BYTES)?);
        }
        let mut sources = vec![];
        for side in ["backend", "frontend"] {
            let root = path(&build["source"]["roots"][side])?;
            sources.push(plan::SourceGuard::capture(
                &root,
                &build["source"]["generated"][side],
            )?);
        }
        let value = Self {
            execution,
            output: output.to_owned(),
            receipt,
            build,
            guards,
            sources,
            private: boot["private"].clone(),
            environment_guard,
        };
        value.verify()?;
        Ok(value)
    }

    pub(super) fn verify(&self) -> Result<()> {
        unchanged(&self.environment_guard)?;
        for guard in &self.guards {
            unchanged(guard)?;
        }
        for source in &self.sources {
            source.unchanged()?;
        }
        check(
            super::super::fixture_control::verify_full_stack_runtime(
                &self.execution,
                &self.output,
            )? == self.receipt,
            "夹具运行合同或配置在执行期间漂移",
        )
    }

    pub(super) fn controller(
        &self,
        role: full_stack::worker::role::Role,
        action: &str,
    ) -> Result<std::process::Command> {
        unchanged(&self.environment_guard)?;
        let mut command =
            full_stack::worker::control_command(role, &self.execution, &self.output, action, 60.0)?;
        self.configure_environment(&mut command)?;
        Ok(command)
    }

    /// 只叠加已绑定的应用环境与可选临时目录，操作系统环境由调用方保留。
    pub(super) fn configure_environment(&self, command: &mut std::process::Command) -> Result<()> {
        unchanged(&self.environment_guard)?;
        command.env("APP_ENV", text(&self.private["APP_ENV"])?);
        for key in ["TEMP", "TMP"] {
            if let Some(value) = self.private[key].as_str() {
                command.env(key, value);
            }
        }
        unchanged(&self.environment_guard)
    }

    pub(super) fn summary(&self) -> Result<Value> {
        self.verify()?;
        Ok(
            json!({"status":"reference_fixture_runtime_verified","runtime":bound(&self.output.join("runtime.json"))?,
            "scope_id":self.receipt["scope_id"],"build_evidence":self.build,"remote_writes":0}),
        )
    }
}

pub(super) fn verify(args: &Value) -> Result<Value> {
    strict::exact(
        args,
        &["backend", "environment", "output"],
        "夹具运行核验参数",
    )?;
    Runtime::load(
        &path(&args["backend"])?,
        &path(&args["environment"])?,
        &path(&args["output"])?,
    )?
    .summary()
}

pub(super) fn control(args: &Value, observe: bool, worker_only: bool) -> Result<Value> {
    use full_stack::worker::{controller, role::Role};
    strict::exact(
        args,
        &["backend", "environment", "output", "action"],
        "夹具运行控制参数",
    )?;
    let action = text(&args["action"])?;
    check(
        matches!(action, "start" | "stop" | "status") && (!observe || action == "status"),
        "夹具运行操作无效",
    )?;
    let context = Runtime::load(
        &path(&args["backend"])?,
        &path(&args["environment"])?,
        &path(&args["output"])?,
    )?;
    let roles = if worker_only {
        vec![Role::Worker]
    } else if action == "stop" {
        vec![Role::Worker, Role::Api]
    } else {
        vec![Role::Api, Role::Worker]
    };
    let mut processes = serde_json::Map::new();
    for role in roles {
        context.verify()?;
        let result = if observe || action == "status" {
            controller::observe(role, &context.execution, &context.output)
                .map(|value| value["state"].clone())
        } else {
            super::worker_handoff::control(&context, role, action)
        };
        context.verify()?;
        processes.insert(role.name().into(), result?);
    }
    if worker_only {
        return Ok(processes["worker"].clone());
    }
    Ok(
        json!({"status":if observe { "reference_fixture_runtime_observed".into() } else {format!("reference_fixture_runtime_{action}")},
        "runtime":bound(&context.output.join("runtime.json"))?,"processes":processes,"remote_writes":0}),
    )
}
