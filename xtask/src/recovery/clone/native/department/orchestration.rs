//! 按固定租户和身份前提执行部门阶段，动作证据仍由既有模块发布。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{common, document, model, plan, workflow};
use crate::{
    Result,
    recovery::clone::native::{lifecycle::state, seed::latest},
};

#[path = "orchestration/actions.rs"]
mod actions;

pub(crate) trait Port {
    fn check(&self, prerequisite: Option<&Value>, quick: bool) -> Result<()>;
    fn verify_administrator(&mut self) -> Result<()>;
    fn login(&mut self, tenant_ids: &Value, templates: &Value) -> Result<Value>;
    fn call(&mut self, operation: &str, values: Value) -> Result<Value>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plan,
    Apply,
    Reconcile,
    Verify,
}

impl Mode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "departments-plan" => Ok(Self::Plan),
            "departments-apply" => Ok(Self::Apply),
            "departments-reconcile" => Ok(Self::Reconcile),
            "departments-verify" => Ok(Self::Verify),
            _ => Err("未知部门执行阶段".into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Plan => "departments-plan",
            Self::Apply => "departments-apply",
            Self::Reconcile => "departments-reconcile",
            Self::Verify => "departments-verify",
        }
    }
}

struct Published {
    plan: Value,
    prerequisite: Value,
    descriptor: Value,
}

pub(crate) struct Prepared<'a> {
    backend: &'a Path,
    run: &'a Path,
    output: &'a Path,
    output_id: (u64, u64),
    request: &'a Value,
    mode: Mode,
    files: Vec<Value>,
    targets: Vec<Value>,
    templates: Value,
    published: Option<Published>,
}

impl<'a> Prepared<'a> {
    pub(crate) fn load(
        backend: &'a Path,
        run: &'a Path,
        output: &'a Path,
        number: u64,
        request: &'a Value,
        mode: &str,
    ) -> Result<Self> {
        let selected = Mode::parse(mode)?;
        if number == 0 || output != run.join(format!("seed-runtime/attempt-{number:04}")) {
            return Err("部门输出必须绑定当前 seed 阶段目录".into());
        }
        common::local_directory(backend, run, "seed 部门运行目录")?;
        let output_id = output_identity(backend, output)?;
        let mut files = ["manifest.json", "state.json", "seed-runtime.json"]
            .iter()
            .map(|name| common::binding(&run.join(name)))
            .collect::<Result<Vec<_>>>()?;
        if common::read_bound(backend, &files[2], "seed 部门登记")? != *request {
            return Err("部门请求不同于当前 seed 登记".into());
        }
        files.push(request["identity_plan"].clone());
        let saved = state::load(run, true)?;
        let running = state::running_attempt(&saved)?.ok_or("缺少当前运行的 seed 部门阶段")?;
        if running["number"] != number
            || running["stage"] != "seed-runtime"
            || running["mode"] != mode
        {
            return Err("部门输出不属于当前运行阶段".into());
        }
        let targets = plan::targets(&plan::identity_plan(backend, request)?)?;
        let templates = plan::template_identities(backend, run, request)?;
        let published = if selected == Mode::Plan {
            if plan::identity_projection(backend, run, request, true)?["state"] == "verified" {
                return Err("身份已验证后不再补建部门计划".into());
            }
            None
        } else {
            Some(load_published(
                backend, run, number, request, &saved, &mut files,
            )?)
        };
        let prepared = Self {
            backend,
            run,
            output,
            output_id,
            request,
            mode: selected,
            files,
            targets,
            templates,
            published,
        };
        prepared.check_files()?;
        prepared.preflight()?;
        Ok(prepared)
    }

    // 调用方负责有界注销、生产者回收和最终阶段结果发布。
    pub(crate) fn run(mut self, port: &mut impl Port) -> Result<Value> {
        let published = self.published.take();
        let prior = published.as_ref().map(|item| &item.prerequisite);
        self.guard(port, prior, false)?;
        port.verify_administrator()?;
        self.check_files()?;
        if plan::template_identities(self.backend, self.run, self.request)? != self.templates {
            return Err("部门模板身份在登录前变化".into());
        }
        if self.mode == Mode::Apply {
            let plan = &published.as_ref().ok_or("缺少部门计划")?.plan;
            self.check_unknown_writes(plan)?;
        }
        let ids = self
            .targets
            .iter()
            .map(|target| &target["tenant_id"])
            .collect::<Vec<_>>();
        let auth = port.login(&json!(ids), &self.templates)?;
        self.check_files()?;
        self.workflow(
            "authenticate",
            json!({"request":self.request,"auth":auth,"output":self.output}),
        )?;
        if let Some(published) = published {
            self.guard(port, Some(&published.prerequisite), true)?;
            let result = self.execute(port, &published)?;
            self.check_files()?;
            Ok(result)
        } else {
            self.create_plan(port)
        }
    }

