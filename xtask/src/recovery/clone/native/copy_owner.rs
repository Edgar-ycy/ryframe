//! 复制控制器登记、原会话绑定和两个受管复制锁的归属证据。
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    digest, json as json_io,
    lifecycle::{identity, lock::ProcessGuard, manifest},
    resources::common,
    target_verify::{guard::Inputs, publication},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{fixture_control::portable_path, inputs::document},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Target,
    Ledger,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Ledger => "ledger",
        }
    }

    pub(crate) fn guard_name(self) -> &'static str {
        match self {
            Self::Target => "copy-target.guard",
            Self::Ledger => "copy-ledger.guard",
        }
    }
}

pub(crate) struct Owner {
    backend: PathBuf,
    copy_root: PathBuf,
    owner_binding: Value,
    session_id: String,
    plan_sha256: String,
    generation_sha256: String,
    controller: Value,
    target_lock: PathBuf,
    ledger_lock: PathBuf,
    owner_input: Inputs,
    session_inputs: Inputs,
}

impl Owner {
    pub(crate) fn register(
        backend: &Path,
        copy_root: &Path,
        source_export: &Value,
        initialized: &Value,
    ) -> Result<Self> {
        validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)?;
        let backend = portable_path(&fs::canonicalize(backend)?);
        let copy_root = common::local_directory(&backend, copy_root, "固定复制目录")?;
        let session = session(&backend, &copy_root, None, source_export, initialized)?;
        let uuid = uuid::Uuid::new_v4().simple().to_string();
        let value = json!({
            "format_version": 1, "kind": "devex-copy-owner",
            "backend_root": common::path_text(&backend, "复制锁后端")?,
            "copy_directory": common::path_text(&copy_root, "登记复制目录")?,
            "session": session.binding, "session_id": format!("session-{uuid}"),
            "source_export": source_export, "initialized": initialized,
            "plan_sha256": session.plan_sha256, "generation_sha256": session.generation_sha256,
            "controller": identity::current()?
        });
        let path = copy_root.join(format!("copy-owner-{uuid}.json"));
        validate_local_test_path(&path, &backend, LocalTestPathKind::OutputFile)?;
        let raw = document::canonical_bytes(&value)?;
        let binding = manifest::binding(
            &path,
            &digest::FileDigest {
                bytes: raw.len() as u64,
                sha256: hex::encode(Sha256::digest(&raw)),
            },
        )?;
        session.inputs.check()?;
        publication::publish(&path, &value)?;
        let owner = Self::read(&backend, &copy_root, &binding, source_export, initialized)?;
        session.inputs.check()?;
        Ok(owner)
    }

    pub(crate) fn read(
        backend: &Path,
        copy_root: &Path,
        owner_binding: &Value,
        source_export: &Value,
        initialized: &Value,
    ) -> Result<Self> {
        validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)?;
        let backend = portable_path(&fs::canonicalize(backend)?);
        let selected = common::local_directory(&backend, copy_root, "固定复制目录")?;
        let owner_path = bound_path(&backend, owner_binding, "复制锁持有人收据")?;
        let owner_input = protect_inputs(std::slice::from_ref(&owner_path), "复制锁持有人收据")?;
        common::bound_file(&backend, owner_binding, "复制锁持有人收据")?;
        let value = json_io::read(&owner_path)?;
        let copy_root = owner_scope(&backend, &selected, &owner_path, &value)?;
        if value["source_export"] != *source_export || value["initialized"] != *initialized {
            return Err("复制锁恢复必须绑定调用方固定的来源和目标".into());
        }
        let session_id = json_io::text(&value["session_id"], "复制会话")?;
        common::scope_id(session_id)?;
        let controller = common::validate_process_identity(&value["controller"])?;
        controller_pid(&controller)?;
        controller_started(&controller)?;

        let session = session(
            &backend,
            &copy_root,
            Some(&value["session"]),
            source_export,
            initialized,
        )?;
        if value["plan_sha256"].as_str() != Some(session.plan_sha256.as_str())
            || value["generation_sha256"].as_str() != Some(session.generation_sha256.as_str())
        {
            return Err("复制锁计划或代次与原会话不匹配".into());
        }
        let target_lock = session
            .target
            .parent()
            .ok_or("初始化收据没有固定目标目录")?
            .join("initialize.lock");
        let ledger_lock = copy_root.join("ledger").join("clone.lock");
        let owner = Self {
            backend,
            copy_root,
            owner_binding: owner_binding.clone(),
            session_id: session_id.to_owned(),
            plan_sha256: session.plan_sha256,
            generation_sha256: session.generation_sha256,
            controller,
            target_lock,
            ledger_lock,
            owner_input,
            session_inputs: session.inputs,
        };
        owner.check()?;
        Ok(owner)
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn binding(&self) -> &Value {
        &self.owner_binding
    }

    pub(crate) fn ledger_binding(&self) -> (&Path, &str, &str) {
        (&self.backend, &self.plan_sha256, &self.generation_sha256)
    }

    pub(crate) fn controller(&self) -> &Value {
        &self.controller
    }

    pub(crate) fn lock_path(&self, role: Role) -> &Path {
        match role {
            Role::Target => &self.target_lock,
            Role::Ledger => &self.ledger_lock,
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.owner_input.check()?;
        self.session_inputs.check()
    }

    pub(crate) fn observe_stopped(&self) -> Result<Option<Value>> {
        self.check()?;
        let pid = controller_pid(self.controller())?;
        let expected_started = controller_started(self.controller())?;
        let actual = identity::process(pid)?;
        if let Some(value) = &actual {
            let observed = common::validate_process_identity(value)?;
            if controller_pid(&observed)? != pid {
                return Err("复制控制器观察返回不同 PID".into());
            }
            // Python 记录映像的规范路径。Windows 原生观察可能返回同一映像的另一种写法。
            if controller_started(&observed)? == expected_started {
                return Err("登记的复制控制器仍在运行，禁止回收锁".into());
            }
        }
        self.check()?;
        Ok(actual)
    }

    pub(crate) fn claim_lock(&self, role: Role, guard: &ProcessGuard) -> Result<Value> {
        self.check_guard(role, guard)?;
        self.check_current_controller()?;
        self.check()?;
        let path = self.lock_path(role);
        validate_local_test_path(path, &self.backend, LocalTestPathKind::NewDirectory)?;
        fs::create_dir(path)?;
        let created = lock_identity(path)?.ok_or("新复制锁目录已消失，保留现场")?;
        self.check_guard(role, guard)?;
        self.check()?;
        let record = self.lock_record(role, created)?;
        if lock_identity(path)? != Some(created)
            || fs::read_dir(path)?.next().transpose()?.is_some()
        {
            return Err("新复制锁目录身份或成员变化，保留现场".into());
        }
        publication::publish(&path.join("owner.json"), &record)?;
        self.check_live_lock(role, guard, &record)?;
        Ok(record)
    }

    pub(crate) fn check_live_lock(
        &self,
        role: Role,
        guard: &ProcessGuard,
        expected: &Value,
    ) -> Result<()> {
        self.check_guard(role, guard)?;
        self.check_current_controller()?;
        self.require_lock(role, expected)?;
        self.check_guard(role, guard)
    }

    pub(crate) fn release_lock(
        &self,
        role: Role,
        guard: &ProcessGuard,
        expected: &Value,
    ) -> Result<()> {
        self.check_current_controller()?;
        self.release_verified(role, guard, expected)
    }

    pub(super) fn release_recovered_lock(
        &self,
        role: Role,
        guard: &ProcessGuard,
        expected: &Value,
    ) -> Result<()> {
        self.observe_stopped()?;
        self.release_verified(role, guard, expected)
    }

    fn check_current_controller(&self) -> Result<()> {
        if identity::current()? != self.controller {
            return Err("只有登记的当前复制控制器可操作活体锁".into());
        }
        Ok(())
    }

    fn check_guard(&self, role: Role, guard: &ProcessGuard) -> Result<()> {
        let parent = self.lock_path(role).parent().ok_or("复制锁缺少父目录")?;
        guard.check_named(parent, role.guard_name())
    }

    fn require_lock(&self, role: Role, expected: &Value) -> Result<()> {
        if self.verify_lock(role)?.as_ref() != Some(expected) {
            return Err("复制锁归属在核对前变化，保留现场".into());
        }
        Ok(())
    }

    fn lock_record(&self, role: Role, (device, inode): (u64, u64)) -> Result<Value> {
        Ok(json!({
            "format_version": 1, "kind": "devex-owned-copy-lock", "role": role.name(),
            "owner_receipt": self.owner_binding, "controller": self.controller,
            "path": common::path_text(self.lock_path(role), "复制锁路径")?,
            "device": device, "inode": inode
        }))
    }

    fn release_verified(&self, role: Role, guard: &ProcessGuard, expected: &Value) -> Result<()> {
        self.check_guard(role, guard)?;
        let path = self.lock_path(role);
        let owner_path = input_path(&self.backend, &path.join("owner.json"), "复制锁登记持有人")?;
        let frozen = protect_inputs(std::slice::from_ref(&owner_path), "复制锁登记持有人")?;
        self.require_lock(role, expected)?;
        let mut raw = Vec::new();
        File::open(&owner_path)?
            .take(json_io::MAX_JSON_BYTES + 1)
            .read_to_end(&mut raw)?;
        if raw.len() as u64 > json_io::MAX_JSON_BYTES {
            return Err("复制锁归属收据超过读取上限".into());
        }
        frozen.check()?;
        self.check_guard(role, guard)?;
        drop(frozen);
        fs::remove_file(&owner_path)?;
        if let Err(error) = fs::remove_dir(path) {
            return match restore_owner(path, &owner_path, expected, &raw) {
                Ok(()) => Err(error.into()),
                Err(restore) => {
                    Err(format!("{error}；复制锁原持有人收据恢复失败：{restore}").into())
                }
            };
        }
        self.check_guard(role, guard)
    }

    /// 调用方在两个既有复制 guard 内核对，缺失目录代表该锁尚未建立或已释放。
    pub(crate) fn verify_lock(&self, role: Role) -> Result<Option<Value>> {
        self.check()?;
        let path = self.lock_path(role);
        validate_local_test_path(path, &self.backend, LocalTestPathKind::StateDirectory)?;
        let Some(before) = lock_identity(path)? else {
            return Ok(None);
        };
        let receipt = path.join("owner.json");
        require_only_owner(path)?;
        let file = input_path(&self.backend, &receipt, "复制锁登记持有人")?;
        let inputs = protect_inputs(std::slice::from_ref(&file), "复制锁登记持有人")?;
        let actual = json_io::read(&file)?;
        let expected = self.lock_record(role, before)?;
        if actual != expected || lock_identity(path)? != Some(before) {
            return Err("复制锁没有匹配的登记持有人或目录身份已变化".into());
        }
        require_only_owner(path)?;
        inputs.check()?;
        self.check()?;
        if lock_identity(path)? != Some(before) {
            return Err("复制锁目录身份在核对后验时变化".into());
        }
        Ok(Some(actual))
    }

    /// 只证明同会话已登记的直接命令有退出记录。
    pub(crate) fn verify_commands(&self) -> Result<Commands<'_>> {
        self.check()?;
        let paths = command_paths(&self.copy_root, self.session_id())?;
        for path in &paths {
            input_path(&self.backend, path, "复制命令记录")?;
        }
        let inputs = protect_inputs(&paths, "复制命令记录")?;
        for path in &paths {
            let value = json_io::read(path)?;
            if value["returncode"].as_i64().is_none() && value["returncode"].as_u64().is_none() {
                return Err("复制命令未登记明确整数退出结果，必须先核实现场".into());
            }
        }
        let result = Commands {
            owner: self,
            paths,
            inputs,
        };
        result.check()?;
        Ok(result)
    }
}

