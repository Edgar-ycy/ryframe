//! 在原恢复控制锁内按逆序收口当代 API、Worker 和前端监督树。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    bind::ownership::{Owner, control_root},
    dataset_authority::{inputs::Inputs, reads::Window},
    generation::{self, LAUNCH, ROLES, STATE, receipts},
    registration::backend_root,
    status::{Bound, optional_file, roles},
};
use crate::{
    Result,
    cli::RuntimeGenerationOptions,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{resources::common, target_verify::authority::require_closed},
        fixture_control::services::lifecycle::readonly::{
            check_restore_completion, restore_completion,
        },
        inputs::document::{self, Document},
    },
};

#[path = "stop/pending.rs"]
mod pending;
#[path = "stop/tree.rs"]
mod tree;
pub(super) use tree::{require_exited, terminate as terminate_tree};

pub(super) fn run(root: &Path, options: &RuntimeGenerationOptions) -> Result<Value> {
    let backend = backend_root(root)?;
    let window = Window::begin()?;
    let mut immutable = Inputs::new(&backend);
    let before = Bound::read(&backend, &options.control, &mut immutable)?;
    let control = control_root(&backend)?;
    Owner::with(&control, &before.operation("stop")?, |owner| {
        under_owner(&backend, options, &before, owner, &mut immutable, window)
    })
}

pub(super) fn under_owner(
    backend: &Path,
    options: &RuntimeGenerationOptions,
    bound: &Bound,
    owner: &Owner,
    immutable: &mut Inputs,
    window: Window,
) -> Result<Value> {
    let snapshot = (|| {
        owner.check()?;
        if &Bound::read(backend, &options.control, immutable)? != bound {
            return Err("取得恢复运行停止锁前原登记或目标计划发生变化".into());
        }
        Snapshot::read(backend, bound, u64::from(options.generation), immutable)
    })();
    let snapshot = window.finish_runtime_owner(owner, snapshot);
    // 结束静态读取后才发布，独立的登记、请求与 creation 句柄保持到锁内后验。
    drop(window);
    let result =
        snapshot.and_then(|snapshot| stop_locked(backend, bound, owner, immutable, snapshot));
    immutable.finish(result)
}

struct Snapshot {
    state: Document,
    commands: Value,
    observed: Value,
    launch: Option<Value>,
    runtime_identity: (u64, u64),
    generation_identity: Option<(u64, u64)>,
}

impl Snapshot {
    fn read(backend: &Path, bound: &Bound, number: u64, immutable: &mut Inputs) -> Result<Self> {
        let state_path = bound.runtime.join(STATE);
        if !optional_file(&state_path)? {
            return Err("恢复运行尚未启动，不能执行停止".into());
        }
        let state = document::read(&state_path, backend, "恢复运行生命周期")?;
        let commands = bound.validate_generation(&state.value, immutable)?;
        let creation = immutable.bound(&state.value["creation_intent"])?;
        if creation.value["request"] != state.value["generations"][0]["request"] {
            return Err("首代启动请求与原首次运行 intent 不同".into());
        }
        let generation = current(&state.value)?;
        if generation["number"].as_u64() != Some(number) {
            return Err("停止操作必须绑定当前明确运行代次".into());
        }
        bound.check_stop_authority(&generation["request"]["authority"])?;
        let directory = generation::path(&generation["directory"])?;
        let runtime_identity = directory_identity(&bound.runtime)?;
        let generation_identity = directory
            .try_exists()?
            .then(|| directory_identity(&directory))
            .transpose()?;
        let observed = roles::observe(backend, generation, immutable)?;
        for role in ROLES {
            if !observed[role]["tree"].is_null() {
                immutable.protect(&directory.join(format!("{role}-tree.json")))?;
                let product = directory.join(format!("{role}.json"));
                if optional_file(&product)? {
                    immutable.protect(&product)?;
                }
            }
        }
        let launch_path = directory.join(LAUNCH);
        let launch = if !generation["launch"].is_null() {
            let registered = immutable.bound(&generation["launch"])?;
            check_launch(generation, &registered.value)?;
            None
        } else if optional_file(&launch_path)? {
            let launch = immutable.read(&launch_path)?;
            check_launch(generation, &launch.value)?;
            Some(document::binding(&launch))
        } else {
            None
        };
        Ok(Self {
            state,
            commands,
            observed,
            launch,
            runtime_identity,
            generation_identity,
        })
    }

    fn check(&self, bound: &Bound, owner: &Owner, directory: &Path) -> Result<()> {
        owner.check()?;
        self.state.assert_unchanged()?;
        if directory_identity(&bound.runtime)? != self.runtime_identity
            || directory
                .try_exists()?
                .then(|| directory_identity(directory))
                .transpose()?
                != self.generation_identity
        {
            return Err("恢复运行根或当代目录的原身份变化，保留现场".into());
        }
        owner.check()
    }

