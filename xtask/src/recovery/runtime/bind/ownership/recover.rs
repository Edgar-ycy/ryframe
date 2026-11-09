//! 在原登记内核互斥内归档已结束 owner，并把同一 guard 交给 Recover。
use super::{
    Owner, ProcessGuard, REGISTRATION_GUARD, REGISTRATION_LOCK, common, directory_id, file_id, fs,
    identity, json, new_receipt, only_owner_entry, owner_bytes, parse_owner, publish_verified,
    strict,
};
use crate::{Result, recovery::runtime::verify::same_path};
use serde_json::Value;
use std::path::{Path, PathBuf};

const RECOVERY_KIND: &str = "restore-runtime-registration-reconciliation";
const RECOVERY_PREFIX: &str = "restore-registration-reconcile";

impl Owner {
    pub(in crate::recovery::runtime) fn recover_with<P, T>(
        control: &Path,
        operation: &str,
        expected_owner_path: &Path,
        allowed_operations: &[String],
        precheck: impl FnOnce(&Value) -> Result<P>,
        body: impl FnOnce(&Self, Value, P) -> Result<T>,
    ) -> Result<T> {
        if !operation.starts_with("runtime-recover:")
            || !allowed_operations
                .iter()
                .any(|allowed| allowed == operation)
        {
            return Err("控制恢复必须绑定同一登记的明确 recover 操作".into());
        }
        let guard = ProcessGuard::acquire_named(control, REGISTRATION_GUARD)?;
        let stale = Stale::read(control, expected_owner_path, &guard, allowed_operations)?;
        let observed = stale.observe()?;
        let prepared = precheck(&stale.receipt)?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let replacement = new_receipt(control, operation, &token)?;
        let recovery = stale.exchange(&guard, observed, &token, &replacement)?;
        let owner = Self {
            guard,
            directory: stale.directory,
            directory_identity: stale.directory_id,
            file_identity: unique_file_id(&stale.path)?,
            receipt: replacement,
        };
        owner.check()?;
        if Self::prior_recovered_owner(control, &owner.receipt)? != stale.receipt {
            return Err("恢复 owner 的前驱归档与原 owner 不同，保留现场".into());
        }
        match body(&owner, recovery, prepared) {
            Ok(value) => owner.release().map(|()| value),
            Err(error) => {
                let context = match owner.check() {
                    Ok(()) => "新恢复 owner 已保留，请使用同一显式 owner 路径继续核对".to_owned(),
                    Err(after) => format!("新恢复 owner 后验失败，保留现场：{after}"),
                };
                Err(format!("{error}；{context}").into())
            }
        }
    }

    /// 首代状态还未物化时，读取当前 Rust Recover token 唯一指向的前驱。
    pub(in crate::recovery::runtime) fn prior_recovered_owner(
        control: &Path,
        current: &Value,
    ) -> Result<Value> {
        validate(current, control)?;
        if !strict::text(&current["operation"], "当前恢复操作")?.starts_with("runtime-recover:")
        {
            return Err("只有同一登记的 Recover owner 可读取前驱".into());
        }
        let token = strict::text(&current["token"], "当前恢复 token")?;
        let path = archive_path(control, token);
        let before = unique_file_id(&path)?;
        if fs::symlink_metadata(&path)?.len() > 256 * 1024 {
            return Err("前驱恢复归档超过 256 KiB，保留现场".into());
        }
        let archive = strict::read(&path)?;
        strict::exact(
            &archive,
            &[
                "format_version",
                "kind",
                "owner",
                "observed_identity",
                "remote_writes_reconciled",
            ],
            "原恢复前驱归档",
        )?;
        if archive["format_version"].as_u64() != Some(1)
            || archive["kind"] != RECOVERY_KIND
            || archive["remote_writes_reconciled"] != false
        {
            return Err("前驱归档不属于原恢复控制或远端写入证明不明确".into());
        }
        let prior = &archive["owner"];
        validate(prior, control)?;
        if prior["token"] == current["token"] {
            return Err("原恢复前驱 token 与当前 owner 相同，拒绝循环".into());
        }
        let actual = &archive["observed_identity"];
        stopped_identity(
            &prior["identity"],
            (!actual.is_null()).then(|| actual.clone()),
        )?;
        if unique_file_id(&path)? != before || strict::read(&path)? != archive {
            return Err("前驱恢复归档在核验期间变化，保留现场".into());
        }
        Ok(prior.clone())
    }
}

struct Stale<'a> {
    control: &'a Path,
    selected: &'a Path,
    directory: PathBuf,
    path: PathBuf,
    directory_id: (u64, u64),
    file_id: (u64, u64),
    raw: Vec<u8>,
    receipt: Value,
    pid: u32,
}

