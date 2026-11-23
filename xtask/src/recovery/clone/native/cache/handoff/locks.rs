//! 交接沿用原存储运行与运行控制锁；不通过跨进程编号授予目标权限。
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document,
        lifecycle::{
            identity,
            lock::{ProcessGuard, RunLock},
        },
        resources::common,
        target_verify,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Storage {
    backend: PathBuf,
    pub directory: PathBuf,
    state: Value,
    identity: (u64, u64),
    guard: Option<ProcessGuard>,
    lock: Option<RunLock>,
}

impl Storage {
    pub(crate) fn acquire(backend: &Path, current: &Path, manifest: &Value) -> Result<Self> {
        let directory = resolve(backend, current, manifest)?;
        Self::claim(backend, current, directory)
    }

    pub(in crate::recovery) fn acquire_independent_target(
        backend: &Path,
        current: &Path,
        registration: &Value,
        storage: &Value,
    ) -> Result<Self> {
        let directory = registered_target(backend, registration, storage)?;
        let physical_target = fs::canonicalize(&directory)?;
        let physical_current = fs::canonicalize(current)?;
        if physical_target.starts_with(&physical_current)
            || physical_current.starts_with(&physical_target)
        {
            return Err("独立 ARM 必须使用互不重叠的目标存储运行".into());
        }
        Self::claim(backend, current, directory)
    }

    fn claim(backend: &Path, current: &Path, directory: PathBuf) -> Result<Self> {
        if directory != current
            && (directory.starts_with(current) || current.starts_with(&directory))
        {
            return Err("当前运行与目标存储运行不能互相包含".into());
        }
        let identity = directory_identity(backend, &directory)?;
        let state = common::binding(&directory.join("state.json"))?;
        let (guard, lock) = if directory == current {
            let owner = document::read(&current.join("run.lock/owner.json"))?;
            if owner["identity"] != identity::current()?
                || owner["directory"] != json!(current)
                || owner["manifest_sha256"]
                    != common::binding(&current.join("manifest.json"))?["sha256"]
            {
                return Err("API交接没有持有当前目标存储的固定运行锁".into());
            }
            (None, None)
        } else {
            let guard = ProcessGuard::acquire(&directory)?;
            let lock = RunLock::claim(&directory)?;
            (Some(guard), Some(lock))
        };
        let result = Self {
            backend: backend.to_owned(),
            directory,
            state,
            identity,
            guard,
            lock,
        };
        let checked = result.check();
        if let Err(error) = checked {
            return result.finish(Err(error));
        }
        Ok(result)
    }

    pub(crate) fn check(&self) -> Result<()> {
        if directory_identity(&self.backend, &self.directory)? != self.identity
            || common::binding(&self.directory.join("state.json"))? != self.state
        {
            return Err("API交接期间目标存储目录身份或完整账本变化".into());
        }
        if let Some(guard) = &self.guard {
            guard.check()?;
        }
        if let Some(lock) = &self.lock {
            lock.verify()?;
        }
        Ok(())
    }

    pub(crate) fn finish<T>(self, result: Result<T>) -> Result<T> {
        let result = combine(result, self.check());
        let result = if let Some(lock) = &self.lock {
            combine(result, lock.release())
        } else {
            result
        };
        if let Some(guard) = &self.guard {
            combine(result, guard.check())
        } else {
            result
        }
    }
}

pub(super) fn resolve(backend: &Path, current: &Path, manifest: &Value) -> Result<PathBuf> {
    if manifest["copy_stage"] == "seed_to_arm" {
        return registered_target(
            backend,
            &manifest["target_registration"],
            &manifest["target_storage_run"],
        );
    }
    let initialized = common::bound_file(backend, &manifest["initialized"], "API交接初始化")?;
    let workspace = initialized
        .parent()
        .and_then(Path::parent)
        .ok_or("初始化工作区缺失")?;
    let path = workspace.join("registration.json");
    if !path.try_exists()? {
        return Ok(current.to_owned());
    }
    let registration = common::read_bound(backend, &common::binding(&path)?, "目标初始化登记")?;
    let descriptor = &registration["storage_run"];
    if descriptor.is_null() || descriptor.as_object().is_some_and(|value| value.is_empty()) {
        return Ok(current.to_owned());
    }
    let run = target_verify::fresh::storage_run(backend, descriptor, None)?;
    if document::read(&run.join("manifest.json"))?["kind"] != "reference-fixture-service-run" {
        return Ok(current.to_owned());
    }
    target_verify::published::unlocked_history(backend, &initialized)?;
    Ok(run)
}

