//! 只消费原生控制回收和空锁归档，不把失败类别当作收口证明。
use super::*;
use crate::recovery::clone::native::{
    lifecycle::{identity, manifest::path_text},
    target_verify::guard::prepare_lock::guard_identity,
};

pub(super) struct Evidence {
    backend: PathBuf,
    files: Inputs,
    bindings: Vec<Value>,
    directories: Vec<Inputs>,
    archives: Vec<PathBuf>,
    absences: Vec<PathBuf>,
    guard: Option<ObservedGuard>,
    pub(super) binding: Value,
}

struct Read<'a> {
    backend: &'a Path,
    run: &'a Path,
    bindings: Vec<Value>,
    directories: Vec<Inputs>,
    archives: Vec<PathBuf>,
    absences: Vec<PathBuf>,
    guard: Option<ObservedGuard>,
}

#[derive(PartialEq, Eq)]
struct ObservedGuard {
    target: PathBuf,
    identity: (u64, u64),
}

impl Read<'_> {
    fn file(&mut self, path: &Path) -> Result<(Value, Value)> {
        let binding = common::binding(&protected_file(self.backend, path)?)?;
        let value = common::read_bound(self.backend, &binding, "原准备失败收口证据")?;
        self.bindings.push(binding.clone());
        Ok((binding, value))
    }

    fn directory(&mut self, path: &Path, expected: &Value) -> Result<()> {
        let path = common::local_directory(self.backend, path, "原准备归档目录")?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if json!(common::device_inode_for(&path, &metadata)?) != *expected {
            return Err("原准备归档目录的物理身份变化".into());
        }
        self.directories.push(Inputs::directory(&path)?);
        Ok(())
    }
}

impl Evidence {
    pub(super) fn load(
        backend: &Path,
        run: &Path,
        manifest: &Value,
        saved: &Value,
        registration: &Value,
    ) -> Result<Self> {
        let mut read = Read {
            backend,
            run,
            bindings: Vec::new(),
            directories: Vec::new(),
            archives: Vec::new(),
            absences: Vec::new(),
            guard: None,
        };
        let mut proofs = Vec::new();
        for row in state::attempts(saved)?
            .iter()
            .filter(|r| r["stage"] == "post-copy" && r["mode"] == "prepare")
        {
            let number = row["number"].as_u64().ok_or("原准备阶段号无效")?;
            read.absences.push(state::result_path(run, number));
            let running = running(row);
            let (controller, owner) = state::controller_record(run, number, &running)?;
            identity::validate(&owner, run)?;
            if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"] {
                return Err("原准备控制器属于其他运行清单".into());
            }
            if read.file(&state::controller_path(run, number))?.0 != controller {
                return Err("原准备控制器在固定证据期间变化".into());
            }
            if row["error_type"] == "PostCopyPreparationRejected" {
                let (failure, value) = read.file(&run.join(format!("failure-{number:04}.json")))?;
                normal_failure(&value, number, &controller)?;
                proofs.push(
                    json!({"failure":failure,"controller":controller,"identity":owner["identity"]}),
                );
            } else if row["error_type"] == "ControllerInterrupted" {
                proofs.push(interrupted(
                    &mut read,
                    manifest,
                    saved,
                    &running,
                    &controller,
                    &owner,
                    registration,
                )?);
            } else {
                return Err("原准备失败类别未受支持".into());
            }
        }
        let initialized = common::bound_file(backend, &manifest["initialized"], "原初始化")?;
        let (_, initial) = read.file(&initialized)?;
        read.absences.push(PathBuf::from(document::text(
            &initial["generation"]["selected"]["runtime_dir"],
            "原产品运行目录",
        )?));
        let files = Inputs::open(
            read.bindings
                .iter()
                .map(|b| common::bound_file(backend, b, "原准备固定证据"))
                .collect::<Result<Vec<_>>>()?,
        )?;
        let result = Self {
            backend: backend.to_owned(),
            files,
            bindings: read.bindings,
            directories: read.directories,
            archives: read.archives,
            absences: read.absences,
            guard: read.guard,
            binding: json!(proofs),
        };
        result.check()?;
        Ok(result)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.files.check()?;
        for binding in &self.bindings {
            common::bound_file(&self.backend, binding, "原准备失败收口后像")?;
        }
        for directory in &self.directories {
            directory.check()?;
        }
        for archive in &self.archives {
            if std::fs::read_dir(archive)?.next().transpose()?.is_some() {
                return Err("原空锁归档出现新成员".into());
            }
        }
        for path in &self.absences {
            absent(path)?;
        }
        if let Some(guard) = &self.guard
            && guard_identity(&guard.target)? != guard.identity
        {
            return Err("原目标互斥身份变化".into());
        }
        self.files.check()
    }
}

fn running(row: &Value) -> Value {
    let mut result = row.clone();
    result["status"] = json!("running");
    for key in ["finished_at", "result", "error_type"] {
        result[key] = Value::Null;
    }
    result
}