pub(crate) struct Commands<'a> {
    owner: &'a Owner,
    paths: Vec<PathBuf>,
    inputs: Inputs,
}

impl Commands<'_> {
    pub(crate) fn check(&self) -> Result<()> {
        self.owner.check()?;
        self.inputs.check()?;
        if command_paths(&self.owner.copy_root, self.owner.session_id())? != self.paths {
            return Err("复制会话的已登记命令文件集合变化".into());
        }
        Ok(())
    }
}

fn owner_scope(
    backend: &Path,
    selected: &Path,
    owner_path: &Path,
    value: &Value,
) -> Result<PathBuf> {
    json_io::exact(
        value,
        &[
            "format_version",
            "kind",
            "backend_root",
            "copy_directory",
            "session",
            "session_id",
            "source_export",
            "initialized",
            "plan_sha256",
            "generation_sha256",
            "controller",
        ],
        "复制锁持有人收据",
    )?;
    let recorded_backend = Path::new(json_io::text(&value["backend_root"], "复制锁后端")?);
    validate_absolute_path(recorded_backend, LocalTestPathKind::ExistingDirectory)?;
    let root = Path::new(json_io::text(&value["copy_directory"], "登记复制目录")?);
    let root = common::local_directory(backend, root, "登记复制目录")?;
    let owner_parent = owner_path.parent().ok_or("复制锁持有人收据缺少父目录")?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-copy-owner"
        || fs::canonicalize(recorded_backend)? != fs::canonicalize(backend)?
        || fs::canonicalize(&root)? != fs::canonicalize(selected)?
        || fs::canonicalize(owner_parent)? != fs::canonicalize(&root)?
    {
        return Err("复制锁持有人不属于明确的后端、复制目录与会话".into());
    }
    Ok(root)
}

