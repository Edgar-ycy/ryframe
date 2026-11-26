//! 正式控制窗口内的受管会话与已有数据生产者。
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::super::{
    json as document,
    lifecycle::identity,
    resources::common,
    s3::app_configuration,
    target_verify::{guard::Inputs, input::request},
};
use super::{command, context::Context, files, protocol, runtime};
use crate::Result;

#[path = "producer/department.rs"]
mod department;
#[path = "producer/identity.rs"]
mod identity_producer;
#[path = "producer/job.rs"]
mod job;
#[path = "producer/pipes.rs"]
mod pipes;
#[path = "producer/quota.rs"]
mod quota;

pub(in crate::recovery::clone::native) use department::{DepartmentSession, with_department};
pub(in crate::recovery::clone::native) use identity_producer::with_identity;
pub(in crate::recovery::clone::native) use quota::{QuotaSession, with_quota};

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/producer.rs"]
mod tests;

const RPC_TIMEOUT: Duration = Duration::from_secs(180);
const EXISTING_TIMEOUT: Duration = Duration::from_secs(1800);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Session,
    Existing,
    QuotaPlan,
    QuotaApply,
    QuotaReconcile,
    Department(&'static str),
    Identity(&'static str),
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Existing => "existing",
            Self::QuotaPlan => "quota-plan",
            Self::QuotaApply => "quota-apply",
            Self::QuotaReconcile => "quota-reconcile",
            Self::Department(name) | Self::Identity(name) => name,
        }
    }

    fn seed_registered(self) -> bool {
        !matches!(self, Self::Session | Self::Existing)
    }

    fn session_framing(self) -> bool {
        !matches!(self, Self::Existing | Self::Identity(_))
    }
}

pub(super) fn with_session<T>(
    context: &Context<'_>,
    body: impl FnOnce(&mut Session<'_, '_>) -> Result<T>,
    final_guard: impl FnOnce() -> Result<()>,
) -> Result<T> {
    let mut session = Session::new(Producer::start(context, Kind::Session, None)?);
    let result = body(&mut session);
    finish_session(
        session,
        result,
        ["session-logout", "session-final-guard", "session-cleanup"],
        final_guard,
    )
}

fn finish_session<T>(
    mut session: Session<'_, '_>,
    result: Result<T>,
    [logout_label, guard_label, close_label]: [&str; 3],
    final_guard: impl FnOnce() -> Result<()>,
) -> Result<T> {
    let context = session.process.context;
    let logout = if session.attempted_login {
        session.logout(Instant::now() + LOGOUT_TIMEOUT)
    } else {
        Ok(())
    };
    let result = retain_cleanup(context, logout_label, "ProducerLogoutError", result, logout);
    let result = retain_cleanup(
        context,
        guard_label,
        "ProducerFinalGuardError",
        result,
        final_guard(),
    );
    session.process.finish(result, close_label, None)
}

pub(super) fn existing(context: &Context<'_>) -> Result<Value> {
    let mut process = Producer::start(context, Kind::Existing, None)?;
    let result = (|| {
        let deadline = Instant::now() + EXISTING_TIMEOUT;
        let authorization =
            json!({"operation":"start","run_dir":context.run,"attempt":context.number});
        let raw = process.exchange(&authorization, true, deadline)?;
        if !process.job.wait(deadline)?.success() {
            return Err("既有数据原生验证未成功退出；保留独立日志，禁止重放".into());
        }
        let value = parse(&raw, "既有数据原生验证结果")?;
        document::object(&value, "既有数据原生验证结果")?;
        context.check()?;
        Ok(value)
    })();
    process.finish(result, "session-cleanup", None)
}

pub(super) struct Session<'a, 'r> {
    process: Producer<'a, 'r>,
    number: u64,
    attempted_login: bool,
    ready: bool,
    channel: Channel,
}

impl<'a, 'r> Session<'a, 'r> {
    fn new(process: Producer<'a, 'r>) -> Self {
        Self {
            process,
            number: 0,
            attempted_login: false,
            ready: false,
            channel: Channel::Open,
        }
    }