pub(in crate::recovery) fn registered_target(
    backend: &Path,
    registration: &Value,
    storage: &Value,
) -> Result<PathBuf> {
    let registered = common::read_bound(backend, registration, "目标fresh登记")?;
    let run = target_verify::fresh::storage_run(backend, &registered["storage_run"], None)?;
    if storage["path"] != json!(run) || storage["manifest"] != registered["storage_run"]["manifest"]
    {
        return Err("API交接外部目标存储不属于原fresh登记".into());
    }
    Ok(run)
}

pub(super) struct Runtime {
    directory: PathBuf,
    path: PathBuf,
    identity: (u64, u64),
    owner: Value,
    guard: ProcessGuard,
}

impl Runtime {
    pub(super) fn acquire(backend: &Path, directory: &Path) -> Result<Self> {
        Self::acquire_for(backend, directory, "start")
    }

    pub(super) fn acquire_for(backend: &Path, directory: &Path, operation: &str) -> Result<Self> {
        if !matches!(operation, "start" | "stop") {
            return Err("交接运行控制操作无效".into());
        }
        common::local_directory(backend, directory, "API运行目录")?;
        let guard = ProcessGuard::acquire_named(directory, "runtime-control.guard")?;
        let path = directory.join("worker-control.lock");
        super::guard::absent(&path)?;
        fs::create_dir(&path)?;
        let identity = directory_identity(backend, &path)?;
        let owner = json!({"format_version":1,"kind":"devex-clone-runtime-lock",
            "identity":identity::current()?,"runtime_directory":directory,"operation":operation,
            "token":uuid::Uuid::new_v4().simple().to_string()});
        document::write_new(&path.join("owner.json"), &owner)?;
        let lock = Self {
            directory: directory.to_owned(),
            path,
            identity,
            owner,
            guard,
        };
        lock.check(backend)?;
        Ok(lock)
    }

    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        self.guard.check()?;
        let names = fs::read_dir(&self.path)?
            .map(|entry| entry.map(|value| value.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        if directory_identity(backend, &self.path)? != self.identity
            || names != [std::ffi::OsString::from("owner.json")]
            || document::read(&self.path.join("owner.json"))? != self.owner
            || self.owner["identity"] != identity::current()?
            || self.owner["runtime_directory"] != json!(self.directory)
        {
            return Err("API交接运行控制锁的原owner或目录身份变化".into());
        }
        Ok(())
    }

    pub(super) fn finish<T>(self, backend: &Path, result: Result<T>) -> Result<T> {
        let cleanup = (|| {
            self.check(backend)?;
            fs::remove_file(self.path.join("owner.json"))?;
            fs::remove_dir(&self.path)?;
            self.guard.check()
        })();
        combine(result, cleanup)
    }
}

/// 双角色共用的内核互斥在角色运行目录之外。prepare 只发布文件，
/// 异常退出由已登记 run owner 收口，不在角色目录制造新的持久锁。
pub(in crate::recovery::clone::native) fn seed_runtime_guard(
    backend: &Path,
    run: &Path,
) -> Result<ProcessGuard> {
    let parent = run.join("seed-runtime");
    common::local_directory(backend, &parent, "seed 双角色控制目录")?;
    ProcessGuard::acquire_named(&parent, "runtime-control.guard")
}

fn directory_identity(backend: &Path, path: &Path) -> Result<(u64, u64)> {
    common::local_directory(backend, path, "API交接控制目录")?;
    let metadata = fs::symlink_metadata(path)?;
    if digest::linked(&metadata) {
        return Err("API交接控制目录不能是链接".into());
    }
    common::device_inode_for(path, &metadata)
}

pub(super) fn combine<T>(result: Result<T>, checked: Result<()>) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => Err(super::diagnostic::failure(
            super::diagnostic::Phase::Cleanup,
            primary,
            Some(secondary),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_runtime_control_uses_a_distinct_path_from_worker_role_control() {
        let backend = crate::workspace::root_dir();
        let root = backend.join(".local-tests");
        fs::create_dir_all(&root).unwrap();
        let run = tempfile::Builder::new()
            .prefix("seed-pair-lock-")
            .tempdir_in(root)
            .unwrap();
        let pair = run.path().join("seed-runtime");
        let runtime = pair.join("runtime");
        let worker = runtime.join("worker-control.lock");
        fs::create_dir_all(&worker).unwrap();
        let lock = seed_runtime_guard(&backend, run.path()).unwrap();
        assert!(pair.join("runtime-control.guard").is_file());
        assert!(seed_runtime_guard(&backend, run.path()).is_err());
        lock.check().unwrap();
        drop(lock);
        assert!(seed_runtime_guard(&backend, run.path()).is_ok());
        assert!(worker.is_dir());
    }
}