fn normal_failure(value: &Value, number: u64, controller: &Value) -> Result<()> {
    if *value
        != json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
        "stage":"post-copy","mode":"prepare","error_type":"PostCopyPreparationRejected",
        "operation_id":"clone.post-copy.prepare","controller":controller})
    {
        return Err("原准备拒绝未绑定完整阶段、操作和控制器".into());
    }
    Ok(())
}

fn interrupted(
    read: &mut Read<'_>,
    manifest: &Value,
    saved: &Value,
    running: &Value,
    controller: &Value,
    owner: &Value,
    registration: &Value,
) -> Result<Value> {
    let number = running["number"].as_u64().ok_or("原中断阶段无效")?;
    let observation_path = read
        .run
        .join(format!("prepare-target-lock-{number:04}.json"));
    let (observation_binding, observation) = read.file(&observation_path)?;
    observation_schema(&observation)?;
    let mut after = saved.clone();
    after["attempts"]
        .as_array_mut()
        .ok_or("原阶段列表无效")?
        .truncate(usize::try_from(number)?);
    let mut before = after.clone();
    *before["attempts"]
        .as_array_mut()
        .and_then(|r| r.last_mut())
        .ok_or("缺少原准备前像")? = running.clone();
    if observation["format_version"] != 1
        || !matches!(
            observation["kind"].as_str(),
            Some(
                "devex-post-copy-prepare-target-lock"
                    | "devex-post-copy-prepare-target-lock-after-exit"
            )
        )
        || Path::new(document::text(&observation["path"], "原空锁观察路径")?) != observation_path
        || observation["attempt"] != *running
        || observation["controller"] != *controller
        || observation["owner"] != *owner
        || observation["state_sha256"] != digest::plan_hash(&before)?
        || observation["context"]["manifest"] != state::manifest_binding(read.run)?
        || observation["context"]["initialized"] != manifest["initialized"]
        || observation["context"]["registration"]["active"] != *registration
    {
        return Err("原准备前像、登记或控制器与当前精确历史不同".into());
    }
    let register = state::attempts(saved)?
        .iter()
        .find(|r| r["stage"] == "post-copy" && r["mode"] == "register")
        .ok_or("缺少原登记")?;
    if observation["context"]["registration"]["register"] != register["result"] {
        return Err("原准备未绑定唯一正式登记".into());
    }
    let recovery = controller_recovery(read, controller, owner, &before, &after, number)?;
    let (_, intent) = read.file(&observation_path.with_extension("recovering.json"))?;
    let (recovered_binding, recovered) =
        read.file(&observation_path.with_extension("recovered.json"))?;
    let archive = observation_path.with_extension("original-empty-lock");
    if intent
        != json!({"status":"prepare_target_lock_recovery_started","observation":observation_binding,
        "state":intent["state"],"archive":path_text(&archive, "原空锁归档路径")?})
        || recovered
            != json!({"status":"prepare_target_lock_recovered",
            "intent":path_text(&observation_path.with_extension("recovering.json"), "原空锁恢复意图路径")?,
            "intent_sha256":digest::plan_hash(&intent)?,"attempt":number,"remote_writes":0})
    {
        return Err("原目标空锁恢复未绑定完整前像与结果".into());
    }
    state_bytes(read.run, &after, &intent["state"], number)?;
    archive_identity(read, manifest, number, &observation, &archive)?;
    Ok(
        json!({"controller":controller,"observation":observation_binding,"controller_recovery":recovery,"target_lock_recovered":recovered_binding}),
    )
}

fn observation_schema(observation: &Value) -> Result<()> {
    document::exact(
        observation,
        &[
            "format_version",
            "kind",
            "path",
            "state_sha256",
            "attempt",
            "controller",
            "owner",
            "context",
            "lock",
        ],
        "原准备空锁前像",
    )?;
    document::exact(
        &observation["context"],
        &[
            "manifest",
            "initialized",
            "registration",
            "attempt_directory",
        ],
        "原准备上下文",
    )?;
    document::exact(
        &observation["context"]["registration"],
        &["active", "register"],
        "原准备登记",
    )?;
    document::exact(
        &observation["lock"],
        &["target", "guard", "directory"],
        "原准备空锁身份",
    )?;
    Ok(())
}

fn state_bytes(run: &Path, value: &Value, binding: &Value, number: u64) -> Result<()> {
    document::exact(
        binding,
        &["path", "bytes", "sha256"],
        "原阶段前后像字节绑定",
    )?;
    if Path::new(document::text(&binding["path"], "原状态路径")?) != run.join("state.json") {
        return Err("原状态路径不同".into());
    }
    let mut raw = serde_json::to_vec_pretty(value)?;
    raw.push(b'\n');
    if state::prefix::count(
        &raw,
        binding["bytes"].as_u64().ok_or("原状态大小无效")?,
        document::text(&binding["sha256"], "原状态摘要")?,
    )? != usize::try_from(number)?
    {
        return Err("原准备前后像不是当前历史的精确前缀".into());
    }
    Ok(())
}