    pub(super) fn login(
        &mut self,
        tenant_ids: Option<&Value>,
        post_request: Option<&Value>,
        template_identities: Option<&Value>,
    ) -> Result<Value> {
        if self.attempted_login || tenant_ids.is_some() || template_identities.is_some() {
            return Err("post-copy 会话仅接受一次原管理员初始化".into());
        }
        let context = self.process.context;
        let request = post_request.unwrap_or(&context.post.request);
        let directory = request::config_directory(context.backend, &context.target)?;
        let config = json!({
            "contract":{"timeout_ms":120000,"pacing":request["pacing"]["contract"]},
            "bindings":{"scope_id":context.target["target"]["scope_id"],
                "api_url":context.selected["api_url"],"frontend_url":context.selected["frontend_url"],
                "pacing":request["pacing"]["bindings"],"app_config_dir":directory["path"]},
        });
        let admin = &request["source_admin"];
        let identity = json!({"subject_id":admin["subject_id"],"tenant_id":admin["tenant_id"],
            "username":admin["username"],"client_address":admin["client_address"]});
        self.initialize(&request["frontend_root"], &config, &identity, None)
    }

    fn initialize(
        &mut self,
        frontend: &Value,
        config: &Value,
        identity: &Value,
        tenant_ids: Option<&Value>,
    ) -> Result<Value> {
        let mut values = self.initialization_fields(frontend, config, identity);
        if let Some(tenant_ids) = tenant_ids {
            values["tenant_ids"] = tenant_ids.clone();
        }
        self.send_initialization(values)
    }

    fn initialization_fields(&self, frontend: &Value, config: &Value, identity: &Value) -> Value {
        let context = self.process.context;
        json!({"backend":context.backend,"frontend":frontend,
            "config":config,"identity":identity,"artifacts":context.output.join("pacing"),
            "run_dir":context.run,"attempt":context.number,"producer_owner":self.process.owner,
            "launch":self.process.launch,"process":self.process.process})
    }

    fn send_initialization(&mut self, values: Value) -> Result<Value> {
        if self.attempted_login {
            return Err("已尝试初始化本代会话；禁止重放".into());
        }
        self.attempted_login = true;
        let result = self.rpc("initialize", values, RPC_TIMEOUT);
        self.ready = result.is_ok();
        result
    }

    pub(super) fn call(&mut self, operation: &str, values: Value) -> Result<Value> {
        if !self.ready {
            return Err("post-copy 会话尚未明确登录".into());
        }
        let fields: &[&str] = match operation {
            "get" => &["schedule_id"],
            "disable" => &["schedule_id", "version"],
            _ => return Err("post-copy 会话操作未授权".into()),
        };
        document::exact(&values, fields, "固定调度会话请求")?;
        self.rpc(operation, values, RPC_TIMEOUT)
    }

    fn logout(&mut self, deadline: Instant) -> Result<()> {
        if let Some(deadline) = self.channel.logout_deadline(deadline)? {
            let _ = self.process.job.wait(deadline)?;
            return Err("生产者已退出，自动注销未收到独立确认".into());
        }
        let value = self.rpc_until("close", json!({}), deadline)?;
        if value != json!({"closed":true}) {
            return Err("生产者注销响应未确认；保留证据，禁止重放".into());
        }
        self.ready = false;
        Ok(())
    }

    fn rpc(&mut self, operation: &str, values: Value, timeout: Duration) -> Result<Value> {
        self.rpc_until(operation, values, Instant::now() + timeout)
    }

    fn rpc_until(&mut self, operation: &str, values: Value, deadline: Instant) -> Result<Value> {
        if !matches!(self.channel, Channel::Open) {
            return Err("生产者已终止或前次结果未知；禁止继续发送会话请求".into());
        }
        self.number = self.number.checked_add(1).ok_or("生产者 RPC 序号耗尽")?;
        let request = sequenced(self.number, operation, values)?;
        self.channel = Channel::Unknown;
        let raw = self.process.exchange(&request, false, deadline)?;
        let value = parse(&raw, "复制会话响应")?;
        match response(self.number, &value) {
            Reply::Data(data) => {
                self.channel = Channel::Open;
                Ok(data)
            }
            Reply::Failure {
                diagnostic,
                channel,
            } => {
                self.channel = channel;
                document::write_new(
                    &self
                        .process
                        .context
                        .output
                        .join(format!("http-failure-{}.json", self.number)),
                    &diagnostic,
                )?;
                Err("精确会话请求失败或响应不明；保留诊断，禁止重放".into())
            }
        }
    }
}

struct Producer<'a, 'r> {
    context: &'a Context<'r>,
    job: job::Job,
    pipes: pipes::Pipes,
    identity: Value,
    owner: Value,
    launch: Value,
    process: Value,
    authority: Inputs,
    receipts: Inputs,
}

