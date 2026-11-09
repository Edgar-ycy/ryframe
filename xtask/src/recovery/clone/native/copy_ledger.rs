//! 复制账本的持久证据、会话与独立只读核验。
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{digest, input::digest_shape, json as document, resources::common};
use crate::Result;

#[path = "copy_ledger/model.rs"]
pub(crate) mod model;
#[path = "copy_ledger/session.rs"]
pub(super) mod session;
#[path = "copy_ledger/storage.rs"]
pub(super) mod storage;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/clone/native/copy_ledger.rs"]
mod tests;

struct Reads<'a> {
    backend: &'a Path,
    files: Vec<(PathBuf, u64, String, (u64, u64))>,
}

impl Reads<'_> {
    fn bytes(&mut self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        let image = read_image(self.backend, path, limit)?;
        self.files.push((
            path.to_owned(),
            limit,
            hex::encode(Sha256::digest(&image.raw)),
            image.identity,
        ));
        self.unchanged()?;
        Ok(image.raw)
    }

    fn unchanged(&self) -> Result<()> {
        for (path, limit, expected, identity) in &self.files {
            let current = read_image(self.backend, path, *limit)?;
            if current.identity != *identity
                || hex::encode(Sha256::digest(&current.raw)) != *expected
            {
                return Err("复制账本核验期间文件变化；保留现场，禁止自动重放".into());
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
struct Image {
    raw: Vec<u8>,
    identity: (u64, u64),
}

fn read_image(backend: &Path, path: &Path, limit: u64) -> Result<Image> {
    common::common_file(backend, path, "复制账本证据")?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.len() > limit {
        return Err("复制账本证据超过既定读取上限".into());
    }
    let identity = common::device_inode_for(path, &metadata)?;
    let file = fs::File::open(path)?;
    let opened = same_file::Handle::from_file(file.try_clone()?)?;
    if opened != same_file::Handle::from_path(path)? {
        return Err("复制账本证据打开期间被替换".into());
    }
    let mut raw = Vec::new();
    (&file).take(limit + 1).read_to_end(&mut raw)?;
    let after = file.metadata()?;
    if raw.len() as u64 != metadata.len()
        || after.len() != metadata.len()
        || metadata.modified()? != after.modified()?
        || opened != same_file::Handle::from_path(path)?
        || identity != common::device_inode_for(path, &fs::symlink_metadata(path)?)?
    {
        return Err("复制账本证据读取期间变化".into());
    }
    Ok(Image { raw, identity })
}

pub(crate) fn inspect(
    backend: &Path,
    directory: &Path,
    plan: &str,
    generation: &str,
) -> Result<Value> {
    Ok(inspect_state(backend, directory, plan, generation)?.0)
}

pub(super) fn check_continuation(
    backend: &Path,
    directory: &Path,
    plan: &str,
    generation: &str,
    mode: session::Mode,
) -> Result<()> {
    let root = common::local_directory(backend, directory, "原复制账本")?;
    if mode == session::Mode::New || lock_identity(&root)?.is_some() {
        return Err("原复制账本已持锁或模式无效，必须先核对登记的原控制器".into());
    }
    let (evidence, outcome) = inspect_state(backend, &root, plan, generation)?;
    if lock_identity(&root)?.is_some() {
        return Err("原复制账本在续跑预检期间取得了新锁".into());
    }
    if mode == session::Mode::Resume
        && (outcome != "partial"
            || evidence["steps"].as_object().is_none_or(|steps| {
                steps.values().any(|step| {
                    !matches!(
                        step["phase"].as_str(),
                        Some("confirmed" | "reconciled_before")
                    )
                })
            }))
    {
        return Err("原复制会话未正常关闭或有未知写入，必须先显式核对".into());
    }
    Ok(())
}

fn inspect_state(
    backend: &Path,
    directory: &Path,
    plan: &str,
    generation: &str,
) -> Result<(Value, String)> {
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
    let head_raw = reads.bytes(&root.join("head.json"), HEAD_LIMIT)?;
    let VerifiedJournal {
        frames,
        projection: projected,
        ..
    } = verify_journal(&raw, &head_raw, plan, generation)?;
    let last = frames.last().ok_or("复制账本事件链不能为空")?;
    let binding = json!({"plan_sha256":plan,"generation_sha256":generation});
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
    let evidence = json!({"plan_sha256":plan,"generation_sha256":generation,
        "status":if receipt.is_some(){"ledger_evidence_complete"}else if uncertain{"needs_reconciliation"}else{"partial"},
        "steps":projected.steps,"target_ready":false,
        "completed_at":receipt.map(|v|v["completed_at"].clone()).unwrap_or(Value::Null)});
    Ok((evidence, session.outcome.clone()))
}

const HEAD_LIMIT: u64 = 4096;

struct VerifiedJournal {
    frames: Vec<Value>,
    events: Vec<Value>,
    projection: model::Projection,
}

fn verify_journal(
    raw: &[u8],
    head_raw: &[u8],
    plan: &str,
    generation: &str,
) -> Result<VerifiedJournal> {
    let (frames, events) = journal(raw)?;
    let last = frames.last().ok_or("复制账本事件链不能为空")?;
    if checked(head_raw)? != head(plan, generation, frames.len() - 1, &last["sha256"])
        || events.first() != Some(&created(plan, generation))
    {
        return Err("计划、代次或账本 head 不匹配；禁止自动截断补写".into());
    }
    let projection = model::project(&events)?;
    Ok(VerifiedJournal {
        frames,
        events,
        projection,
    })
}

fn head(plan: &str, generation: &str, sequence: usize, sha256: &Value) -> Value {
    json!({"format_version":1,"plan_sha256":plan,"generation_sha256":generation,
        "sequence":sequence,"sha256":sha256})
}

fn created(plan: &str, generation: &str) -> Value {
    json!({"type":"created","plan_sha256":plan,"generation_sha256":generation})
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
    if receipt != completion_value(binding, &last["sha256"], projected)? {
        return Err("完成收据与账本后像或计划代次不一致".into());
    }
    Ok(Some(receipt))
}

fn require_complete(projected: &model::Projection) -> Result<&model::Session> {
    let session = projected.session.as_ref().ok_or("复制账本缺少会话")?;
    if projected.steps.is_empty()
        || projected
            .steps
            .values()
            .any(|step| step["phase"] != "confirmed")
        || session.mode == "reconcile"
            && (session.touched.len() != projected.steps.len()
                || projected
                    .steps
                    .keys()
                    .any(|step| !session.touched.contains(step)))
    {
        return Err("完成收据需要完整已确认步骤，核对会话必须覆盖全部步骤".into());
    }
    Ok(session)
}

fn completion_value(
    binding: &Value,
    journal_sha256: &Value,
    projected: &model::Projection,
) -> Result<Value> {
    let session = require_complete(projected)?;
    if session.outcome != "publishing" {
        return Err("复制账本必须先持锁关闭会话".into());
    }
    Ok(
        json!({"format_version":1,"status":"ledger_evidence_complete",
        "plan_sha256":binding["plan_sha256"],"generation_sha256":binding["generation_sha256"],
        "session":session.id,"journal_sha256":journal_sha256,"steps":projected.steps.len(),
        "completed_at":session.completed_at,"target_ready":false,"restore_success":false,"runtime_performance_passed":false}),
    )
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
    if encoded(&value)? != raw {
        return Err("账本 JSON 非规范格式、重复字段或已截断".into());
    }
    Ok(value)
}

fn encoded(value: &Value) -> Result<Vec<u8>> {
    let mut raw = serde_json::to_vec(value)?;
    raw.push(b'\n');
    Ok(raw)
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
