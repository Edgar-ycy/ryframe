//! 正式调度和既有数据核验共享同一已登记 API、复制账本和目标保护。

use serde_json::{Value, json};

use super::{
    business_binding::{self, Completion, ObjectRead},
    context::Context,
    live::Live,
    producer::{self, Session},
    schedule_images,
    schedules::{self, ReadPort, SessionPort, Settings},
};
use crate::{
    Result,
    recovery::clone::native::{
        export::transport::SystemTransport, json as document, lifecycle::state, resources::common,
        s3::app_configuration,
    },
};

pub(super) fn run(context: &Context<'_>) -> Result<(Value, Option<Completion>)> {
    let live = Live::load(context)?;
    let base = context.run.join("post-copy");
    let mode = match context.mode {
        "verify" => return verify(&live).map(|(value, completion)| (value, Some(completion))),
        "schedules" => schedules::Mode::Apply,
        "reconcile" => schedules::Mode::Reconcile,
        _ => return Err("未知复制后执行模式".into()),
    };
    let action = schedules::Prepared::load(settings(&live, &base)?, mode)?;
    producer::with_session(
        context,
        |session| {
            action.run(&mut SchedulePort {
                live: &live,
                session,
            })
        },
        || live.check(),
    )
    .map(|value| (value, None))
}

fn settings<'a>(live: &'a Live<'_, '_>, base: &'a std::path::Path) -> Result<Settings<'a>> {
    let context = live.context;
    Ok(Settings {
        backend: context.backend,
        execution: &live.execution,
        base,
        output: &context.output,
        registration: &context.post.descriptor,
        administrator: &context.post.request["source_admin"],
        pending: document::array(&context.plan["pending_target_actions"], "原复制调度待办")?,
        number: context.number,
    })
}

pub(super) fn verified_schedules(live: &Live<'_, '_>) -> Result<Vec<Value>> {
    let base = live.context.run.join("post-copy");
    let confirmed = schedules::verify(&settings(live, &base)?, &mut ReadOnly(live))?;
    require_schedule_stage(live.context, &confirmed)?;
    Ok(confirmed)
}

fn verify(live: &Live<'_, '_>) -> Result<(Value, Completion)> {
    let context = live.context;
    verified_schedules(live)?;
    live.check()?;
    let business = business_binding::prepare(
        context.backend,
        &context.output,
        &context.post.request,
        &context.plan,
        &context.selected,
    )?;
    business.check()?;
    let observed = producer::existing(context)?;
    business.check()?;
    live.check()?;
    let public = app_configuration::public_configuration(&live.execution, &live.configuration)?;
    let completion = business.finish(
        &observed,
        ObjectRead {
            configuration_root: &live.execution,
            configuration: &live.configuration,
            endpoint: document::text(&public["endpoint"], "业务目标端点")?,
            region: document::text(&public["region"], "业务目标区域")?,
        },
        &SystemTransport,
        &|| live.checkpoint(),
    )?;
    verified_schedules(live)?;
    business.check()?;
    live.check()?;
    let result = json!({"status":"post_copy_existing_data_verified","evidence":completion.evidence,
        "objects":completion.objects,"registration":context.post.descriptor,
        "worker_must_remain_stopped":true,"restore_qualified":false});
    Ok((result, completion))
}

pub(super) fn require_schedule_stage(context: &Context<'_>, confirmed: &[Value]) -> Result<()> {
    let saved = state::load(context.run, true)?;
    let latest = state::attempts(&saved)?
        .iter()
        .rfind(|row| {
            row["stage"] == "post-copy"
                && matches!(row["mode"].as_str(), Some("schedules" | "reconcile"))
        })
        .ok_or("须先完成最新调度阶段")?;
    if latest["mode"] != "schedules" || latest["status"] != "passed" {
        return Err("最新调度阶段尚未通过完整来源与收尾核验".into());
    }
    let result = common::read_bound(context.backend, &latest["result"], "调度阶段发布结果")?;
    if result["status"] != "target_schedule_actions_verified"
        || result["registration"] != context.post.descriptor
        || result["confirmed"].as_array().map(Vec::as_slice) != Some(confirmed)
    {
        return Err("调度阶段发布结果与当前完整确认不同".into());
    }
    for receipt in confirmed {
        common::bound_file(context.backend, receipt, "原已确认调度")?;
    }
    Ok(())
}

struct ReadOnly<'a, 'b, 'c>(&'a Live<'b, 'c>);

impl ReadPort for ReadOnly<'_, '_, '_> {
    fn checkpoint(&self) -> Result<()> {
        if self.0.context.stage == "post-copy" {
            self.0.check()
        } else {
            self.0.checkpoint()
        }
    }

    fn row(&mut self, action: &Value) -> Result<Value> {
        let live = self.0;
        let target = live.target();
        schedule_images::observe(
            &target,
            &live.execution,
            target.target_scope()?,
            action,
            &SystemTransport,
            &|| live.checkpoint(),
        )
    }
}

struct SchedulePort<'a, 'b, 'c, 'd, 'e> {
    live: &'a Live<'b, 'c>,
    session: &'a mut Session<'d, 'e>,
}

impl ReadPort for SchedulePort<'_, '_, '_, '_, '_> {
    fn checkpoint(&self) -> Result<()> {
        self.live.check()
    }

    fn row(&mut self, action: &Value) -> Result<Value> {
        ReadOnly(self.live).row(action)
    }
}

impl SessionPort for SchedulePort<'_, '_, '_, '_, '_> {
    fn verify_administrator(&mut self) -> Result<()> {
        schedule_images::verify_administrator(
            &self.live.source(),
            &self.live.target(),
            &self.live.context.post.request["source_admin"],
            &SystemTransport,
            &|| self.live.checkpoint(),
        )
    }

    fn login(&mut self) -> Result<Value> {
        self.session.login(None, None, None)
    }

    fn get(&mut self, action: &Value) -> Result<Value> {
        self.session
            .call("get", json!({"schedule_id":action["schedule_id"]}))
    }

    fn disable(&mut self, action: &Value) -> Result<Value> {
        self.session.call(
            "disable",
            json!({"schedule_id":action["schedule_id"],"version":action["expected_version"]}),
        )
    }
}