impl<'a, 'r> Producer<'a, 'r> {
    fn start(context: &'a Context<'r>, kind: Kind, seed: Option<SeedLaunch<'_>>) -> Result<Self> {
        let (plan, owner, authority) = prepare(context, kind, seed)?;
        let launch_path = context.output.join("session-launch.json");
        let process_path = context.output.join("session-process.json");
        let controller = common::bound_file(
            context.backend,
            &plan["launch"]["controller"],
            "生产者控制器",
        )?;
        let controller_owner = context.run.join("run.lock/owner.json");
        let prelaunch = Inputs::open([
            launch_path.clone(),
            controller.clone(),
            controller_owner.clone(),
        ])?;
        if document::read(&launch_path)? != plan["launch"] {
            return Err("原生生产者启动收据与已授权记录不同".into());
        }
        let argv = document::array(&plan["command"], "授权生产者命令")?
            .iter()
            .map(|arg| document::text(arg, "生产者参数").map(str::to_owned))
            .collect::<Result<Vec<_>>>()?;
        let log = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(context.output.join("session.stderr.log"))?;
        let mut job = job::Job::spawn(context.backend, &argv, log)?;
        let mut observed_identity = None;
        let registration = (|| {
            let identity = job.identity()?;
            observed_identity = Some(identity.clone());
            let recorded = protocol::record_process(&json!({
                "output":context.output,"identity":identity,"kind":kind.name(),"owner":owner,
            }))?;
            let launch = common::binding(&launch_path)?;
            let process = common::binding(&process_path)?;
            if document::read(&process_path)? != recorded || recorded["launch"] != launch {
                return Err("原生生产者进程收据与启动及内核登记不同".into());
            }
            let receipts = Inputs::open([launch_path, process_path, controller, controller_owner])?;
            prelaunch.check()?;
            receipts.check()?;
            authority.check()?;
            job.check(&identity)?;
            let pipes = job.pipes(kind.session_framing())?;
            Ok((identity, launch, process, receipts, pipes))
        })();
        match registration {
            Ok((identity, launch, process, receipts, pipes)) => Ok(Self {
                context,
                job,
                pipes,
                identity,
                owner,
                launch,
                process,
                authority,
                receipts,
            }),
            Err(error) => retain_cleanup(
                context,
                "session-cleanup",
                "ProducerCleanupError",
                Err(error),
                job.finish(observed_identity),
            ),
        }
    }

    fn exchange(&self, value: &Value, close: bool, deadline: Instant) -> Result<Vec<u8>> {
        self.context.owner.check()?;
        self.authority.checkpoint()?;
        self.receipts.checkpoint()?;
        self.job.check(&self.identity)?;
        self.pipes.exchange(value, close, deadline)
    }

    fn finish<T>(
        self,
        result: Result<T>,
        cleanup_label: &str,
        deadline: Option<Instant>,
    ) -> Result<T> {
        let Self {
            context,
            job,
            pipes,
            identity,
            authority,
            receipts,
            ..
        } = self;
        let check = || authority.check().and_then(|()| receipts.check());
        let (checked, closed) = if let Some(deadline) = deadline {
            let stopped = job.finish_until(Some(identity), deadline);
            let closed = files::combine(stopped, pipes.finish_until(deadline));
            (check(), closed)
        } else {
            let checked = check();
            let stopped = job.finish(Some(identity));
            (checked, files::combine(stopped, pipes.finish()))
        };
        let result = files::combine(result, checked);
        retain_cleanup(
            context,
            cleanup_label,
            "ProducerCleanupError",
            result,
            closed,
        )
    }
}

struct SeedLaunch<'a> {
    request: &'a Value,
    descriptor: &'a Value,
    identity_plan: Value,
    inputs: BTreeSet<PathBuf>,
}

fn prepare(
    context: &Context<'_>,
    kind: Kind,
    seed: Option<SeedLaunch<'_>>,
) -> Result<(Value, Value, Inputs)> {
    context.check()?;
    if !command::accepts(kind.name(), context.mode) || kind.seed_registered() != seed.is_some() {
        return Err("原生生产者类型不属于当前正式阶段".into());
    }
    let (configuration, private) = configuration(context, kind)?;
    let (registration, descriptor, tool) = match &seed {
        Some(seed) => (
            seed.request,
            seed.descriptor,
            seed.request["identity_tool"].clone(),
        ),
        None => {
            let tool = crate::rate_limit::current_tool_binding()?;
            (
                &context.post.request,
                &context.post.descriptor,
                json!({"path":tool["path"],"sha256":tool["sha256"]}),
            )
        }
    };
    let executable = request::external_file(&tool)?;
    let source = command::source(context.backend, kind.name())?;
    let authority = Inputs::open(
        [executable, source.clone()]
            .into_iter()
            .chain(configuration.iter().cloned())
            .chain(private)
            .chain(
                seed.iter()
                    .flat_map(|binding| binding.inputs.iter().cloned()),
            ),
    )?;
    if let Some(seed) = &seed {
        seed.check(context)?;
    }
    let owner = if kind != Kind::Existing {
        json!(uuid::Uuid::new_v4().to_string())
    } else {
        Value::Null
    };
    let plan = protocol::prepare_launch(&json!({
        "backend":context.backend,"run_dir":context.run,"attempt":context.number,
        "kind":kind.name(),"output":context.output,"tool":tool,"registration":registration,
        "configuration":configuration,"registration_binding":descriptor,
        "owner":owner,"controller_identity":identity::current()?,
    }))?;
    authority.check()?;
    if plan["launch"]["tool"] != tool || plan["launch"]["source"] != common::binding(&source)? {
        return Err("生产者工具或来源与授权启动记录不同".into());
    }
    Ok((plan, owner, authority))
}