    fn save(
        &mut self,
        backend: &Path,
        bound: &Bound,
        owner: &Owner,
        directory: &Path,
        next: &Value,
    ) -> Result<()> {
        self.check(bound, owner, directory)?;
        // 当前文件已存在，候选的纯合同检查可在原子替换前完成。
        generation::request(&json!({
            "action":"validate-state", "root":bound.runtime, "binding":bound.binding,
            "value":next, "previous":null, "commands":self.commands, "number":1,
        }))?;
        self.check(bound, owner, directory)?;
        let published = generation::request(&json!({
            "action":"save-state", "root":bound.runtime, "binding":bound.binding,
            "value":next, "previous":document::binding(&self.state), "commands":self.commands,
            "number":1,
        }))?;
        let document = document::read(&self.state.path, backend, "恢复运行发布状态")?;
        if document.value != *next || document::binding(&document) != published {
            return Err("恢复运行状态在发布后发生变化，保留原现场".into());
        }
        self.state = document;
        self.check(bound, owner, directory)
    }
}

fn check_launch(generation: &Value, launch: &Value) -> Result<()> {
    if launch["generation"] != generation["number"] || launch["request"] != generation["request"] {
        return Err("启动收据与原代次或启动请求不同".into());
    }
    Ok(())
}

fn stop_locked(
    backend: &Path,
    bound: &Bound,
    owner: &Owner,
    immutable: &Inputs,
    mut snapshot: Snapshot,
) -> Result<Value> {
    immutable.finish(Ok(()))?;
    let mut next = snapshot.state.value.clone();
    let generation = current_mut(&mut next)?;
    let directory = generation::path(&generation["directory"])?;
    snapshot.check(bound, owner, &directory)?;
    if generation["status"] == "stopped"
        && ROLES
            .iter()
            .all(|role| snapshot.observed[*role]["state"] != "running")
    {
        verify_pending(backend, &directory, generation, &snapshot.observed)?;
        snapshot.check(bound, owner, &directory)?;
        return Ok(reply("already_stopped", generation, &snapshot.observed));
    }

    if let Some(launch) = snapshot.launch.take() {
        generation["launch"] = launch;
        snapshot.save(backend, bound, owner, &directory, &next)?;
    }
    if snapshot.generation_identity.is_none() {
        validate_local_test_path(&directory, backend, LocalTestPathKind::StateDirectory)?;
        // 缺目录仅可能出现在已校验的最新 starting 代次；外部抢先创建会明确失败。
        fs::create_dir(&directory)?;
        snapshot.generation_identity = Some(directory_identity(&directory)?);
    }
    current_mut(&mut next)?["status"] = json!("stopping");
    snapshot.save(backend, bound, owner, &directory, &next)?;

    let generation = current_mut(&mut next)?;
    let failures = stop_roles(
        generation,
        &snapshot.observed,
        |generation, role, observed| {
            snapshot.check(bound, owner, &directory)?;
            generation::verify_directory(&directory, &generation["roles"])?;
            stop_role(backend, &directory, generation, role, observed, || {
                immutable.finish(Ok(()))?;
                snapshot.check(bound, owner, &directory)
            })
        },
    );
    set_outcome(generation, failures.is_empty());
    let published = snapshot.save(backend, bound, owner, &directory, &next);
    if !failures.is_empty() {
        let mut message = format!("恢复运行未能回收全部进程树：{}", failures.join("；"));
        if let Err(error) = published {
            message.push_str(&format!("；失败状态未能确认：{error}"));
        }
        return Err(message.into());
    }
    published?;
    verify_stopped(backend, bound, owner, &directory, &snapshot)
}

fn verify_stopped(
    backend: &Path,
    bound: &Bound,
    owner: &Owner,
    directory: &Path,
    snapshot: &Snapshot,
) -> Result<Value> {
    let window = Window::begin()?;
    let mut inputs = Inputs::new(backend);
    let result = (|| {
        inputs.bound(&document::binding(&snapshot.state))?;
        let generation = current(&snapshot.state.value)?;
        let observed = roles::observe(backend, generation, &mut inputs)?;
        verify_pending(backend, directory, generation, &observed)?;
        snapshot.check(bound, owner, directory)?;
        if ROLES
            .iter()
            .any(|role| observed[*role]["state"] == "running")
        {
            return Err("停止状态发布后仍有原角色在运行，保留现场".into());
        }
        Ok(reply("stopped", generation, &observed))
    })();
    window.finish_runtime_owner(owner, inputs.finish(result))
}

