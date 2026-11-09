//! 只读核对精确归档启动，按同代内核状态确认结果。
use super::{
    super::{documents, environment::Environment, guard::Guard, preparation, receipts, runtime},
    Role,
    bound::{StopGeneration, require_no_pending},
    operation, process, tree,
};
use crate::{
    Result,
    recovery::clone::native::{
        resources::common,
        target_verify::{authority::require_closed, guard::Inputs},
    },
};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

/// 保留原帧，用独立观察确认归档结果。原活动控制记录由既有入口处理。
pub(crate) fn archived_start_bound(
    role: Role,
    product: &Path,
    directory: &Path,
    environment: &Value,
    original_executor: &Value,
    id: &str,
) -> Result<Option<Value>> {
    if !matches!(role, Role::Api | Role::Worker) || !documents::hex(&json!(id), 32) {
        return Err("归档启动只接受明确 API 或 Worker 及原预留操作 ID".into());
    }
    if operation::canonical(directory)?.as_path() != directory {
        return Err("归档启动必须使用原规范运行目录".into());
    }
    let parent = directory.join(role.archive());
    let path = parent.join(id);
    if !present(&parent)? {
        return Ok(None);
    }
    directory_id(&parent)?;
    if !present(&path)? {
        return Ok(None);
    }

    let guard = Guard::acquire_existing(role, directory)?;
    require_no_pending(role, directory)?;
    let environment = Environment::capture(environment)?;
    let archive = Archive::open(parent, path)?;
    let mut source_paths = vec![directory.join("runtime.json")];
    let evidence = directory.join("runtime-evidence.json");
    if present(&evidence)? {
        source_paths.push(evidence);
    }
    let source_inputs = Inputs::open(source_paths)?;
    let source = operation::source(directory)?;
    let current = runtime::load(role, product, directory)?;
    let frame = archive.frame(directory, product, &source, &current)?;
    let outcome = frame.classify(role, id, original_executor, &environment)?;
    if runtime::load(role, product, directory)? != current
        || operation::source(directory)? != source
    {
        return Err("归档启动的原运行合同或来源字节发生变化".into());
    }
    source_inputs.check()?;
    archive.check()?;
    environment.check()?;
    let observed = frame.observe(role, directory, &current, id, outcome)?;
    require_no_pending(role, directory)?;
    guard.check()?;
    Ok(observed.then(|| json!({"outcome":outcome,"result":frame.result})))
}

pub(crate) struct ArchivedStop<'a> {
    pub(crate) executor: &'a Value,
    pub(crate) id: &'a str,
    pub(crate) generation: StopGeneration<'a>,
}