fn configuration(context: &Context<'_>, kind: Kind) -> Result<(Option<PathBuf>, Vec<PathBuf>)> {
    if kind != Kind::Existing {
        return Ok((None, Vec::new()));
    }
    let runtime = runtime::contract(
        context.backend,
        &context.product_root()?,
        &context.post.runtime,
    )?;
    let directory = Path::new(document::text(
        &runtime["api_config_dir"],
        "原运行 API 配置",
    )?);
    let binding = app_configuration::configuration_value(context.backend, directory, "test")?;
    let mut private = vec![
        common::common_file(
            context.backend,
            &context.output.join("business-target.json"),
            "既有数据目标绑定",
        )?,
        context.post.runtime.join("runtime.json"),
    ];
    private.extend(
        document::object(&binding["files"], "既有数据 TOML")?
            .keys()
            .map(|name| directory.join(name)),
    );
    let output = context.output.join("existing-configuration.json");
    document::write_new(&output, &binding)?;
    Ok((Some(output), private))
}

fn retain_cleanup<T>(
    context: &Context<'_>,
    label: &str,
    category: &str,
    primary: Result<T>,
    cleanup: Result<()>,
) -> Result<T> {
    protocol::retain_cleanup(
        context.backend,
        &context.output,
        label,
        category,
        primary,
        cleanup,
    )
}

fn sequenced(number: u64, operation: &str, mut values: Value) -> Result<Value> {
    let fields = values.as_object_mut().ok_or("生产者请求值必须为对象")?;
    if fields.contains_key("id") || fields.contains_key("operation") {
        return Err("生产者请求不能覆盖固定序号或操作".into());
    }
    fields.insert("id".into(), json!(number));
    fields.insert("operation".into(), json!(operation));
    Ok(values)
}

fn parse(raw: &[u8], label: &str) -> Result<Value> {
    document::parse_fragment(
        std::str::from_utf8(raw).map_err(|_| "生产者返回无效 UTF-8；结果未知，禁止重放")?,
        label,
    )
    .map_err(|_| "生产者返回无效 JSON；结果未知，禁止重放".into())
}

enum Channel {
    Open,
    Terminal,
    Unknown,
}

impl Channel {
    fn logout_deadline(&self, deadline: Instant) -> Result<Option<Instant>> {
        match self {
            Self::Open => Ok(None),
            // 子进程先发终止错误回包，再在自身的注销时限内退出。
            Self::Terminal => Ok(Some(deadline)),
            Self::Unknown => Err("生产者前次结果未知；禁止继续发送会话请求".into()),
        }
    }
}

enum Reply {
    Data(Value),
    Failure { diagnostic: Value, channel: Channel },
}

fn response(number: u64, value: &Value) -> Reply {
    if value["id"].as_u64() == Some(number)
        && value["ok"] == true
        && document::exact(value, &["id", "ok", "data"], "复制会话响应").is_ok()
    {
        return Reply::Data(value["data"].clone());
    }
    let category = value["error_type"].as_str().filter(|text| {
        !text.is_empty()
            && text.len() <= 96
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    });
    let status = &value["status"];
    if value["ok"] == false
        && category.is_some()
        && (status.is_null() || status.as_u64().is_some_and(|v| (100..=599).contains(&v)))
        && (value["id"].is_null() || value["id"].is_u64())
        && document::exact(
            value,
            &["id", "ok", "error_type", "status"],
            "复制会话失败响应",
        )
        .is_ok()
    {
        let channel = if value["id"].as_u64() == Some(number) {
            Channel::Terminal
        } else {
            Channel::Unknown
        };
        Reply::Failure {
            diagnostic: value.clone(),
            channel,
        }
    } else {
        Reply::Failure {
            diagnostic: json!({"id":value["id"].as_u64(),"ok":false,
                "error_type":"InvalidSessionResponse","status":null}),
            channel: Channel::Unknown,
        }
    }
}
