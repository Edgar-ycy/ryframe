//! 控制锁和原子收据；任何失败均保留未确认的操作，不自动重放。
use super::{documents, receipts, role::Role};
use crate::{
    Result,
    recovery::clone::native::{digest, json, resources::common},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) fn write_new(path: &Path, value: &Value) -> Result<()> {
    publish(path.parent().ok_or("收据缺少父目录")?, path, value, false)
}
fn publish(directory: &Path, path: &Path, value: &Value, replace: bool) -> Result<()> {
    let name = path.file_name().ok_or("收据缺少文件名")?;
    let parent = path.parent().ok_or("收据缺少父目录")?;
    // 仅文件系统使用原生路径；Windows 保留长路径前缀，不改收据中的 portable 事实。
    // 暂存目录与目标父目录分别解析，保留 runtime 根暂存、控制锁内发布的边界。
    let native_directory = fs::canonicalize(directory)?;
    let native_target = fs::canonicalize(parent)?.join(name);
    let mut staged = tempfile::NamedTempFile::new_in(&native_directory)?;
    serde_json::to_writer_pretty(&mut staged, value)?;
    staged.write_all(b"\n")?;
    staged.as_file().sync_all()?;
    if replace {
        staged.persist(&native_target).map_err(|failure| {
            drop(failure.file);
            failure.error
        })?;
    } else {
        staged
            .persist_noclobber(&native_target)
            .map_err(|failure| {
                drop(failure.file);
                failure.error
            })?;
    }
    Ok(())
}

pub(super) fn read(path: &Path) -> Result<Value> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.len() == 0
        || metadata.len() > 4 * 1024 * 1024
        || common::link_count_for(path, &metadata)? != 1
    {
        return Err("角色 收据大小无效或经过硬链接".into());
    }
    json::read(path)
}

const FILES: &[&str] = &[
    "owner.json",
    "request.json",
    "candidate.json",
    "supervisor.json",
    "progress.json",
    "result.json",
    "preparation-failed.json",
];

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}
pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    crate::local_test_path::validate_absolute_path(
        path,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    Ok(crate::recovery::fixture_control::portable_path(
        &fs::canonicalize(path)?,
    ))
}
pub(super) fn source(directory: &Path) -> Result<String> {
    let evidence = directory.join("runtime-evidence.json");
    let value = json!({"runtime":file(&directory.join("runtime.json"))?,"runtime_evidence":if evidence.exists(){file(&evidence)?}else{Value::Null}});
    documents::hash(&value)
}
pub(super) fn file(path: &Path) -> Result<Value> {
    let value = digest::file(path)?;
    Ok(json!({"bytes":value.bytes,"sha256":value.sha256}))
}
pub(super) fn common(kind: &str, id: &Value, directory: &Path, source: &Value) -> Value {
    json!({"format_version":1,"kind":kind,"operation_id":id,"runtime_directory":directory,"source_sha256":source})
}