    fn preflight(&self) -> Result<()> {
        if let Some(published) = &self.published {
            self.require_prerequisite(&published.prerequisite)?;
            if self.mode == Mode::Apply {
                self.check_unknown_writes(&published.plan)?;
            }
        }
        Ok(())
    }

    fn create_plan(&mut self, port: &mut impl Port) -> Result<Value> {
        let prior = self.workflow("prepare-prerequisite", json!({"request":self.request}))?;
        self.files.push(common::binding(
            &self
                .run
                .join("seed-runtime/departments/identity-prerequisite.json"),
        )?);
        let templates = document::array(&self.templates, "部门模板身份")?;
        let mut actions = Vec::new();
        for (target, template) in self.targets.iter().zip(templates) {
            let action = self.workflow(
                "initial-action",
                json!({"target":target,"template":template}),
            )?;
            self.guard(port, Some(&prior), false)?;
            let observed = actions::observe(port, &action, target)?;
            actions.push(self.workflow(
                "plan-action",
                json!({"action":action,"target":target,
                "observed":observed,"output":self.output}),
            )?);
        }
        self.guard(port, Some(&prior), false)?;
        let result = self.workflow(
            "publish-plan",
            json!({"request":self.request,"actions":actions}),
        )?;
        self.check_files()?;
        Ok(result)
    }

    fn execute(&self, port: &mut impl Port, published: &Published) -> Result<Value> {
        if self.mode == Mode::Verify {
            return self.verify(port, published);
        }
        let mut writes = 0;
        let mut unresolved = Vec::new();
        for (action, target) in plan_actions(&published.plan)?.iter().zip(&self.targets) {
            self.guard(port, Some(&published.prerequisite), true)?;
            if self.mode == Mode::Apply {
                writes += u64::from(self.apply(port, &published.prerequisite, action, target)?);
            } else if self.reconcile(port, &published.prerequisite, action, target)?
                == Some("before")
            {
                unresolved.push(action["target"]["slot"].clone());
            }
        }
        self.workflow(
            "stage-result",
            json!({"plan":published.plan,"targets":self.targets,
            "mode":self.mode.name(),"writes":writes,"unresolved":unresolved}),
        )
    }

    fn workflow(&self, operation: &str, args: Value) -> Result<Value> {
        workflow::dispatch(self.backend, self.run, operation, &args)
    }

    fn require_prerequisite(&self, prior: &Value) -> Result<()> {
        plan::dispatch(
            self.backend,
            self.run,
            &json!("require-prerequisite"),
            &json!({"request":self.request,"value":prior}),
        )?;
        Ok(())
    }

    fn guard(&self, port: &impl Port, prior: Option<&Value>, quick: bool) -> Result<()> {
        self.check_files()?;
        port.check(prior, quick)?;
        self.check_files()
    }

    fn check_files(&self) -> Result<()> {
        for value in &self.files {
            common::bound_file(self.backend, value, "部门阶段固定证据")?;
        }
        if output_identity(self.backend, self.output)? != self.output_id {
            return Err("部门阶段输出目录身份变化，保留现场".into());
        }
        Ok(())
    }
}

fn load_published(
    backend: &Path,
    run: &Path,
    number: u64,
    request: &Value,
    saved: &Value,
    files: &mut Vec<Value>,
) -> Result<Published> {
    let record = latest(
        saved,
        "seed-runtime",
        &["departments-plan"],
        Some(number),
        false,
    )?;
    let descriptor = common::binding(&run.join("seed-runtime/department-plan.json"))?;
    let value = workflow::dispatch(
        backend,
        run,
        "load-plan",
        &json!({"request":request,"record":record}),
    )?;
    let prerequisite = plan::read_prerequisite(backend, run)?;
    files.extend([
        record["result"].clone(),
        descriptor.clone(),
        value["prerequisite"].clone(),
    ]);
    Ok(Published {
        plan: value,
        prerequisite,
        descriptor,
    })
}

fn output_identity(backend: &Path, path: &Path) -> Result<(u64, u64)> {
    common::local_directory(backend, path, "部门阶段输出")?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn plan_actions(value: &Value) -> Result<&[Value]> {
    document::array(&value["actions"], "部门动作")
}
