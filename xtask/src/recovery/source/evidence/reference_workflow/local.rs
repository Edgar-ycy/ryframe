use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{files, inputs, model, native, path, protocol, receipts};
use crate::Result;

pub(super) struct Lock {
    path: PathBuf,
    identity: (u64, u64),
    release_on_drop: bool,
}

impl Lock {
    pub(super) fn claim(work: &Path) -> Result<Self> {
        let path = work.join(".reference-lock");
        fs::create_dir(&path)?;
        let identity = Self::identity(&path)?;
        Ok(Self {
            path,
            identity,
            release_on_drop: true,
        })
    }

    pub(super) fn retain(&mut self) {
        self.release_on_drop = false;
    }

    pub(super) fn check(&self) -> Result<()> {
        if Self::identity(&self.path)? != self.identity
            || fs::read_dir(&self.path)?.next().transpose()?.is_some()
        {
            return Err("参考阶段锁的原目录身份或内容变化，保留现场".into());
        }
        Ok(())
    }

    fn identity(path: &Path) -> Result<(u64, u64)> {
        super::unlinked(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err("参考阶段锁必须为普通目录".into());
        }
        native::resources::common::device_inode_for(path, &metadata)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // 只移除本阶段创建的空目录；异常现场或外来文件保持原样。
        if self.release_on_drop && self.check().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

pub(super) fn run(raw: &str) -> Result<Value> {
    let request = protocol::request(&json!({"raw":raw}))?;
    let command = request["command"].as_str().ok_or("参考操作缺失")?;
    if matches!(command, "check-dataset" | "check-existing") {
        return super::readonly::run(&request);
    }
    if command == "restore" {
        return super::restore::run(path(&request["backend_dir"])?, &request);
    }
    let backend = path(&request["backend_dir"])?;
    let reference = inputs::document::read(path(&request["plan"])?, backend, "参考恢复计划")?;
    inputs::domain::validate_plan(&reference.value, backend)?;
    if command == "plan" {
        return if request.as_object().is_some_and(|value| value.len() == 4) {
            model::summary(&json!({"plan":reference.value}))
        } else {
            super::plan::run(backend, &request, reference)
        };
    }
    let plan = &reference.value;
    if command == "dataset" {
        return super::dataset::run(backend, &request, plan);
    }
    if !matches!(command, "backup" | "copy" | "damage") {
        return Err("参考文件操作无效".into());
    }
    let work = files::work(&json!({"plan":plan}), true)?;
    let _guard = Lock::claim(path(&work)?)?;
    let receipt = receipts::start(
        &json!({"plan":plan,"command":command,"copy_id":request["copy_id"],
        "root":request["backup_root"],"receipt":null,"bindings":null}),
    )?;
    let result = (|| {
        if command == "backup" {
            return super::backup::run(backend, &reference, path(&work)?, &request);
        }
        let root = path(&request["backup_root"])?;
        super::unlinked(root)?;
        let manifest = native::json::read(&root.join("manifest.json"))?;
        if command == "copy" {
            files::copy(
                &json!({"work":work,"root":root,"manifest":manifest,"copy_id":request["copy_id"]}),
            )
        } else {
            files::damage(
                &json!({"work":work,"root":root,"manifest":manifest,"relative":request["artifact"],"missing":request["missing"]}),
            )
        }
    })();
    match result {
        Ok(value) => {
            receipts::finish(
                &json!({"receipt":receipt,"result":value,"failure":null,"resource_images":null}),
            )?;
            Ok(value)
        }
        Err(error) => {
            let (kind, message) = if command == "backup" {
                (
                    "ReferenceBackupFailure",
                    "参考备份未完成；核对原产物和外部只读诊断，禁止覆盖或自动重放",
                )
            } else {
                (
                    "ReferenceLocalFailure",
                    "参考本地阶段失败；保留现场，禁止覆盖或自动重放",
                )
            };
            let published = receipts::finish(&json!({"receipt":receipt,"result":null,
                "failure":{"error_type":kind,"message":message},"resource_images":null}));
            if published.is_err() {
                return Err(
                    "参考本地阶段与失败收据发布均未确认；保留运行收据和现场，禁止自动重放".into(),
                );
            }
            Err(error)
        }
    }
}