fn controller_recovery(
    read: &mut Read<'_>,
    controller: &Value,
    owner: &Value,
    before: &Value,
    after: &Value,
    number: u64,
) -> Result<Value> {
    let mut selected = None;
    for entry in std::fs::read_dir(read.run)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_prefix("recovery-")?.strip_suffix(".intent.json"))
        else {
            continue;
        };
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("原控制回收意图名无效".into());
        }
        let value = document::read(&entry.path())?;
        if value["owner"] != *controller {
            continue;
        }
        if selected.is_some() {
            return Err("原控制器存在多份回收意图".into());
        }
        let (intent_binding, value) = read.file(&entry.path())?;
        document::exact(
            &value,
            &[
                "status",
                "owner",
                "state_before",
                "state_after_sha256",
                "lock",
            ],
            "原控制恢复意图",
        )?;
        let (result_binding, result) = read.file(&read.run.join(format!("recovered-{id}.json")))?;
        if value["status"] != "controller_recovery_started"
            || value["owner"] != *controller
            || value["state_after_sha256"] != digest::plan_hash(after)?
            || value["lock"][2] != *owner
            || result
                != json!({"status":"controller_recovered","owner":owner,"remote_writes":0,
                "copy_requires_reconciliation":true,"state_sha256":value["state_after_sha256"]})
        {
            return Err("原控制回收未形成同一准备的明确后像".into());
        }
        state_bytes(read.run, before, &value["state_before"], number)?;
        selected = Some(json!({"intent":intent_binding,"result":result_binding}));
    }
    selected.ok_or_else(|| "原中断准备缺少原生控制回收证明".into())
}

fn archive_identity(
    read: &mut Read<'_>,
    manifest: &Value,
    number: u64,
    observation: &Value,
    archive: &Path,
) -> Result<()> {
    read.directory(archive, &observation["lock"]["directory"])?;
    read.archives.push(archive.to_owned());
    let initialized = common::bound_file(read.backend, &manifest["initialized"], "原初始化")?;
    let target = initialized.parent().ok_or("原目标目录缺失")?;
    read.directory(target, &observation["lock"]["target"])?;
    let guard = ObservedGuard {
        target: target.to_owned(),
        identity: guard_identity(target)?,
    };
    if json!(guard.identity) != observation["lock"]["guard"]
        || read.guard.as_ref().is_some_and(|prior| prior != &guard)
    {
        return Err("原目标互斥身份变化".into());
    }
    // 互斥为原生单字节文件。随后由 Generation 持有可写句柄并复核内容。
    read.guard = Some(guard);
    let attempt = read
        .run
        .join("post-copy")
        .join(format!("attempt-{number:04}"));
    read.directory(&attempt, &observation["context"]["attempt_directory"])?;
    for name in [
        "session-launch.json",
        "session-process.json",
        "session.stderr.log",
        "session.stdout.log",
    ] {
        absent(&attempt.join(name))?;
        read.absences.push(attempt.join(name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_rejection_binds_operation_attempt_and_controller() {
        let controller = json!({"path":"controller-0029.json","bytes":697,"sha256":"exact"});
        let original = json!({"format_version":2,"kind":"devex-stage-failure","attempt":29,"stage":"post-copy",
            "mode":"prepare","error_type":"PostCopyPreparationRejected","operation_id":"clone.post-copy.prepare","controller":controller});
        assert!(normal_failure(&original, 29, &controller).is_ok());
        for (key, value) in [
            ("operation_id", json!("clone.post-copy.start")),
            ("attempt", json!(28)),
            ("controller", json!({})),
            ("error_type", json!("StageError")),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            assert!(normal_failure(&changed, 29, &controller).is_err());
        }
    }

    #[test]
    fn interrupted_preparation_requires_the_exact_original_state_bytes() {
        let run = std::env::temp_dir().join("marker-state-proof");
        let original = json!({"format_version":1,"manifest":{},"attempts":[
            {"number":22,"stage":"post-copy","mode":"register","status":"passed"},
            {"number":23,"stage":"post-copy","mode":"prepare","status":"failed","error_type":"ControllerInterrupted"}
        ]});
        let mut raw = serde_json::to_vec_pretty(&original).unwrap();
        raw.push(b'\n');
        use sha2::{Digest, Sha256};
        let binding = json!({"path":run.join("state.json"),"bytes":raw.len(),"sha256":hex::encode(Sha256::digest(&raw))});
        assert!(state_bytes(&run, &original, &binding, 2).is_ok());
        let mut changed = original;
        changed["attempts"][1]["error_type"] = json!("PostCopyPreparationRejected");
        assert!(state_bytes(&run, &changed, &binding, 2).is_err());
    }
}