pub(super) struct Operation {
    pub role: Role,
    pub path: PathBuf,
    pub owner: Value,
    pub request: Value,
    inode: (u64, u64),
}
impl Operation {
    pub fn read(role: Role, directory: &Path, source: &str) -> Result<Self> {
        let path = directory.join(role.lock());
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || digest::linked(&metadata) {
            return Err("角色 控制锁不是可信目录".into());
        }
        let names = fs::read_dir(&path)?
            .map(|v| Ok(v?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>>>()?;
        if !["owner.json", "request.json"]
            .iter()
            .all(|v| names.iter().any(|n| n == v))
            || names.iter().any(|n| !FILES.contains(&n.as_str()))
        {
            return Err("角色 控制锁缺少收据或包含未知文件".into());
        }
        let operation = Self {
            role,
            owner: read(&path.join("owner.json"))?,
            request: read(&path.join("request.json"))?,
            inode: common::device_inode_for(&path, &metadata)?,
            path,
        };
        if Role::owner(&operation.owner)? != role {
            return Err("控制锁目录与 owner 角色不一致".into());
        }
        if operation.owner["source_sha256"] != source {
            return Err("角色 运行来源发生变化，保留锁".into());
        }
        operation.validate("operation", Value::Null)?;
        Ok(operation)
    }
    pub fn claim(
        role: Role,
        action: &str,
        backend: &Path,
        directory: &Path,
        runtime: &Value,
        timeout: f64,
    ) -> Result<Self> {
        Self::claim_inner(role, action, backend, directory, runtime, timeout, None)
    }
    pub fn claim_reserved(
        reservation: &super::start::Reservation,
        runtime: &Value,
        timeout: f64,
    ) -> Result<Self> {
        reservation.verify()?;
        Self::claim_inner(
            reservation.role,
            "start",
            &reservation.backend,
            &reservation.directory,
            runtime,
            timeout,
            Some(reservation),
        )
    }
    fn claim_inner(
        role: Role,
        action: &str,
        backend: &Path,
        directory: &Path,
        runtime: &Value,
        timeout: f64,
        reservation: Option<&super::start::Reservation>,
    ) -> Result<Self> {
        let source = json!(source(directory)?);
        let id = json!(
            reservation
                .map(|v| v.id().to_owned())
                .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string())
        );
        let identity = super::process::current()?;
        let mut owner = common(&role.kind("owner"), &id, directory, &source);
        owner["identity"] = identity.clone();
        owner["created_at_ns"] = json!(now());
        let mut request = common(&role.kind("request"), &id, directory, &source);
        for (key,value) in json!({"operation":action,"backend_root":backend,"scope_id":runtime["scope_id"],"controller_identity":identity,"owner_sha256":documents::hash(&owner)?,"runtime_receipt_sha256":file(&directory.join("runtime.json"))?["sha256"],(role.artifact()):runtime["artifacts"][role.name()],(role.ready()):runtime[role.ready()],"timeout_seconds":timeout,"log":if action=="start"{json!(format!("{}-{}.log",role.name(),id.as_str().ok_or("操作ID无效")?))}else{Value::Null},"requested_at_ns":now()}).as_object().ok_or("请求无效")? {request[key]=value.clone();}
        if role == Role::Frontend {
            request["request_binding"] = runtime["request_binding"].clone();
        }
        if let Some(reservation) = reservation {
            request["environment_binding"] = reservation.binding().clone();
            reservation.verify()?;
        }
        let temporary = directory.join(format!(
            ".{}.{}.tmp",
            role.lock(),
            id.as_str().ok_or("操作ID无效")?
        ));
        fs::create_dir(&temporary)?;
        json::write_new(&temporary.join("owner.json"), &owner)?;
        json::write_new(&temporary.join("request.json"), &request)?;
        if directory.join(role.lock()).exists() {
            return Err("角色 控制正在执行，保留候选操作".into());
        }
        fs::rename(&temporary, directory.join(role.lock()))?;
        Self::read(role, directory, source.as_str().ok_or("来源无效")?)
    }
    pub fn check(&self) -> Result<()> {
        let current = Self::read(
            self.role,
            self.directory(),
            self.owner["source_sha256"].as_str().ok_or("来源无效")?,
        )?;
        if current.inode != self.inode
            || current.owner != self.owner
            || current.request != self.request
        {
            return Err("角色 控制锁在发布前发生变化".into());
        }
        Ok(())
    }
    pub fn directory(&self) -> &Path {
        self.path.parent().expect("操作锁具有父目录")
    }
    pub fn id(&self) -> Result<&str> {
        self.owner["operation_id"]
            .as_str()
            .ok_or_else(|| "操作ID无效".into())
    }
    pub fn validate(&self, mode: &str, value: Value) -> Result<Value> {
        if let Some(binding) = self.request.get("environment_binding") {
            super::environment::Environment::recorded(binding)?.check()?;
        }
        let directory = self.directory();
        if self.role == Role::Frontend {
            crate::recovery::fixture_runtime::verify_frontend_handoff(
                directory,
                &self.request["request_binding"],
            )?;
        }
        let optional = |name: &str| -> Result<Value> {
            let path = self.path.join(name);
            if path.exists() {
                read(&path)
            } else {
                Ok(Value::Null)
            }
        };
        let runtime = if mode == "request" && self.role != Role::Worker {
            super::runtime::load(
                self.role,
                Path::new(
                    self.request["backend_root"]
                        .as_str()
                        .ok_or("缺少后端目录")?,
                ),
                directory,
            )?
        } else {
            read(&directory.join("runtime.json"))?
        };
        receipts::execute(
            &json!({"mode":mode,"directory":directory,"source":self.owner["source_sha256"],"owner":self.owner,"request":self.request,"value":value,"candidate":optional("candidate.json")?,"supervisor":optional("supervisor.json")?,"runtime_sha256":file(&directory.join("runtime.json"))?["sha256"],"backend":self.request["backend_root"],"runtime":runtime}),
        )
    }
    pub fn child(&self, name: &str) -> Result<Value> {
        self.validate(
            if name == "candidate.json" {
                "candidate"
            } else {
                "supervisor"
            },
            read(&self.path.join(name))?,
        )
    }
    pub fn bound(&self, kind: &str) -> Result<Value> {
        let mut value = common(
            kind,
            &self.owner["operation_id"],
            self.directory(),
            &self.owner["source_sha256"],
        );
        value["owner_sha256"] = json!(documents::hash(&self.owner)?);
        value["request_sha256"] = json!(documents::hash(&self.request)?);
        Ok(value)
    }
    pub fn publish(&self, name: &str, value: &Value, replace: bool) -> Result<()> {
        if !FILES[2..].contains(&name) {
            return Err("未知角色收据名称".into());
        }
        self.check()?;
        let target = self.path.join(name);
        if !replace && target.exists() {
            return Err("角色控制收据已经存在".into());
        }
        // 暂存不进入闭合控制目录，避免读者把发布中的临时文件误认为未知收据。
        publish(self.directory(), &target, value, replace)
    }
    pub fn result(
        &self,
        state: &str,
        identity: Value,
        error: Option<&str>,
        reconciled: bool,
    ) -> Result<Value> {
        let mut value = self.bound(&self.role.kind("result"))?;
        for (key,item) in json!({"operation":self.request["operation"],"scope_id":self.request["scope_id"],"controller_identity":self.owner["identity"],"outcome":if error.is_some(){"failed"}else{"succeeded"},"state":state,"identity":identity,"log":self.request["log"],"reconciled":reconciled,"error_type":error.map(|_|self.role.failure()),"error":error,"completed_at_ns":now()}).as_object().ok_or("结果无效")?{value[key]=item.clone();}
        Ok(value)
    }
    pub fn archive(&self) -> Result<Value> {
        self.check()?;
        let result = self.validate("result", read(&self.path.join("result.json"))?)?;
        for (name, mode) in [
            ("candidate.json", "candidate"),
            ("supervisor.json", "supervisor"),
            ("progress.json", "progress"),
        ] {
            if self.path.join(name).exists() {
                if !matches!(self.request["operation"].as_str(), Some("start" | "run")) {
                    return Err("非start操作包含监督收据".into());
                }
                self.validate(mode, read(&self.path.join(name))?)?;
            }
        }
        if result["outcome"] == "succeeded"
            && matches!(self.request["operation"].as_str(), Some("start" | "run"))
        {
            let progress = self.validate("progress", read(&self.path.join("progress.json"))?)?;
            let expected = if self.role == Role::Frontend {
                "completed"
            } else {
                "ready"
            };
            if progress["phase"] != expected
                || (self.role != Role::Frontend && progress["identity"] != result["identity"])
            {
                return Err("成功启动缺少同一进程的就绪进度".into());
            }
        }
        let archive = self.directory().join(self.role.archive());
        if !archive.exists() {
            fs::create_dir(&archive)?;
        }
        canonical(&archive)?;
        let target = archive.join(self.id()?);
        if target.exists() {
            return Err("操作ID已经归档".into());
        }
        self.check()?;
        fs::rename(&self.path, target)?;
        Ok(result)
    }
}
