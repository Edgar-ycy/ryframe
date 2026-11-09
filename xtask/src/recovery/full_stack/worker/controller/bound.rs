//! 同进程复制交接使用冻结环境及原已成功的角色结果。
use super::super::{
    documents,
    environment::Environment,
    operation::{self, Operation},
    role::Role,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::resources::common,
};
use serde_json::{Value, json};
use std::{ffi::OsString, fs, path::Path, time::Instant};

/// 退场先确认原 API 树和进程收据相同，关闭阶段无需 API 仍就绪。
pub(crate) fn require_api_tree(directory: &Path, runtime: &Value) -> Result<()> {
    super::tree::read(directory, runtime, Role::Api, true).map(|_| ())
}

pub(crate) fn control_bound(
    role: Role,
    action: &str,
    backend: &Path,
    directory: &Path,
    timeout: f64,
    file: &Value,
) -> Result<Value> {
    let started = Instant::now();
    if action != "stop" {
        return Err("绑定交接控制只允许一次明确停止；未知原操作必须显式核对，不重放".into());
    }
    let environment = Environment::capture(file)?;
    super::control_verified(
        role,
        action,
        backend,
        directory,
        timeout,
        started,
        Some(&super::BoundControl {
            environment: &environment,
            stop: None,
        }),
    )
}

pub(crate) struct StopGeneration<'a> {
    pub(crate) start_id: &'a str,
    pub(crate) identity: &'a Value,
}

impl StopGeneration<'_> {
    pub(super) fn prove_closed(
        &self,
        role: Role,
        directory: &Path,
        runtime: &Value,
        tree: &Value,
    ) -> Result<()> {
        if tree["operation_id"] != self.start_id || tree["process"] != *self.identity {
            return Err("原停止结果不同于已预留的启动代次和进程身份".into());
        }
        super::tree::closure(tree, std::time::Duration::ZERO, false)?;
        crate::recovery::clone::native::target_verify::authority::require_closed(
            runtime[role.ready()].as_str().ok_or("原角色端点无效")?,
        )?;
        if super::tree::read(directory, runtime, role, true)?.as_ref() != Some(tree) {
            return Err("原停止进程树在闭口核验期间变化".into());
        }
        Ok(())
    }
}

pub(crate) struct StopTarget<'a> {
    pub(crate) generation: StopGeneration<'a>,
    pub(crate) record: &'a dyn Fn(&str) -> Result<()>,
    pub(crate) publish: &'a dyn Fn() -> Result<()>,
}

impl StopTarget<'_> {
    pub(super) fn check(&self, role: Role, directory: &Path, runtime: &Value) -> Result<()> {
        let generation = &self.generation;
        let tree = super::tree::running(directory, runtime, role, generation.identity)?;
        if tree["operation_id"] != generation.start_id
            || super::process::registered(role, directory, runtime)?.as_ref()
                != Some(generation.identity)
        {
            return Err("停止目标不同于原启动代次和实际进程身份".into());
        }
        Ok(())
    }
}

pub(crate) fn stop_bound_recorded(
    role: Role,
    backend: &Path,
    directory: &Path,
    timeout: f64,
    file: &Value,
    stop: StopTarget<'_>,
) -> Result<Value> {
    let environment = Environment::capture(file)?;
    super::control_verified(
        role,
        "stop",
        backend,
        directory,
        timeout,
        Instant::now(),
        Some(&super::BoundControl {
            environment: &environment,
            stop: Some(stop),
        }),
    )
}

pub(crate) fn observe_bound(
    role: Role,
    backend: &Path,
    directory: &Path,
    file: &Value,
) -> Result<Value> {
    let environment = Environment::capture(file)?;
    if matches!(role, Role::Api | Role::Worker) {
        require_no_pending(role, directory)?;
    }
    let result = super::observe(role, backend, directory);
    if matches!(role, Role::Api | Role::Worker) {
        require_no_pending(role, directory)?;
    }
    environment.check()?;
    result
}