impl<'a> Stale<'a> {
    fn read(
        control: &'a Path,
        selected: &'a Path,
        guard: &ProcessGuard,
        allowed: &[String],
    ) -> Result<Self> {
        let directory = control.join(REGISTRATION_LOCK);
        let path = directory.join("owner.json");
        if !same_path(selected, &path) {
            return Err("控制恢复必须显式绑定全局恢复运行 owner 文件".into());
        }
        guard.check_named(control, REGISTRATION_GUARD)?;
        let directory_id = directory_id(&directory)?;
        only_owner_entry(&directory)?;
        let file_id = unique_file_id(&path)?;
        let raw = owner_bytes(&path)?;
        let receipt = parse_owner(&raw)?;
        let pid = validate(&receipt, control)?;
        let operation = strict::text(&receipt["operation"], "原恢复运行操作")?;
        if !allowed.iter().any(|candidate| candidate == operation) {
            return Err("原控制 owner 不属于同一登记的明确操作".into());
        }
        let stale = Self {
            control,
            selected,
            directory,
            path,
            directory_id,
            file_id,
            raw,
            receipt,
            pid,
        };
        stale.check(guard)?;
        Ok(stale)
    }

    fn check(&self, guard: &ProcessGuard) -> Result<()> {
        guard.check_named(self.control, REGISTRATION_GUARD)?;
        if directory_id(&self.directory)? != self.directory_id
            || unique_file_id(&self.path)? != self.file_id
            || file_id(self.selected)? != self.file_id
        {
            return Err("原控制锁目录或显式 owner 的物理身份变化，保留现场".into());
        }
        only_owner_entry(&self.directory)?;
        if owner_bytes(&self.path)? != self.raw
            || unique_file_id(&self.path)? != self.file_id
            || file_id(self.selected)? != self.file_id
            || directory_id(&self.directory)? != self.directory_id
        {
            return Err("原控制锁或 owner 原始内容变化，保留现场".into());
        }
        guard.check_named(self.control, REGISTRATION_GUARD)
    }

    fn observe(&self) -> Result<Option<Value>> {
        stopped_identity(&self.receipt["identity"], identity::process(self.pid)?)
    }

    fn exchange(
        &self,
        guard: &ProcessGuard,
        observed: Option<Value>,
        token: &str,
        replacement: &Value,
    ) -> Result<Value> {
        let output = archive_path(self.control, token);
        let mut result = json!({
            "format_version":1, "kind":RECOVERY_KIND, "owner":self.receipt,
            "observed_identity":observed, "remote_writes_reconciled":false,
        });
        let unchanged = || {
            self.check(guard)?;
            Ok(self.observe()? == observed)
        };
        publish_verified(&output, &result, unchanged)?;
        if !unchanged()? {
            return Err("原 owner 的内核观察在回收收据发布后变化，保留现场".into());
        }
        strict::write_replace(&self.path, replacement)?;
        if directory_id(&self.directory)? != self.directory_id
            || unique_file_id(&self.path)? == self.file_id
        {
            return Err("恢复 owner 原子接替后的文件或锁目录身份无效，保留现场".into());
        }
        guard.check_named(self.control, REGISTRATION_GUARD)?;
        result["receipt"] = json!(output);
        Ok(result)
    }
}

fn archive_path(control: &Path, token: &str) -> PathBuf {
    control.join(format!("{RECOVERY_PREFIX}-{token}.json"))
}

fn unique_file_id(path: &Path) -> Result<(u64, u64)> {
    let id = file_id(path)?;
    if common::link_count_for(path, &fs::symlink_metadata(path)?)? != 1 {
        return Err("原运行控制 owner 必须是独占普通文件".into());
    }
    Ok(id)
}

fn validate(value: &Value, control: &Path) -> Result<u32> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "identity",
            "runtime_directory",
            "operation",
            "token",
        ],
        "原恢复运行 owner",
    )?;
    let operation = strict::text(&value["operation"], "原恢复运行操作")?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != super::OWNER_KIND
        || value["runtime_directory"] != json!(control)
        || operation.is_empty()
        || !value["token"].as_str().is_some_and(|token| {
            token.len() == 32
                && token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
    {
        return Err("原控制 owner 不属于同一登记、明确操作或固定运行目录".into());
    }
    let (pid, _) = creation(&value["identity"])?;
    Ok(pid)
}

fn creation(process: &Value) -> Result<(u32, u64)> {
    common::validate_process_identity(process)?;
    let pid = u32::try_from(process["pid"].as_u64().ok_or("原 owner PID 无效")?)?;
    let started = process["started"].as_str().ok_or("原 owner 创建时间无效")?;
    let number = started
        .parse::<u64>()
        .map_err(|_| "原 owner 创建时间超出内核整数范围")?;
    if number.to_string() != started {
        return Err("原 owner 创建时间不是规范内核十进制文本".into());
    }
    Ok((pid, number))
}

fn stopped_identity(expected: &Value, observed: Option<Value>) -> Result<Option<Value>> {
    let (expected_pid, expected_started) = creation(expected)?;
    let Some(actual) = observed else {
        return Ok(None);
    };
    let (actual_pid, actual_started) = creation(&actual)?;
    if actual == *expected {
        return Err("原运行控制器仍在运行，禁止释放控制锁".into());
    }
    if actual_pid != expected_pid || actual_started == expected_started {
        return Err("PID 观察不能证明不同的创建身份，保留原控制锁".into());
    }
    Ok(Some(actual))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/runtime/bind/ownership.rs"]
mod tests;