/// 保持角色互斥直到原停止历史发布完毕；这里只读取角色收据和内核状态。
pub(crate) fn archived_stop_bound(
    role: Role,
    product: &Path,
    directory: &Path,
    environment: &Value,
    stop: &ArchivedStop<'_>,
    publish: impl FnOnce() -> Result<()>,
) -> Result<Option<Value>> {
    if !matches!(role, Role::Api | Role::Worker)
        || !documents::hex(&json!(stop.id), 32)
        || !documents::hex(&json!(stop.generation.start_id), 32)
        || operation::canonical(directory)?.as_path() != directory
    {
        return Err("归档停止必须使用原角色、操作及规范运行目录".into());
    }
    let parent = directory.join(role.archive());
    let path = parent.join(stop.id);
    if !present(&path)? {
        return Ok(None);
    }
    let guard = Guard::acquire_existing(role, directory)?;
    require_no_pending(role, directory)?;
    let environment = Environment::capture(environment)?;
    let archive = Archive::open(parent, path)?;
    if archive.files.len() != 3 || process::alive(stop.executor)? {
        return Err("原停止归档包含其他收据或原执行者仍在运行".into());
    }
    let source = operation::source(directory)?;
    let current = runtime::load(role, product, directory)?;
    let frame = archive.frame(directory, product, &source, &current)?;
    frame.require_stop(role, stop)?;
    let tree = tree::read(directory, &current, role, true)?.ok_or("归档停止缺少原角色树")?;
    if tree["operation_id"] != stop.generation.start_id
        || tree["process"] != *stop.generation.identity
    {
        return Err("归档停止不同于已预留的原启动代次和进程身份".into());
    }
    let protected = Inputs::open([
        directory.join("runtime.json"),
        directory.join(format!("{}.json", role.name())),
        directory.join(format!("{}-tree.json", role.name())),
        tree::path(directory, role, stop.generation.start_id, "stopped"),
    ])?;
    let check = || {
        require_no_pending(role, directory)?;
        guard.check()?;
        environment.check()?;
        archive.check()?;
        protected.check()?;
        if runtime::load(role, product, directory)? != current
            || operation::source(directory)? != source
        {
            return Err("归档停止期间原角色闭口或冻结运行来源发生变化".into());
        }
        stop.generation
            .prove_closed(role, directory, &current, &tree)?;
        protected.check()
    };
    check()?;
    publish()?;
    check()?;
    Ok(Some(frame.result))
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn directory_id(path: &Path) -> Result<(u64, u64)> {
    if operation::canonical(path)?.as_path() != path {
        return Err("角色归档目录缺少原规范路径".into());
    }
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn members(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(path)?
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    files.sort();
    if files.iter().any(|path| {
        !operation::FILES
            .iter()
            .any(|name| path.file_name() == Some(OsStr::new(name)))
    }) || ["owner.json", "request.json", "result.json"]
        .iter()
        .any(|name| !files.contains(&path.join(name)))
    {
        return Err("角色归档缺少必需收据或包含未知成员".into());
    }
    Ok(files)
}

struct Archive {
    parent: PathBuf,
    path: PathBuf,
    identities: [(u64, u64); 2],
    files: Vec<PathBuf>,
    inputs: Inputs,
}

impl Archive {
    fn open(parent: PathBuf, path: PathBuf) -> Result<Self> {
        let identities = [directory_id(&parent)?, directory_id(&path)?];
        let files = members(&path)?;
        for file in &files {
            operation::check_receipt_file(file)?;
        }
        let inputs = Inputs::open(files.iter().cloned())?;
        let archive = Self {
            parent,
            path,
            identities,
            files,
            inputs,
        };
        archive.check()?;
        Ok(archive)
    }

    fn check(&self) -> Result<()> {
        if [directory_id(&self.parent)?, directory_id(&self.path)?] != self.identities
            || members(&self.path)? != self.files
        {
            return Err("角色归档身份或成员集合发生变化".into());
        }
        self.inputs.check()
    }

    fn frame(
        &self,
        directory: &Path,
        product: &Path,
        source: &str,
        runtime: &Value,
    ) -> Result<Frame> {
        let read = |name| {
            let path = self.path.join(name);
            if self.files.contains(&path) {
                operation::read(&path)
            } else {
                Ok(Value::Null)
            }
        };
        let owner = read("owner.json")?;
        let request = read("request.json")?;
        let candidate = read("candidate.json")?;
        let supervisor = read("supervisor.json")?;
        let progress = read("progress.json")?;
        let result = read("result.json")?;
        let mut args = json!({"mode":"request","directory":directory,"source":source,
            "owner":owner,"request":request,"value":null,"candidate":candidate,"supervisor":supervisor,
            "runtime_sha256":operation::file(&directory.join("runtime.json"))?["sha256"],
            "backend":product,"runtime":runtime});
        receipts::execute(&args)?;
        for (mode, value) in [
            ("candidate", &candidate),
            ("supervisor", &supervisor),
            ("progress", &progress),
            ("result", &result),
        ] {
            if !value.is_null() {
                args["mode"] = json!(mode);
                args["value"] = value.clone();
                receipts::execute(&args)?;
            }
        }
        let diagnostic = read("preparation-failed.json")?;
        if !diagnostic.is_null() {
            preparation::validate_record(&diagnostic, directory, &owner, &request)?;
        }
        Ok(Frame {
            owner,
            request,
            supervisor,
            progress,
            result,
        })
    }
}

struct Frame {
    owner: Value,
    request: Value,
    supervisor: Value,
    progress: Value,
    result: Value,
}

impl Frame {
    fn require_stop(&self, role: Role, stop: &ArchivedStop<'_>) -> Result<()> {
        if Role::owner(&self.owner)? != role
            || self.owner["operation_id"] != stop.id
            || self.owner["identity"] != *stop.executor
            || self.request["operation"] != "stop"
            || self.result["outcome"] != "succeeded"
            || self.result["state"] != "stopped"
            || !self.result["identity"].is_null()
            || !self.supervisor.is_null()
            || !self.progress.is_null()
        {
            return Err("归档停止未由原执行者完成或缺少明确停止结果".into());
        }
        Ok(())
    }

    fn classify(
        &self,
        role: Role,
        id: &str,
        original_executor: &Value,
        environment: &Environment,
    ) -> Result<&'static str> {
        if Role::owner(&self.owner)? != role
            || self.owner["operation_id"] != id
            || self.owner["identity"] != *original_executor
            || self.request["operation"] != "start"
            || self.request.get("environment_binding") != Some(environment.binding())
        {
            return Err("归档启动与原操作、控制器或冻结环境不一致".into());
        }
        match self.result["outcome"].as_str() {
            Some("succeeded")
                if self.progress["phase"] == "ready"
                    && self.progress["identity"] == self.result["identity"] =>
            {
                Ok("succeeded")
            }
            Some("failed")
                if self.result["error_type"] == role.failure()
                    && self.result["identity"].is_null() =>
            {
                Ok("failed")
            }
            _ => Err("归档启动结果与原角色进度或固定失败状态不一致".into()),
        }
    }

    fn observe(
        &self,
        role: Role,
        directory: &Path,
        runtime: &Value,
        id: &str,
        outcome: &str,
    ) -> Result<bool> {
        let ready_path = tree::path(directory, role, id, "ready");
        let closed_path = tree::path(directory, role, id, "stopped");
        let tree_path = directory.join(format!("{}-tree.json", role.name()));
        let role_path = directory.join(format!("{}.json", role.name()));
        let mut files = vec![ready_path.clone(), tree_path, role_path];
        let failed = outcome == "failed";
        let closed = present(&closed_path)?;
        if failed {
            if !closed
                || self.supervisor.is_null()
                || !matches!(self.progress["phase"].as_str(), Some("started" | "ready"))
            {
                return Ok(false);
            }
            for path in &files {
                if !present(path)? {
                    return Ok(false);
                }
            }
            files.push(closed_path.clone());
        } else if closed {
            return Err("归档成功启动已存在同代成员关闭记录".into());
        }
        for file in &files {
            operation::check_receipt_file(file)?;
        }
        let protected = Inputs::open(files)?;
        let registered = tree::read(directory, runtime, role, true)?.ok_or("缺少角色进程树")?;
        if failed && registered["operation_id"] != id {
            return Ok(false);
        }
        let ready = operation::read(&ready_path)?;
        self.match_tree(role, directory, id, &registered, &ready)?;
        let product = process::registered(role, directory, runtime)?;
        if failed {
            if product.is_some() {
                return Err("归档失败启动的原产品仍存活".into());
            }
            tree::closure(&registered, Duration::ZERO, false)?;
            require_closed(runtime[role.ready()].as_str().ok_or("角色端点无效")?)?;
        } else if product.as_ref() != Some(&registered["process"])
            || tree::running(directory, runtime, role, &registered["process"])? != registered
            || present(&closed_path)?
        {
            return Err("归档成功启动的原完整进程树未保持运行".into());
        }
        protected.check()?;
        Ok(true)
    }

    fn match_tree(
        &self,
        role: Role,
        directory: &Path,
        id: &str,
        registered: &Value,
        ready: &Value,
    ) -> Result<()> {
        if registered["operation_id"] != id
            || registered["supervisor"] != self.supervisor["identity"]
            || registered["process"] != self.progress["identity"]
            || registered["supervisor"]["pid"] == registered["process"]["pid"]
            || *ready
                != json!({"format_version":1,"operation_id":id,"directory":directory,
                    "role":role.name(),"scope_id":self.request["scope_id"],
                    "supervisor":registered["supervisor"],"monitor":registered["monitor"]})
        {
            return Err("归档启动的原监督器、成员或产品与当前进程树不一致".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/full_stack/worker/archived_start_tests.rs"]
pub(super) mod tests;