/// 核对运行角色身份和完整树，按需检查就绪和监听；停止态始终核对闭口。
pub(crate) fn observe_bound_state(
    role: Role,
    backend: &Path,
    directory: &Path,
    file: &Value,
    probe_ready: bool,
) -> Result<Value> {
    if !matches!(role, Role::Api | Role::Worker) {
        return Err("绑定角色观察只接受 API 或 Worker".into());
    }
    let environment = Environment::capture(file)?;
    require_no_pending(role, directory)?;
    let readiness = if probe_ready {
        super::observation::Readiness::Probe
    } else {
        super::observation::Readiness::Skip
    };
    let result = super::observation::observe_role(role, backend, directory, readiness);
    require_no_pending(role, directory)?;
    environment.check()?;
    let (identity, ready) = result?;
    Ok(
        json!({"state":if identity.is_some(){"running"}else{"stopped"},"identity":identity,"ready":ready}),
    )
}

/// 旧 API 交接使用了 Worker 的控制锁名称。仅允许其已绑定外层 owner 下的 Worker 停止观察。
pub(crate) fn observe_worker_under_outer_lock(
    backend: &Path,
    directory: &Path,
    file: &Value,
    expected_outer_owner: &Value,
) -> Result<Value> {
    let environment = Environment::capture(file)?;
    let original = outer_lock(directory, expected_outer_owner)?;
    let observed = super::observe(Role::Worker, backend, directory);
    if outer_lock(directory, expected_outer_owner)? != original {
        return Err("API 交接外层锁在 Worker 停止观察期间变化".into());
    }
    environment.check()?;
    let observed = observed?;
    if observed["state"] != "stopped" || !observed["identity"].is_null() {
        return Err("API 交接外层锁仅允许观察 Worker 完整停止状态".into());
    }
    Ok(observed)
}

#[derive(Debug, PartialEq, Eq)]
struct OuterLock {
    runtime: (u64, u64),
    directory: (u64, u64),
    owner: (u64, u64),
    binding: Value,
}

fn outer_lock(directory: &Path, expected: &Value) -> Result<OuterLock> {
    let lock = directory.join(Role::Worker.lock());
    if operation::canonical(&lock)? != lock {
        return Err("Worker 外层锁必须为原运行目录下的规范目录".into());
    }
    let lock_identity = common::device_inode_for(&lock, &fs::symlink_metadata(&lock)?)?;
    let path = lock.join("owner.json");
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    let owner_identity = common::device_inode_for(&path, &fs::symlink_metadata(&path)?)?;
    let binding = operation::file(&path)?;
    let owner = operation::read(&path)?;
    crate::recovery::clone::native::json::exact(
        &owner,
        &[
            "format_version",
            "kind",
            "identity",
            "runtime_directory",
            "operation",
            "token",
        ],
        "Worker 外层锁 owner",
    )?;
    let names = fs::read_dir(&lock)?
        .map(|entry| entry.map(|value| value.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let recorded = Path::new(
        owner["runtime_directory"]
            .as_str()
            .ok_or("Worker 外层锁缺少已登记运行目录")?,
    );
    let runtime = common::device_inode_for(directory, &fs::symlink_metadata(directory)?)?;
    if names != [OsString::from("owner.json")]
        || owner != *expected
        || owner["format_version"].as_u64() != Some(1)
        || owner["kind"] != "devex-clone-runtime-lock"
        || operation::canonical(recorded)? != operation::canonical(directory)?
        || common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)? != runtime
        || !matches!(owner["operation"].as_str(), Some("start" | "stop"))
        || !documents::hex(&owner["token"], 32)
        || common::device_inode_for(&lock, &fs::symlink_metadata(&lock)?)? != lock_identity
        || common::device_inode_for(&path, &fs::symlink_metadata(&path)?)? != owner_identity
        || operation::file(&path)? != binding
    {
        return Err("Worker 外层锁缺少原 owner、封闭成员或原目录身份".into());
    }
    common::validate_process_identity(&owner["identity"])?;
    Ok(OuterLock {
        runtime,
        directory: lock_identity,
        owner: owner_identity,
        binding,
    })
}

fn pending(role: Role, directory: &Path) -> Result<bool> {
    match fs::symlink_metadata(directory.join(role.lock())) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
        Ok(_) => Ok(true),
    }
}