pub(super) fn verify_pending(
    backend: &Path,
    directory: &Path,
    generation: &Value,
    observed: &Value,
) -> Result<()> {
    let mut inputs = Inputs::new(backend);
    for role in ROLES {
        if observed[role]["state"] == "pending" {
            pending::verify(backend, directory, generation, role, &mut inputs)?;
            require_closed(document::text(
                &generation["request"]["authority"][format!("{role}_endpoint")],
                "恢复待定角色端点",
            )?)?;
        }
    }
    inputs.finish(Ok(()))
}

fn stop_roles(
    generation: &mut Value,
    observed: &Value,
    mut stop: impl FnMut(&mut Value, &str, &Value) -> Result<()>,
) -> Vec<String> {
    let mut failures = Vec::new();
    for role in ROLES.into_iter().rev() {
        if let Err(error) = stop(generation, role, &observed[role]) {
            failures.push(format!("{role}：{error}"));
        }
    }
    failures
}

pub(super) fn stop_role(
    backend: &Path,
    directory: &Path,
    generation: &mut Value,
    role: &str,
    observed: &Value,
    checkpoint: impl Fn() -> Result<()>,
) -> Result<()> {
    let mut inputs = Inputs::new(backend);
    let tree_path = directory.join(format!("{role}-tree.json"));
    let process_path = directory.join(format!("{role}.json"));
    let authority = &generation["request"]["authority"];
    let endpoint =
        document::text(&authority[format!("{role}_endpoint")], "恢复角色停止端点")?.to_owned();
    let tree = &observed["tree"];
    let has_process = optional_file(&process_path)?;
    let mut completion = None;
    if tree.is_null() {
        if optional_file(&tree_path)? || has_process {
            return Err("待启动角色的进程收据发生变化，保留现场".into());
        }
        pending::verify(backend, directory, generation, role, &mut inputs)?;
    } else {
        inputs.protect(&tree_path)?;
        if receipts::tree(directory, role, &authority["scope_id"])? != *tree {
            return Err("原恢复进程树在停止前变化，保留现场".into());
        }
        if has_process {
            inputs.protect(&process_path)?;
            // 原启动可能在树发布后、产品收据发布前被中断。
            let product = receipts::process(directory, role, &authority["scope_id"])?;
            if product.0 != tree["process"] {
                return Err("恢复进程树与原产品身份不同，保留现场".into());
            }
        }
        if observed["state"] == "running" {
            tree::terminate(directory, role, tree, || {
                checkpoint()?;
                inputs.finish(Ok(()))
            })?;
        } else if has_process {
            generation["roles"][role]["tree"] = tree.clone();
        }
        let bound = restore_completion(backend, tree)?;
        let proof = inputs.bound(&bound)?;
        check_restore_completion(tree, &proof.value)?;
        if has_process {
            generation["roles"][role]["tree"] = tree.clone();
            generation["roles"][role]["completion"] = bound;
        } else {
            completion = Some(bound);
        }
    }
    checkpoint()?;
    require_closed(&endpoint)?;
    checkpoint()?;
    inputs.finish(Ok(()))?;
    let item = &mut generation["roles"][role];
    if let Some(bound) = completion {
        item["tree"] = tree.clone();
        item["completion"] = bound;
    }
    item["state"] = json!("stopped");
    Ok(())
}

fn set_outcome(generation: &mut Value, closed: bool) {
    generation["status"] = json!(if closed { "stopped" } else { "stop_failed" });
    generation["error_type"] = if closed {
        Value::Null
    } else {
        json!("RuntimeStopError")
    };
}

fn reply(status: &str, generation: &Value, observed: &Value) -> Value {
    json!({
        "format_version":1, "kind":"restore-runtime-lifecycle-result", "operation":"stop",
        "status":status, "generation":generation["number"],
        "runtime_directory":generation["directory"], "processes":observed,
    })
}

fn current(state: &Value) -> Result<&Value> {
    state["generations"]
        .as_array()
        .and_then(|rows| rows.last())
        .ok_or_else(|| "恢复运行生命周期缺少代次".into())
}

fn current_mut(state: &mut Value) -> Result<&mut Value> {
    state["generations"]
        .as_array_mut()
        .and_then(|rows| rows.last_mut())
        .ok_or_else(|| "恢复运行生命周期缺少代次".into())
}

fn directory_identity(directory: &Path) -> Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || crate::recovery::clone::native::digest::linked(&metadata) {
        return Err("恢复运行目录必须是无链接目录".into());
    }
    common::device_inode_for(directory, &metadata)
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/stop.rs"]
mod tests;
