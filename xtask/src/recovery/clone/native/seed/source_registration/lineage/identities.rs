//! 完整创建代次的合并与确定性投影；不查询活体进程，也不缩减历史。
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use crate::{Result, recovery::clone::native::json as documents};

type Creation = (u64, u128);
type Occurrence = (u8, usize, String, u64, String);

struct Entry {
    identity: Value,
    occurrences: BTreeMap<Occurrence, Value>,
}

#[derive(Default)]
pub(crate) struct Identities {
    entries: BTreeMap<Creation, Entry>,
}

impl Identities {
    /// rank 由父收集器按同一 run 下的固定 ASCII 相对目录顺序生成。
    pub(crate) fn runtime(
        &mut self,
        identity: &Value,
        runtime: &Path,
        rank: usize,
        role: &str,
        sequence: u64,
    ) -> Result<()> {
        if !runtime.is_absolute() || !matches!(role, "api" | "worker") {
            return Err("历史运行出现无效目录或角色".into());
        }
        let directory = runtime.to_str().ok_or("历史运行目录必须为 UTF-8")?;
        self.record(
            identity,
            (1, rank, role.into(), sequence, String::new()),
            json!({"kind":"runtime","runtime_directory":directory,
                "role":role,"sequence":sequence}),
        )
    }

    pub(crate) fn producer(&mut self, identity: &Value, attempt: u64, kind: &str) -> Result<()> {
        if attempt == 0 || kind.is_empty() {
            return Err("历史生产者缺少明确 attempt 或类别".into());
        }
        self.record(
            identity,
            (0, 0, String::new(), attempt, kind.into()),
            json!({"kind":"node","attempt":attempt,"producer_kind":kind}),
        )
    }

    fn record(&mut self, identity: &Value, key: Occurrence, occurrence: Value) -> Result<()> {
        let creation = creation(identity)?;
        let entry = self.entries.entry(creation).or_insert_with(|| Entry {
            identity: identity.clone(),
            occurrences: BTreeMap::new(),
        });
        if entry.identity != *identity {
            return Err("同一 PID 创建代次登记了不同可执行文件".into());
        }
        if let Some(previous) = entry.occurrences.get(&key) {
            if previous != &occurrence {
                return Err("历史出现位置的权威排序键对应不同证据".into());
            }
        } else {
            entry.occurrences.insert(key, occurrence);
        }
        Ok(())
    }

    pub(crate) fn into_value(self) -> Value {
        Value::Array(
            self.entries
                .into_values()
                .map(|entry| {
                    json!({"identity":entry.identity,
                        "occurrences":entry.occurrences.into_values().collect::<Vec<_>>()})
                })
                .collect(),
        )
    }
}

fn creation(value: &Value) -> Result<Creation> {
    documents::exact(value, &["pid", "started", "executable"], "历史创建身份")?;
    let pid = value["pid"]
        .as_u64()
        .filter(|value| *value > 1)
        .ok_or("历史生产者 PID 无效")?;
    let started = documents::text(&value["started"], "历史创建时间")?;
    if started.is_empty()
        || started.len() > 32
        || !(b'1'..=b'9').contains(&started.as_bytes()[0])
        || !started.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("历史创建时间必须是 1 至 32 位 ASCII 正整数字符串".into());
    }
    let executable = documents::text(&value["executable"], "历史可执行文件")?;
    if !Path::new(executable).is_absolute() {
        return Err("历史可执行文件必须使用绝对路径".into());
    }
    let started = started
        .parse::<u128>()
        .map_err(|_| "历史创建时间超出完整比较范围")?;
    Ok((pid, started))
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/source_registration/lineage/identities.rs"]
mod tests;
