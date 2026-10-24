//! 只读核对复制事件链和原完成收据，不取得写锁，不修补或重放资源操作。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{digest, input::digest_shape, json as document, resources::common};
use crate::Result;

#[path = "copy_ledger/model.rs"]
pub(crate) mod model;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/copy_ledger.rs"]
mod tests;

struct Reads<'a> {
    backend: &'a Path,
    files: Vec<(PathBuf, Value, (u64, u64))>,
}

impl Reads<'_> {
    fn bytes(&mut self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        common::common_file(self.backend, path, "复制账本证据")?;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.len() > limit {
            return Err("复制账本证据超过既定读取上限".into());
        }
        let identity = common::device_inode_for(path, &metadata)?;
        let binding = common::binding(path)?;
        let raw = fs::read(path)?;
        if raw.len() as u64 > limit
            || binding["bytes"] != raw.len() as u64
            || binding["sha256"] != hex::encode(Sha256::digest(&raw))
        {
            return Err("复制账本证据读取期间变化".into());
        }
        self.files.push((path.to_owned(), binding, identity));
        self.unchanged()?;
        Ok(raw)
    }

    fn unchanged(&self) -> Result<()> {
        for (path, expected, identity) in &self.files {
            common::common_file(self.backend, path, "复制账本证据")?;
            if common::binding(path)? != *expected
                || common::device_inode_for(path, &fs::symlink_metadata(path)?)? != *identity
            {
                return Err("复制账本核验期间文件变化；保留现场，禁止自动重放".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn inspect(
    backend: &Path,
    directory: &Path,
    plan: &str,
    generation: &str,
) -> Result<Value> {
    digest_shape(&json!(plan))?;
    digest_shape(&json!(generation))?;
    let root = common::local_directory(backend, directory, "复制账本目录")?;
    let root_identity = common::device_inode_for(&root, &fs::symlink_metadata(&root)?)?;
    let lock = lock_identity(&root)?;
    let mut reads = Reads {
        backend,
        files: Vec::new(),
    };
    let raw = reads.bytes(&root.join("events.ndjson"), document::MAX_JSON_BYTES)?;
    let (frames, events) = journal(&raw)?;
    let last = frames.last().ok_or("复制账本事件链不能为空")?;
    let binding = json!({"plan_sha256":plan,"generation_sha256":generation});
    let expected_head = json!({"format_version":1,"plan_sha256":plan,
        "generation_sha256":generation,"sequence":frames.len()-1,"sha256":last["sha256"]});
    if checked(&reads.bytes(&root.join("head.json"), 4096)?)? != expected_head
        || events[0] != json!({"type":"created","plan_sha256":plan,"generation_sha256":generation})
    {
        return Err("计划、代次或账本 head 不匹配；禁止自动截断补写".into());
    }
    let projected = model::project(&events)?;
    let session = projected.session.as_ref().ok_or("复制账本缺少已登记会话")?;
    let receipt = completion(
        &root,
        &binding,
        last,
        &projected,
        lock.is_some(),
        &mut reads,
    )?;
    let uncertain = lock.is_some()
        || matches!(session.outcome.as_str(), "open" | "failed" | "publishing")
        || projected
            .steps
            .values()
            .any(|step| step["phase"] != "confirmed");
    reads.unchanged()?;
    if lock_identity(&root)? != lock
        || common::device_inode_for(&root, &fs::symlink_metadata(&root)?)? != root_identity
    {
        return Err("复制账本核验期间目录或锁身份变化；保留现场".into());
    }
    Ok(json!({"plan_sha256":plan,"generation_sha256":generation,
        "status":if receipt.is_some(){"ledger_evidence_complete"}else if uncertain{"needs_reconciliation"}else{"partial"},
        "steps":projected.steps,"target_ready":false,
        "completed_at":receipt.map(|v|v["completed_at"].clone()).unwrap_or(Value::Null)}))
}

fn completion(
    root: &Path,
    binding: &Value,
    last: &Value,
    projected: &model::Projection,
    locked: bool,
    reads: &mut Reads<'_>,
) -> Result<Option<Value>> {
    let session = projected.session.as_ref().ok_or("复制账本缺少会话")?;
    if session.outcome != "publishing" || locked {
        return Ok(None);
    }
    let path = root.join(format!("complete-{}.json", session.id));
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let receipt = checked(&reads.bytes(&path, document::MAX_JSON_BYTES)?)?;
    if projected.steps.is_empty()
        || session.mode == "reconcile"
            && projected
                .steps
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                != session.touched
    {
        return Err(
            "账本完成收据不满足原发布条件：必须有已确认步骤，核对会话必须覆盖全部步骤".into(),
        );
    }
    let expected = json!({"format_version":1,"status":"ledger_evidence_complete",
        "plan_sha256":binding["plan_sha256"],"generation_sha256":binding["generation_sha256"],
        "session":session.id,"journal_sha256":last["sha256"],"steps":projected.steps.len(),
        "completed_at":session.completed_at,"target_ready":false,"restore_success":false,"runtime_performance_passed":false});
    if receipt != expected
        || projected
            .steps
            .values()
            .any(|step| step["phase"] != "confirmed")
    {
        return Err("完成收据与账本后像或计划代次不一致".into());
    }
    Ok(Some(receipt))
}

fn journal(raw: &[u8]) -> Result<(Vec<Value>, Vec<Value>)> {
    if raw.is_empty() || !raw.ends_with(b"\n") {
        return Err("账本已截断，禁止自动修复".into());
    }
    let mut frames = Vec::new();
    let mut events = Vec::new();
    let mut previous = "0".repeat(64);
    for (sequence, line) in raw.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let frame = checked(line)?;
        document::exact(
            &frame,
            &["sequence", "previous", "event", "sha256"],
            "复制账本帧",
        )?;
        let body = json!({"sequence":frame["sequence"],"previous":frame["previous"],"event":frame["event"]});
        if frame["sequence"].as_u64() != Some(sequence as u64)
            || frame["previous"] != previous
            || frame["sha256"] != digest::plan_hash(&body)?
        {
            return Err("账本事件顺序或摘要链被篡改".into());
        }
        previous = document::text(&frame["sha256"], "账本帧摘要")?.to_owned();
        events.push(frame["event"].clone());
        frames.push(frame);
    }
    Ok((frames, events))
}

fn checked(raw: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(raw).map_err(|_| "账本必须是规范 UTF-8 JSON")?;
    let value = document::parse_fragment(text, "复制账本")?;
    document::object(&value, "复制账本")?;
    let mut encoded = serde_json::to_vec(&value)?;
    encoded.push(b'\n');
    if encoded != raw {
        return Err("账本 JSON 非规范格式、重复字段或已截断".into());
    }
    Ok(value)
}

fn lock_identity(root: &Path) -> Result<Option<(u64, u64)>> {
    let path = root.join("clone.lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if digest::linked_path(&path)?.iter().any(|value| *value) {
                return Err("复制账本锁路径不能经过链接".into());
            }
            Ok(Some(common::device_inode_for(&path, &metadata)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
