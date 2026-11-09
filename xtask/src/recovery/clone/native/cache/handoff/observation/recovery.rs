//! 只回收已退出原 owner 的外层锁；不向角色控制器发送 start/stop/reconcile。
use super::Frame;
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document, lifecycle::lock::ProcessGuard, resources::common,
    },
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

pub(super) fn run(frame: &Frame) -> Result<Value> {
    let guard = ProcessGuard::acquire_named(&frame.post.runtime, "runtime-control.guard")?;
    let original = Original::load(&frame.backend, &frame.post.runtime)?;
    let operation = document::text(&original.owner["operation"], "原 API 操作")?;
    if frame.retirement.is_some() && operation != "stop" {
        return Err("seed 原 API 退场只回收原停止控制锁".into());
    }
    frame.check()?;
    let _ = crate::recovery::full_stack::worker::controller::reconcile_completed_bound(
        crate::recovery::full_stack::worker::role::Role::Api,
        &frame.product,
        &frame.post.runtime,
        &frame.environment,
        &original.owner["identity"],
        operation,
        None,
    )?;
    // 实际完整树可能仍运行；恢复外层控制器锁不代表角色停止或远端写入成功。
    frame.observed()?;
    guard.check()?;
    let actual = original.check(&frame.backend)?;
    frame.check()?;
    let value = json!({"format_version":1,"kind":"devex-clone-runtime-lock-reconciliation",
        "owner":original.owner,"observed_identity":actual,"remote_writes_reconciled":false});
    let receipt = frame.post.runtime.join(format!(
        "controller-reconcile-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    document::write_new(&receipt, &value)?;
    // 发布后的漂移同样拒绝；保留新恢复证据和原锁，不自动删除或重试。
    frame.check()?;
    guard.check()?;
    frame.observed()?;
    if original.check(&frame.backend)? != actual {
        return Err("外层锁owner进程观察在恢复发布期间变化，保留现场".into());
    }
    original.remove(&frame.backend)?;
    guard.check()?;
    Ok(value)
}

struct Original {
    path: PathBuf,
    inode: (u64, u64),
    runtime_inode: (u64, u64),
    binding: Value,
    owner: Value,
}

impl Original {
    fn load(backend: &Path, directory: &Path) -> Result<Self> {
        common::local_directory(backend, directory, "原API运行目录")?;
        let path = directory.join("worker-control.lock");
        common::local_directory(backend, &path, "原API外层控制锁")?;
        let inode = common::device_inode_for(&path, &fs::symlink_metadata(&path)?)?;
        let binding = common::binding(&common::common_file(
            backend,
            &path.join("owner.json"),
            "原控制器owner",
        )?)?;
        let owner = common::read_bound(backend, &binding, "原API外层控制器")?;
        document::exact(
            &owner,
            &[
                "format_version",
                "kind",
                "identity",
                "runtime_directory",
                "operation",
                "token",
            ],
            "原API外层控制器",
        )?;
        common::validate_process_identity(&owner["identity"])?;
        if owner["format_version"].as_u64() != Some(1)
            || owner["kind"] != "devex-clone-runtime-lock"
            || !owner["operation"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
            || !owner["token"].as_str().is_some_and(|value| {
                value.len() == 32
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err("原外层控制器owner形状或登记运行目录不符，不能回收".into());
        }
        let runtime_inode = same_runtime(backend, &owner["runtime_directory"], directory)?;
        let original = Self {
            path,
            inode,
            runtime_inode,
            binding,
            owner,
        };
        original.check(backend)?;
        Ok(original)
    }

    fn check(&self, backend: &Path) -> Result<Option<Value>> {
        common::local_directory(backend, &self.path, "原外层控制锁")?;
        let metadata = fs::symlink_metadata(&self.path)?;
        let names = fs::read_dir(&self.path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        if digest::linked(&metadata)
            || common::device_inode_for(&self.path, &metadata)? != self.inode
            || names != [OsString::from("owner.json")]
            || common::read_bound(backend, &self.binding, "原owner前后绑定")? != self.owner
        {
            return Err("API外层控制锁目录身份、原owner或文件集合变化，保留现场".into());
        }
        if same_runtime(
            backend,
            &self.owner["runtime_directory"],
            self.path.parent().ok_or("原外层控制锁缺少运行父目录")?,
        )? != self.runtime_inode
        {
            return Err("原外层控制器登记运行目录身份在核对期间变化".into());
        }
        let pid = u32::try_from(
            self.owner["identity"]["pid"]
                .as_u64()
                .ok_or("原控制器PID无效")?,
        )?;
        let actual = common::process_identity(pid)?;
        if actual.as_ref() == Some(&self.owner["identity"]) {
            return Err("控制器仍在运行，禁止释放运行控制锁".into());
        }
        Ok(actual)
    }

    fn remove(self, backend: &Path) -> Result<()> {
        self.check(backend)?;
        fs::remove_file(self.path.join("owner.json"))?;
        fs::remove_dir(&self.path)?;
        Ok(())
    }
}

/// 两个输入均先验证受管、现有且无链接；canonical只用于位置比较，不写回原owner。
pub(super) fn same_runtime(
    backend: &Path,
    recorded: &Value,
    selected: &Path,
) -> Result<(u64, u64)> {
    let recorded = Path::new(
        recorded
            .as_str()
            .ok_or("原外层控制器owner运行目录必须是文本绝对路径")?,
    );
    common::local_directory(backend, recorded, "原owner登记运行目录")?;
    common::local_directory(backend, selected, "当前API运行目录")?;
    let recorded_path = fs::canonicalize(recorded)?;
    let selected_path = fs::canonicalize(selected)?;
    let recorded_inode = common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)?;
    let selected_inode = common::device_inode_for(selected, &fs::symlink_metadata(selected)?)?;
    if recorded_path != selected_path || recorded_inode != selected_inode {
        return Err("原外层控制器登记运行目录不是同一受管真实目录".into());
    }
    common::local_directory(backend, recorded, "原owner登记运行目录后像")?;
    common::local_directory(backend, selected, "当前API运行目录后像")?;
    if common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)? != selected_inode
        || common::device_inode_for(selected, &fs::symlink_metadata(selected)?)? != selected_inode
    {
        return Err("原外层控制器登记运行目录身份在核对期间变化".into());
    }
    Ok(selected_inode)
}
