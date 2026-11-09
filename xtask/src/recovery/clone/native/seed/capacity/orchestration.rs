//! 在原 API 存活窗口内规划、一次应用或显式核对配额。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    directories, history,
    inputs::{Reads, present},
    load_plan, model, validate_plan,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        digest, json as document, lifecycle::state, resources::common,
        seed::history as seed_history,
    },
};

#[path = "orchestration/attempts.rs"]
mod attempts;

pub(crate) trait Port {
    fn check(&self) -> Result<()>;
    fn verify_administrator(&mut self) -> Result<()>;
    fn login(&mut self, tenant_ids: &Value) -> Result<Value>;
    fn get(&mut self, tenant_id: &str) -> Result<Value>;
    fn usage(&mut self, tenant_id: &str) -> Result<Value>;
    fn update(&mut self, tenant_id: &str, body: &Value) -> Result<Value>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plan,
    Apply,
    Reconcile,
}

pub(crate) struct Prepared<'a> {
    run: &'a Path,
    output: &'a Path,
    number: u64,
    post: &'a Value,
    request: &'a Value,
    saved: Value,
    reads: Reads<'a>,
    mode: Mode,
    targets: Vec<Value>,
    plan: Option<Value>,
}

impl<'a> Prepared<'a> {
    pub(crate) fn load(
        backend: &'a Path,
        run: &'a Path,
        output: &'a Path,
        number: u64,
        post: &'a Value,
        request: &'a Value,
        mode: &str,
    ) -> Result<Self> {
        let operation = match mode {
            "quotas-plan" => Mode::Plan,
            "quotas-apply" => Mode::Apply,
            "quotas-reconcile" => Mode::Reconcile,
            _ => return Err("未知 seed 配额阶段".into()),
        };
        if !(1..=9999).contains(&number)
            || output
                != run
                    .join("seed-runtime")
                    .join(format!("attempt-{number:04}"))
        {
            return Err("配额输出必须绑定当前 seed 阶段目录".into());
        }
        common::local_directory(backend, run, "seed 配额运行目录")?;
        let mut reads = Reads::new(backend);
        reads.pin_directory(output)?;
        reads.pin_directory(&run.join("seed-runtime"))?;
        reads.binding(&run.join("manifest.json"))?;
        reads.binding(&run.join("state.json"))?;
        let saved = state::load(run, true)?;
        let running = state::running_attempt(&saved)?.ok_or("缺少当前运行中的 seed 配额阶段")?;
        if running["number"] != number
            || running["stage"] != "seed-runtime"
            || running["mode"] != mode
        {
            return Err("配额输出不属于当前运行中的 seed 阶段".into());
        }
        bind_post(&mut reads, run, number, request, post)?;
        before_identities(backend, run, request, &saved)?;
        let targets = model::targets(&reads.bound(&request["identity_plan"])?)?;
        let plan = if operation == Mode::Plan {
            present(&plan_path(run))?
                .then(|| validate_plan(&mut reads, run, request))
                .transpose()?
        } else {
            Some(load_plan(&mut reads, run, request, &saved, Some(number))?)
        };
        if let Some(plan) = &plan {
            preflight(&mut reads, run, plan, operation)?;
        } else {
            for target in &targets {
                let slot = document::text(&target["slot"], "配额身份槽")?;
                if !reads
                    .directories(&run.join("seed-runtime/quotas").join(slot))?
                    .is_empty()
                {
                    return Err("配额计划缺失而动作证据仍存在，禁止登录或重新规划".into());
                }
            }
        }
        reads.check()?;
        Ok(Self {
            run,
            output,
            number,
            post,
            request,
            saved,
            reads,
            mode: operation,
            targets,
            plan,
        })
    }

    // 外层在返回后负责有界注销、生产者回收和发布阶段结果。
    pub(crate) fn run(mut self, port: &mut impl Port) -> Result<Value> {
        self.check(port)?;
        port.verify_administrator()?;
        self.check(port)?;
        if let Some(plan) = &self.plan {
            preflight(&mut self.reads, self.run, plan, self.mode)?;
        }
        self.reads.check()?;
        let tenant_ids = self
            .targets
            .iter()
            .map(|target| &target["tenant_id"])
            .collect::<Vec<_>>();
        let auth = port.login(&json!(tenant_ids))?;
        authorization_shape(&auth)?;
        model::authorization(self.post, &auth)?;
        self.reads
            .write(&self.output.join("authentication.json"), &auth)?;
        let plan = self.plan.take();
        let result = if self.mode == Mode::Plan {
            self.create_plan(port, plan.as_ref())?
        } else {
            self.execute(port, &plan.ok_or("缺少已登记的配额计划")?)?
        };
        self.check(port)?;
        Ok(result)
    }

    fn create_plan(&mut self, port: &mut impl Port, previous: Option<&Value>) -> Result<Value> {
        let mut actions = Vec::new();
        for target in &self.targets {
            self.check(port)?;
            let observed = observe(port, target)?;
            let slot = document::text(&target["slot"], "配额身份槽")?;
            self.reads
                .write(&self.output.join(format!("{slot}-before.json")), &observed)?;
            actions.push(model::action(target, &observed)?);
        }
        self.check(port)?;
        let path = plan_path(self.run);
        let mut plan = json!({"format_version":1,"kind":"devex-seed-quotas",
            "registration":self.reads.binding(&self.run.join("seed-runtime.json"))?,
            "identity_plan":self.request["identity_plan"],"actions":actions});
        plan["plan_sha256"] = json!(digest::plan_hash(&plan)?);
        let descriptor = if let Some(previous) = previous {
            for (old, new) in items(previous)?.iter().zip(items(&plan)?) {
                if ["target", "before", "after"]
                    .iter()
                    .any(|field| old[*field] != new[*field])
                {
                    return Err("已有配额计划的完整前像变化，不能覆盖或自动重新规划".into());
                }
            }
            self.reads.binding(&path)?
        } else {
            self.reads.write(&path, &plan)?
        };
        Ok(
            json!({"status":"seed_quotas_planned","plan":descriptor,"remote_writes":0,
            "worker_authorized":false,"restore_qualified":false}),
        )
    }