pub(super) fn require_no_pending(role: Role, directory: &Path) -> Result<()> {
    if pending(role, directory)? {
        return Err("角色尚有未完成控制证据；保留原操作并显式核对，不追加停止".into());
    }
    Ok(())
}

/// 显式回收只消费原已成功结果；保留未知操作，不补发 start/stop。
pub(crate) fn reconcile_completed_bound(
    role: Role,
    backend: &Path,
    directory: &Path,
    file: &Value,
    original_controller: &Value,
    required_operation: &str,
    reserved_id: Option<&str>,
) -> Result<Option<Value>> {
    reconcile_completed(
        role,
        backend,
        directory,
        file,
        Completion {
            executor: original_controller,
            operation: required_operation,
            id: reserved_id,
            stop: None,
        },
    )
}

pub(crate) fn reconcile_recorded_stop(
    role: Role,
    backend: &Path,
    directory: &Path,
    file: &Value,
    stop: &super::ArchivedStop<'_>,
) -> Result<Option<Value>> {
    reconcile_completed(
        role,
        backend,
        directory,
        file,
        Completion {
            executor: stop.executor,
            operation: "stop",
            id: Some(stop.id),
            stop: Some(&stop.generation),
        },
    )
}

struct Completion<'a> {
    executor: &'a Value,
    operation: &'a str,
    id: Option<&'a str>,
    stop: Option<&'a StopGeneration<'a>>,
}

fn reconcile_completed(
    role: Role,
    backend: &Path,
    directory: &Path,
    file: &Value,
    completion: Completion<'_>,
) -> Result<Option<Value>> {
    if !matches!(role, Role::Api | Role::Worker) {
        return Err("已完成角色核对只接受明确 API 或 Worker".into());
    }
    if !pending(role, directory)? {
        return Ok(None);
    }
    let environment = Environment::capture(file)?;
    let guard = super::guard(role, directory, std::time::Duration::ZERO)?;
    if !pending(role, directory)? {
        return Ok(None);
    }
    let runtime = super::super::runtime::load(role, backend, directory)?;
    let source = operation::source(directory)?;
    let op = Operation::read(role, directory, &source)?;
    check_original(
        &op,
        backend,
        &runtime,
        completion.executor,
        completion.operation,
        completion.id,
    )?;
    let pid = u32::try_from(
        completion.executor["pid"]
            .as_u64()
            .ok_or("原外层控制器PID无效")?,
    )?;
    if common::process_identity(pid)?.as_ref() == Some(completion.executor) {
        return Err("控制器仍在运行，禁止释放运行控制锁".into());
    }
    let running = completion.operation == "start";
    if running && op.request.get("environment_binding") != Some(environment.binding()) {
        return Err("原角色启动环境与冻结交接不同，保留原结果与两层锁".into());
    }
    let path = op.path.join("result.json");
    let binding = operation::file(&path)?;
    let result = op.validate("result", operation::read(&path)?)?;
    let expected_state = if running { "running" } else { "stopped" };
    if result["outcome"] != "succeeded" || result["state"] != expected_state {
        return Err("角色结果仍未知或失败；仅保留原生证据，不补发start或stop".into());
    }
    let observe = || {
        observe_completed(
            role,
            backend,
            directory,
            &runtime,
            &result,
            running,
            completion.stop,
        )
    };
    let before = observe()?;
    if let Some(tree) = &before
        && running
    {
        require_start_tree(&op, tree)?;
    }
    environment.check()?;
    guard.check()?;
    if operation::file(&path)? != binding || operation::source(directory)? != source {
        return Err("原角色结果或冻结来源在归档前变化，保留两层锁".into());
    }
    let archived = op.archive()?;
    let archived_path = directory
        .join(role.archive())
        .join(op.id()?)
        .join("result.json");
    if archived != result
        || operation::file(&archived_path)? != binding
        || operation::source(directory)? != source
        || observe()? != before
    {
        return Err("已成功角色结果归档后完整树或原结果变化，保留归档与原外层锁".into());
    }
    guard.check()?;
    environment.check()?;
    Ok(Some(archived))
}

