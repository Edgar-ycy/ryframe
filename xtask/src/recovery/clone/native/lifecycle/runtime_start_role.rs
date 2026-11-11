//! 单次启动意图、原产品创建身份、就绪及本次自有子进程回收。
use super::super::{
    identity, runtime_history,
    runtime_spawn::{self, Launch, OwnedChild},
    runtime_status::Context,
};
#[cfg(not(windows))]
use crate::recovery::fixture_control::services::lifecycle::stop_recovery_process;
use crate::{
    Result,
    cli::CloneRole,
    recovery::{
        clone::native::json as document, fixture_control::services::lifecycle::readonly::process,
        full_stack::worker::controller,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub(super) struct Started {
    child: OwnedChild,
    role: CloneRole,
    identity: Value,
}

impl Started {
    pub(super) fn rollback(
        mut self,
        context: &Context,
        check_lock: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        close_owned(&mut self.child, Some(&self.identity))?;
        check_lock()?;
        publish(
            context,
            json!({"event":"startup-rollback","role":self.role.as_str(),"identity":self.identity}),
        )
    }
}

pub(super) fn start(
    backend: &Path,
    context: &Context,
    environment: &BTreeMap<String, String>,
    role: CloneRole,
    check_lock: impl Fn() -> Result<()>,
) -> Result<(Started, Value)> {
    let name = role.as_str();
    #[cfg(windows)]
    runtime_spawn::check_pending(&context.directory, name)?;
    let (url, _) = context.endpoint(role);
    let contract = &context.binding.receipt;
    let artifact = &contract["artifacts"][name];
    let executable = Path::new(document::text(&artifact["path"], "已登记角色产物")?);
    let configuration = Path::new(document::text(
        &contract[format!("{name}_config_dir")],
        "已登记角色配置目录",
    )?);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let log_name = format!("{name}-{token}.log");
    publish(
        context,
        json!({"event":"start-intent","role":name,"token":token,
        "artifact":artifact,"ready_url":url,"configuration_sha256":contract["configuration_sha256"],"log":log_name}),
    )?;
    let mut child = None;
    let mut observed = None;
    let result: Result<Value> = (|| {
        let child = child.insert(runtime_spawn::spawn(&Launch {
            backend,
            artifact: executable,
            configuration,
            environment,
            log: &context.directory.join(&log_name),
        })?);
        let product = observe_product(child, executable)?;
        observed = Some(product.clone());
        check_lock()?;
        let path = context.directory.join(format!("{name}.json"));
        let receipt = json!({"format_version":1,"role":name,"scope_id":context.binding.scope,"identity":product});
        if super::super::runtime_status::role_receipt(&path)?.is_some() {
            document::write_replace(&path, &receipt)?;
        } else {
            document::write_new(&path, &receipt)?;
        }
        publish(
            context,
            json!({"event":"started","role":name,"token":token,"identity":product}),
        )?;
        ready(child, context, role, &product, &check_lock)?;
        #[cfg(windows)]
        child.clear_pending()?;
        Ok(product)
    })();
    match result {
        Ok(identity) => {
            let child = child.ok_or("启动确认缺少本次子进程")?;
            let result = json!({"state":"running","identity":identity,"ready":true,"log":log_name});
            Ok((
                Started {
                    child,
                    role,
                    identity,
                },
                result,
            ))
        }
        Err(error) => {
            if let Some(mut child) = child {
                close_owned(&mut child, observed.as_ref())?;
                #[cfg(windows)]
                child.clear_pending()?;
            }
            check_lock()?;
            record_start_failure(context, name, &token, &log_name, error.as_ref())?;
            Err(error)
        }
    }
}

fn record_start_failure(
    context: &Context,
    name: &str,
    token: &str,
    log_name: &str,
    error: &(dyn Error + 'static),
) -> Result<()> {
    #[cfg(windows)]
    if runtime_spawn::check_pending(&context.directory, name).is_err() {
        // 未确认的启动保留原意图，普通失败事件不能关闭未知结果。
        return Ok(());
    }
    let error_type = match error.downcast_ref::<std::io::Error>() {
        Some(error) if error.kind() == std::io::ErrorKind::TimedOut => "TimeoutError",
        Some(_) => "OSError",
        None => "RuntimeError",
    };
    publish(
        context,
        json!({"event":"start-failed","role":name,"token":token,
            "error_type":error_type,"log":log_name}),
    )
}

fn observe_product(child: &mut OwnedChild, executable: &Path) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.is_exited()? {
            return Err("原角色在创建身份登记前退出，保留启动日志".into());
        }
        if let Some(actual) = identity::process(child.id())? {
            let image = Path::new(document::text(&actual["executable"], "实际产品映像")?);
            if image == executable {
                return Ok(actual);
            }
        }
        if Instant::now() >= deadline {
            return Err("本次角色进程未在期限内取得登记产品映像".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn ready(
    child: &mut OwnedChild,
    context: &Context,
    role: CloneRole,
    expected: &Value,
    check_lock: &impl Fn() -> Result<()>,
) -> Result<()> {
    let (url, endpoint) = context.endpoint(role);
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if child.is_exited()? {
            return Err("原角色在就绪前退出，保留启动日志".into());
        }
        if controller::ready(url)? {
            check_lock()?;
            if identity::process(child.id())?.as_ref() != Some(expected) {
                return Err("原角色就绪时的创建身份已变化".into());
            }
            return process::listener_at(expected, endpoint.host.parse()?, endpoint.port);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "原角色就绪超时，保留启动日志").into())
}

fn close_owned(child: &mut OwnedChild, identity: Option<&Value>) -> Result<()> {
    if child.is_exited()? {
        return Ok(());
    }
    #[cfg(windows)]
    if let Err(error) = child.kill(identity.is_some())
        && !child.is_exited()?
    {
        return Err(error);
    }
    #[cfg(not(windows))]
    if let Some(identity) = identity {
        stop_recovery_process(identity)?;
    } else if let Err(error) = child.kill()
        && !child.is_exited()?
    {
        return Err(error);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !child.is_exited()? {
        if Instant::now() >= deadline {
            return Err("本次角色子进程未在期限内确认退出，保留现场".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn publish(context: &Context, event: Value) -> Result<()> {
    runtime_history::publish(&context.directory, &context.binding.scope, event)
}