    fn execute(&mut self, port: &mut impl Port, plan: &Value) -> Result<Value> {
        let actions = items(plan)?;
        for item in actions {
            self.check(port)?;
            if self.mode == Mode::Apply {
                self.apply_one(port, plan, item)?;
            } else {
                self.reconcile_one(port, plan, item)?;
            }
        }
        let mut confirmed = Vec::new();
        for item in actions {
            if let Some(attempt) = directories(&mut self.reads, self.run, item)?.last()
                && present(&attempt.join("confirmed.json"))?
            {
                self.verify_after(port, plan, item, attempt)?;
                confirmed.push(self.reads.binding(&attempt.join("confirmed.json"))?);
            }
        }
        let status = if confirmed.len() == 11 {
            "seed_quotas_verified"
        } else {
            "seed_quotas_reconciled"
        };
        Ok(
            json!({"status":status,"plan":self.reads.binding(&plan_path(self.run))?,"confirmed":confirmed,
            "remote_writes":if self.mode == Mode::Reconcile {json!(0)} else {Value::Null},
            "worker_authorized":false,"restore_qualified":false}),
        )
    }

    fn check(&self, port: &impl Port) -> Result<()> {
        self.reads.check()?;
        before_identities(self.reads.backend, self.run, self.request, &self.saved)?;
        port.check()
    }

    fn verify_after(
        &mut self,
        port: &mut impl Port,
        plan: &Value,
        item: &Value,
        attempt: &Path,
    ) -> Result<()> {
        history::confirmation(&mut self.reads, self.run, plan, item, attempt)?;
        if model::classification(item, &observe(port, &item["target"])?)? != "after" {
            return Err("已确认配额或用户角色用量发生变化".into());
        }
        Ok(())
    }
}

fn items(plan: &Value) -> Result<&[Value]> {
    document::array(&plan["actions"], "配额动作")
}

fn plan_path(run: &Path) -> PathBuf {
    run.join("seed-runtime/quota-plan.json")
}

fn bind_post(
    reads: &mut Reads<'_>,
    run: &Path,
    number: u64,
    request: &Value,
    post: &Value,
) -> Result<()> {
    if reads.read(&run.join("seed-runtime.json"))? != *request {
        return Err("配额执行登记或 post-copy 绑定不同".into());
    }
    reads.binding(&run.join("post-copy.json"))?;
    let binding = &request["post_copy"];
    reads.bound(binding)?;
    let current = seed_history::post(reads.backend, run, Some(binding), Some(number), false)?;
    if current.request != *post {
        return Err("配额执行登记或 post-copy 绑定不同".into());
    }
    Ok(())
}

fn observe(port: &mut impl Port, target: &Value) -> Result<Value> {
    let tenant = document::text(&target["tenant_id"], "配额租户")?;
    Ok(json!({"tenant":port.get(tenant)?, "usage":port.usage(tenant)?}))
}

fn authorization_shape(auth: &Value) -> Result<()> {
    document::exact(
        auth,
        &[
            "subject_id",
            "tenant_id",
            "username",
            "is_super_admin",
            "permissions",
        ],
        "配额公开授权",
    )?;
    Ok(())
}

pub(crate) fn before_identities(
    backend: &Path,
    run: &Path,
    request: &Value,
    saved: &Value,
) -> Result<()> {
    let identity = Path::new(document::text(&request["identity_state"], "身份账本路径")?);
    validate_local_test_path(identity, backend, LocalTestPathKind::StateDirectory)?;
    if identity != run.join("seed-runtime/identities")
        || present(identity)?
        || present(&run.join("seed-runtime/runtime"))?
    {
        return Err("身份准备或新 runtime 已开始，不能重新规划或改变配额".into());
    }
    for item in state::attempts(saved)? {
        if item["stage"] != "seed-runtime"
            || !matches!(
                item["mode"].as_str(),
                Some("identities-apply" | "identities-verify")
            )
        {
            continue;
        }
        let number = item["number"].as_u64().ok_or("身份阶段序号无效")?;
        let output = run
            .join("seed-runtime")
            .join(format!("attempt-{number:04}"));
        validate_local_test_path(&output, backend, LocalTestPathKind::StateDirectory)?;
        for name in [
            "session-launch.json",
            "session-process.json",
            "session.stdout.log",
            "session.stderr.log",
        ] {
            if present(&output.join(name))? {
                return Err("身份生产者已启动或存在未知启动意图，不再开放配额写入".into());
            }
        }
    }
    Ok(())
}

fn preflight(reads: &mut Reads<'_>, run: &Path, plan: &Value, mode: Mode) -> Result<()> {
    for item in items(plan)? {
        let all = directories(reads, run, item)?;
        let Some(attempt) = all.last() else { continue };
        if present(&attempt.join("confirmed.json"))? {
            history::confirmation(reads, run, plan, item, attempt)?;
        } else if present(&attempt.join("intent.json"))? {
            if mode == Mode::Reconcile {
                history::intent(reads, run, item, attempt)?;
            } else {
                attempts::reconciled_before(reads, run, item, attempt)?;
            }
        } else if mode == Mode::Apply {
            attempts::partial(reads, item, attempt)?;
        }
    }
    Ok(())
}