fn check_original(
    op: &Operation,
    backend: &Path,
    runtime: &Value,
    original_controller: &Value,
    required_operation: &str,
    reserved_id: Option<&str>,
) -> Result<()> {
    if op.owner["identity"] != *original_controller
        || op.request["backend_root"] != json!(backend)
        || op.request["scope_id"] != runtime["scope_id"]
        || !matches!(required_operation, "start" | "stop")
        || op.request["operation"] != required_operation
        || (op.role == Role::Worker && required_operation == "start" && reserved_id.is_none())
        || reserved_id.is_some_and(|id| op.owner["operation_id"] != id)
    {
        return Err("角色已完成结果不属于原外层控制器、明确操作或预留代次，保留两层锁".into());
    }
    Ok(())
}

fn observe_completed(
    role: Role,
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    result: &Value,
    running: bool,
    stop: Option<&StopGeneration<'_>>,
) -> Result<Option<Value>> {
    let tree = super::tree::read(directory, runtime, role, running)?;
    if let Some(stop) = stop {
        stop.prove_closed(
            role,
            directory,
            runtime,
            tree.as_ref().ok_or("原停止缺少进程树")?,
        )?;
    }
    let observed = super::observe(role, backend, directory)?;
    if observed["state"] != result["state"]
        || observed["identity"] != result["identity"]
        || super::tree::read(directory, runtime, role, running)? != tree
    {
        return Err("原已成功角色结果与当前完整树不同，保留原结果与两层锁".into());
    }
    Ok(tree)
}

