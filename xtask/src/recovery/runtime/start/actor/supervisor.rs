//! 监督子进程只从当代原请求启动登记的产品。
use std::{
    path::Path,
    process::{Child, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{Actor, require_identity, restrict_environment};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as strict, lifecycle::identity},
        full_stack::worker::supervise_product,
        inputs::document,
        runtime::{
            dataset_authority::{check, environment, inputs::Inputs, path},
            verify::same_path,
        },
    },
};

pub(super) fn run(actor: &Actor) -> Result<()> {
    let supervisor = identity::current()?;
    let Some(monitor) = wait_monitor(actor, &supervisor)? else {
        return Ok(());
    };
    let mut child = spawn(actor)?;
    let result = (|| {
        let tree = register(actor, &child, &supervisor, &monitor)?;
        supervise_product(&mut child, &tree)?;
        Ok(())
    })();
    if let Err(error) = result {
        return match reap(&mut child) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}；原产品句柄回收也失败：{cleanup}").into()),
        };
    }
    Ok(())
}

fn wait_monitor(actor: &Actor, supervisor: &Value) -> Result<Option<Value>> {
    let ready = actor.member_path("ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if actor.control()?.is_some() {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err("恢复成员监督器原归属登记超时".into());
        }
        if ready.try_exists()? {
            let value = document::read(&ready, &actor.backend, "恢复成员原归属")?;
            let monitor = &value.value["monitor"];
            check(
                value.value
                    == json!({"format_version":1,"operation_id":actor.operation_id,
                    "directory":actor.directory,"role":actor.role,"scope_id":actor.scope,
                    "supervisor":supervisor,"monitor":monitor})
                    && monitor["pid"] != supervisor["pid"],
                "恢复成员归属未绑定本次监督器",
            )?;
            require_identity(monitor)?;
            value.assert_unchanged()?;
            return Ok(Some(monitor.clone()));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn spawn(actor: &Actor) -> Result<Child> {
    let mut inputs = Inputs::new(&actor.backend);
    environment::verify(&mut inputs, &actor.request)?;
    let commands = crate::recovery::runtime::dataset_authority::commands::verify_direct(
        &actor.request,
        &mut inputs,
    )?;
    let arguments = strict::array(&commands[actor.role], "恢复产品原命令")?;
    let program = path(&arguments[0])?;
    inputs.protect(&program)?;
    let expected = if actor.role == "frontend" {
        &actor.request["tools"]["frontend_server"]
    } else {
        &actor.request["artifacts"][actor.role]
    };
    let file = digest::file(&program)?;
    check(
        expected["bytes"] == file.bytes && expected["sha256"] == file.sha256,
        "恢复产品原可执行文件摘要发生变化",
    )?;
    let cwd = path(
        &actor.request["roots"][if actor.role == "frontend" {
            "frontend"
        } else {
            "backend_execution"
        }],
    )?;
    let log_file = actor.log()?;
    let mut command = crate::process::child_command(&program);
    for argument in &arguments[1..] {
        command.arg(document::text(argument, "恢复产品命令参数")?);
    }
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file);
    restrict_environment(&mut command, actor.role != "frontend");
    environment::configure(
        &mut inputs,
        &actor.request,
        &actor.directory,
        actor.role,
        &mut command,
    )?;
    actor.verify_start()?;
    check(actor.control()?.is_none(), "恢复产品在启动前已被取消")?;
    inputs.finish(Ok(()))?;
    command.spawn().map_err(Into::into)
}

fn register(actor: &Actor, child: &Child, supervisor: &Value, monitor: &Value) -> Result<Value> {
    let product = identity::process(child.id())?.ok_or("恢复产品在登记前已经退出")?;
    let declared = if actor.role == "frontend" {
        &actor.request["tools"]["frontend_server"]["path"]
    } else {
        &actor.request["artifacts"][actor.role]["executable"]
    };
    check(
        same_path(&path(&product["executable"])?, &path(declared)?),
        "恢复产品实际可执行文件与原启动命令不同",
    )?;
    require_identity(monitor)?;
    let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":actor.directory,
        "role":actor.role,"scope_id":actor.scope,"operation_id":actor.operation_id,
        "supervisor":supervisor,"process":product,"monitor":monitor,"group_id":supervisor["pid"]});
    actor.verify_start()?;
    publish(
        actor,
        &actor.directory.join(format!("{}-tree.json", actor.role)),
        &tree,
    )?;
    require_identity(&product)?;
    actor.verify_start()?;
    publish(
        actor,
        &actor.directory.join(format!("{}.json", actor.role)),
        &json!({"format_version":1,"role":actor.role,"scope_id":actor.scope,"identity":product}),
    )?;
    Ok(tree)
}

fn publish(actor: &Actor, path: &Path, value: &Value) -> Result<()> {
    actor.publish_receipt(path, value)?;
    check(
        strict::read(path)? == *value,
        "恢复监督原收据发布期间发生变化",
    )
}

fn reap(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    Ok(())
}
