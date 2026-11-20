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

/// 在现有复制账本互斥下核对未完成前缀。旧会话可已中断，但不能含未知写入或完成尝试。
pub(super) fn marker_prefix(
    backend: &Path,
    root: &Path,
    plan: &str,
    generation: &str,
    expected_prefix: Option<&Value>,
) -> Result<(Value, serde_json::Map<String, Value>)> {
    read_marker_journal(backend, root, plan, generation, |current, verified| {
        require_confirmed_marker_state(&verified.projection, &verified.events)?;
        let steps = verified.projection.steps;
        if let Some(prefix) = expected_prefix {
            let sequence = marker_sequence(&verified.frames, plan, generation, prefix)?;
            let prefix_state = model::project(&verified.events[..=sequence])?;
            let appended = &verified.events[(sequence + 1)..];
            if !confirmed_prefix(&prefix_state.steps, &steps, appended) {
                return Err("复制账本已偏离标记意图所绑定的原前缀".into());
            }
        }
        Ok((current, steps))
    })
}

pub(super) struct FailedMarkerJournal {
    head: Value,
    verified: VerifiedJournal,
}

impl FailedMarkerJournal {
    /// 仅供已验证 Redis 单独接续使用。原未知意图维持原样，供后续正式复制核对。
    pub(super) fn load(backend: &Path, root: &Path, plan: &str, generation: &str) -> Result<Self> {
        read_marker_journal(backend, root, plan, generation, |head, verified| {
            require_failed_reconcile(&verified)?;
            for entry in fs::read_dir(root)? {
                let name = entry?.file_name();
                if name
                    .to_str()
                    .is_some_and(|name| name.starts_with("complete-") && name.ends_with(".json"))
                {
                    return Err("复制账本已有完成收据，拒绝接续标记".into());
                }
            }
            Ok(Self { head, verified })
        })
    }

    pub(super) fn head(&self) -> &Value {
        &self.head
    }

    pub(super) fn steps(&self) -> &serde_json::Map<String, Value> {
        &self.verified.projection.steps
    }

    pub(super) fn session_id(&self) -> Result<&str> {
        self.verified
            .projection
            .session
            .as_ref()
            .map(|session| session.id.as_str())
            .ok_or_else(|| "原失败复制核对缺少会话".into())
    }

    pub(super) fn verify_previous(&self, prefix: &Value) -> Result<()> {
        let plan = document::text(&self.head["plan_sha256"], "原复制计划摘要")?;
        let generation = document::text(&self.head["generation_sha256"], "原复制代次摘要")?;
        let sequence = marker_sequence(&self.verified.frames, plan, generation, prefix)?;
        let events = &self.verified.events[..=sequence];
        let previous = model::project(events)?;
        require_confirmed_marker_state(&previous, events)?;
        if !same_confirmed_steps(&previous.steps, self.steps()) {
            return Err("已确认的原复制步骤在续跑账本中发生变化".into());
        }
        Ok(())
    }
}

fn require_failed_reconcile(verified: &VerifiedJournal) -> Result<()> {
    let projection = &verified.projection;
    if projection
        .session
        .as_ref()
        .is_none_or(|session| session.mode != "reconcile" || session.outcome != "failed")
        || verified.events.last()
            != Some(&json!({
                "type":"closed","outcome":"failed","completed_at":null,
            }))
        || has_publication(&verified.events)
    {
        return Err("原复制账本最新会话不是已关闭的失败只读核对，或已有完成发布".into());
    }
    let mut unknown = projection
        .steps
        .values()
        .filter(|step| step["phase"] != "confirmed");
    let open = unknown
        .next()
        .ok_or("原复制账本没有待显式核对的开放对象意图")?;
    if unknown.next().is_some()
        || projection.steps.len() < 2
        || open["type"] != "intent"
        || open["phase"] != "unknown"
        || open["attempt"] != 1
        || open["before"]["kind"] != "object"
        || open["after"]["kind"] != "object"
    {
        return Err("原复制账本只能保留一份未确认的原始对象写入意图".into());
    }
    model::image_pair(&open["before"], &open["after"])
}

fn require_confirmed_marker_state(projection: &model::Projection, events: &[Value]) -> Result<()> {
    if projection
        .session
        .as_ref()
        .is_none_or(|session| !matches!(session.outcome.as_str(), "open" | "partial"))
        || has_publication(events)
        || projection.steps.is_empty()
        || projection
            .steps
            .values()
            .any(|step| step["phase"] != "confirmed")
    {
        return Err("Redis 标记恢复只接受有完整确认且无未知写入的未完成复制前缀".into());
    }
    Ok(())
}

fn has_publication(events: &[Value]) -> bool {
    events
        .iter()
        .any(|event| event["type"] == "closed" && event["outcome"] == "publishing")
}

fn marker_sequence(
    frames: &[Value],
    plan: &str,
    generation: &str,
    prefix: &Value,
) -> Result<usize> {
    let sequence = prefix["sequence"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or("原标记意图缺少复制前缀序号")?;
    let prior = frames
        .get(sequence)
        .ok_or("复制账本缺少标记意图所绑定的原前缀")?;
    if *prefix != head(plan, generation, sequence, &prior["sha256"]) {
        return Err("复制账本已偏离标记意图所绑定的原前缀".into());
    }
    Ok(sequence)
}

fn read_marker_journal<T>(
    backend: &Path,
    root: &Path,
    plan: &str,
    generation: &str,
    inspect: impl FnOnce(Value, VerifiedJournal) -> Result<T>,
) -> Result<T> {
    let root = common::local_directory(backend, root, "原复制账本")?;
    if lock_identity(&root)?.is_some() {
        return Err("Redis 标记恢复要求原复制账本没有写入持有人".into());
    }
    let mut reads = Reads {
        backend,
        files: Vec::new(),
    };
    let raw = reads.bytes(&root.join("events.ndjson"), document::MAX_JSON_BYTES)?;
    let head_raw = reads.bytes(&root.join("head.json"), HEAD_LIMIT)?;
    let verified = verify_journal(&raw, &head_raw, plan, generation)?;
    let result = inspect(checked(&head_raw)?, verified)?;
    reads.unchanged()?;
    if lock_identity(&root)?.is_some() {
        return Err("Redis 标记核验期间出现新的复制账本持有人".into());
    }
    Ok(result)
}

fn confirmed_prefix(
    before: &serde_json::Map<String, Value>,
    current: &serde_json::Map<String, Value>,
    appended: &[Value],
) -> bool {
    if before.len() != current.len()
        || appended.iter().any(|event| match event["type"].as_str() {
            Some("opened") => event["mode"] != "reconcile",
            Some("reconciled") => before
                .get(event["step_id"].as_str().unwrap_or_default())
                .is_none_or(|step| event["observed"] != step["after"]),
            Some("closed") => event["outcome"] != "partial",
            _ => true,
        })
    {
        return false;
    }
    same_confirmed_steps(before, current)
}

fn same_confirmed_steps(
    before: &serde_json::Map<String, Value>,
    current: &serde_json::Map<String, Value>,
) -> bool {
    before.iter().all(|(id, previous)| {
        previous["phase"] == "confirmed"
            && current.get(id).is_some_and(|now| {
                ["before", "after", "artifact_sha256", "attempt", "phase"]
                    .iter()
                    .all(|field| now[*field] == previous[*field])
            })
    })
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