struct Session {
    inputs: Inputs,
    binding: Value,
    target: PathBuf,
    plan_sha256: String,
    generation_sha256: String,
}

fn session(
    backend: &Path,
    root: &Path,
    registered: Option<&Value>,
    source_export: &Value,
    initialized: &Value,
) -> Result<Session> {
    let fixed = input_path(backend, &root.join("session.json"), "复制原会话")?;
    let session = if let Some(binding) = registered {
        let selected = bound_path(backend, binding, "复制原会话")?;
        if fs::canonicalize(&selected)? != fs::canonicalize(&fixed)? {
            return Err("复制锁持有人未绑定固定 session.json".into());
        }
        selected
    } else {
        fixed
    };
    let source = bound_path(backend, source_export, "复制来源导出")?;
    let target = bound_path(backend, initialized, "复制目标初始化")?;
    if target.file_name().and_then(|name| name.to_str()) != Some("initialized.json") {
        return Err("复制锁目标未绑定初始化收据".into());
    }
    let inputs = protect_inputs(&[session.clone(), source, target.clone()], "复制会话输入")?;
    let binding = if let Some(binding) = registered {
        common::bound_file(backend, binding, "复制原会话")?;
        binding.clone()
    } else {
        common::binding(&session)?
    };
    common::bound_file(backend, source_export, "复制来源导出")?;
    common::bound_file(backend, initialized, "复制目标初始化")?;
    let saved = json_io::read(&session)?;
    if saved["source_export"] != *source_export || saved["initialized"] != *initialized {
        return Err("复制锁来源、目标或计划与原会话不匹配".into());
    }
    let plan_sha256 = document::digest(&saved["plan_sha256"], "复制计划摘要")?;
    let generation_sha256 = document::digest(&saved["generation_sha256"], "复制代次摘要")?;
    inputs.check()?;
    Ok(Session {
        inputs,
        binding,
        target,
        plan_sha256,
        generation_sha256,
    })
}