fn require_start_tree(op: &Operation, tree: &Value) -> Result<()> {
    let id = op.id()?;
    let supervisor = op.child("supervisor.json")?;
    let ready = operation::read(&super::tree::path(op.directory(), op.role, id, "ready"))?;
    if tree["operation_id"] != id
        || tree["supervisor"] != supervisor["identity"]
        || ready
            != json!({"format_version":1,"operation_id":id,"directory":op.directory(),
            "role":op.role.name(),"scope_id":op.request["scope_id"],"supervisor":tree["supervisor"],
            "monitor":tree["monitor"]})
    {
        return Err("原已成功启动与当前监督树代次或成员不同，保留两层锁".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary() -> Result<(tempfile::TempDir, std::path::PathBuf)> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let directory = tempfile::tempdir_in(local)?;
        let canonical = operation::canonical(directory.path())?;
        Ok((directory, canonical))
    }

    #[test]
    fn no_role_pending_keeps_bound_reconciliation_read_only() -> Result<()> {
        let (_temporary, directory) = temporary()?;
        for role in [Role::Api, Role::Worker] {
            assert_eq!(
                reconcile_completed_bound(
                    role,
                    &directory,
                    &directory,
                    &Value::Null,
                    &Value::Null,
                    "start",
                    None
                )?,
                None,
            );
            assert!(!pending(role, &directory)?);
        }
        assert!(
            reconcile_completed_bound(
                Role::Frontend,
                &directory,
                &directory,
                &Value::Null,
                &Value::Null,
                "start",
                None
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn worker_outer_observation_requires_exact_lock_members_and_raw_owner() -> Result<()> {
        let (_temporary, directory) = temporary()?;
        let lock = directory.join(Role::Worker.lock());
        fs::create_dir(&lock)?;
        let path = lock.join("owner.json");
        let identity = super::super::super::process::current()?;
        let owner = json!({"format_version":1,"kind":"devex-clone-runtime-lock","identity":identity,
            "runtime_directory":directory,"operation":"stop","token":"a".repeat(32)});
        operation::write_new(&path, &owner)?;
        let first = outer_lock(&directory, &owner)?;
        assert_eq!(outer_lock(&directory, &owner)?, first);
        assert!(require_no_pending(Role::Worker, &directory).is_err());
        assert!(require_no_pending(Role::Api, &directory).is_ok());
        let file = directory.join("environment.json");
        operation::write_new(&file, &json!({"environment":{"APP_ENV":"test"}}))?;
        let environment = common::binding(&file)?;
        let default =
            observe_bound(Role::Worker, &directory, &directory, &environment).unwrap_err();
        assert!(default.to_string().contains("角色尚有未完成控制证据"));

        fs::write(&path, serde_json::to_vec_pretty(&owner)?)?;
        assert_ne!(outer_lock(&directory, &owner)?, first);
        let extra = lock.join("request.json");
        operation::write_new(&extra, &json!({}))?;
        assert!(outer_lock(&directory, &owner).is_err());
        assert!(
            observe_worker_under_outer_lock(&directory, &directory, &environment, &owner).is_err()
        );
        fs::remove_file(extra)?;
        let mut role_owner = owner;
        role_owner["kind"] = json!(Role::Worker.kind("owner"));
        fs::write(&path, serde_json::to_vec(&role_owner)?)?;
        assert!(outer_lock(&directory, &role_owner).is_err());
        Ok(())
    }

    #[test]
    fn worker_start_binds_the_explicit_reserved_id_and_authorized_tree() -> Result<()> {
        let (_temporary, directory) = temporary()?;
        let runtime = json!({"scope_id":"worker-reconcile","worker_ready_url":"http://127.0.0.1:19210/readyz",
            "artifacts":{"worker":{"path":std::env::current_exe()?,"sha256":"d".repeat(64)}}});
        operation::write_new(&directory.join("runtime.json"), &runtime)?;
        let role = Role::Worker;
        let op = Operation::claim(role, "start", &directory, &directory, &runtime, 1.0)?;
        let identity = &op.owner["identity"];
        let id = op.id()?;
        check_original(&op, &directory, &runtime, identity, "start", Some(id))?;
        assert!(check_original(&op, &directory, &runtime, identity, "start", None).is_err());
        assert!(
            check_original(&op, &directory, &runtime, identity, "start", Some("other")).is_err()
        );
        assert!(check_original(&op, &directory, &runtime, identity, "stop", Some(id)).is_err());
        assert!(
            check_original(&op, &directory, &runtime, &Value::Null, "start", Some(id)).is_err()
        );

        let mut candidate = op.bound(&role.kind("candidate"))?;
        candidate["identity"] = identity.clone();
        candidate["created_at_ns"] = json!(1);
        op.publish("candidate.json", &candidate, false)?;
        let mut supervisor = op.bound(&role.kind("supervisor"))?;
        supervisor["identity"] = identity.clone();
        supervisor["candidate_sha256"] = json!(documents::hash(&candidate)?);
        supervisor["authorized_at_ns"] = json!(2);
        op.publish("supervisor.json", &supervisor, false)?;
        let mut monitor = identity.clone();
        monitor["pid"] = json!(identity["pid"].as_u64().ok_or("测试控制器缺少 PID")? + 1);
        let ready = json!({"format_version":1,"operation_id":id,"directory":directory,
            "role":role.name(),"scope_id":runtime["scope_id"],"supervisor":identity,"monitor":monitor});
        operation::write_new(
            &super::super::super::tree::path(&directory, role, id, "ready"),
            &ready,
        )?;
        let tree = json!({"operation_id":id,"supervisor":identity,"monitor":monitor});
        require_start_tree(&op, &tree)?;
        let mut changed = tree;
        changed["operation_id"] = json!("0".repeat(32));
        assert!(require_start_tree(&op, &changed).is_err());
        assert!(pending(role, &directory)?);
        Ok(())
    }
}