fn bound_path(backend: &Path, value: &Value, label: &str) -> Result<PathBuf> {
    let binding = document::binding_shape(value, label)?;
    if binding.bytes > json_io::MAX_JSON_BYTES {
        return Err(format!("{label}的登记大小超过 16 MiB").into());
    }
    input_path(backend, &binding.path, label)
}

fn input_path(backend: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let path = common::common_file(backend, path, label)?;
    require_plain_input(&path, label)?;
    Ok(path)
}

fn protect_inputs(paths: &[PathBuf], label: &str) -> Result<Inputs> {
    let inputs = Inputs::open(paths.iter().cloned())?;
    for path in paths {
        require_plain_input(path, label)?;
    }
    Ok(inputs)
}

fn require_plain_input(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || common::link_count_for(path, &metadata)? != 1
    {
        return Err(format!("{label}必须是无链接的独立普通文件").into());
    }
    if metadata.len() > json_io::MAX_JSON_BYTES {
        return Err(format!("{label}超过 16 MiB").into());
    }
    Ok(())
}

fn controller_pid(controller: &Value) -> Result<u32> {
    controller["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
        .ok_or_else(|| "复制控制器 PID 无效".into())
}

fn controller_started(controller: &Value) -> Result<u64> {
    json_io::text(&controller["started"], "复制控制器创建代次")?
        .parse()
        .map_err(|_| "复制控制器创建代次超出内核时间范围".into())
}

fn lock_identity(path: &Path) -> Result<Option<(u64, u64)>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if digest::linked(&metadata) || !metadata.is_dir() {
        return Err("复制锁必须为真实目录".into());
    }
    Ok(Some(common::device_inode_for(path, &metadata)?))
}

fn restore_owner(path: &Path, owner_path: &Path, expected: &Value, raw: &[u8]) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let expected_id = (expected["device"].as_u64(), expected["inode"].as_u64());
    if !metadata.is_dir() || digest::linked(&metadata) || fs::read_dir(path)?.next().is_some() {
        return Err("锁目录类型或成员已变化，保留现场".into());
    }
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    if expected_id != (Some(device), Some(inode)) {
        return Err("锁目录文件身份变化，保留现场".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(path)?;
    temp.write_all(raw)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(owner_path)
        .map_err(|error| error.error)?;
    Ok(())
}

fn require_only_owner(path: &Path) -> Result<()> {
    let mut members = fs::read_dir(path)?;
    let first = members.next().transpose()?;
    if first.is_none_or(|entry| entry.file_name() != "owner.json")
        || members.next().transpose()?.is_some()
    {
        return Err("复制锁没有唯一的登记持有人，裸锁或其他会话锁必须核实现场".into());
    }
    Ok(())
}

fn command_paths(root: &Path, session_id: &str) -> Result<Vec<PathBuf>> {
    let prefix = format!("command-{session_id}-");
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.as_encoded_bytes();
        #[cfg(windows)]
        let folded = name.to_ascii_lowercase();
        #[cfg(windows)]
        let name = folded.as_slice();
        if name.starts_with(prefix.as_bytes()) && name.ends_with(b".json") {
            paths.push(entry.path());
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/copy_owner.rs"]
mod tests;
