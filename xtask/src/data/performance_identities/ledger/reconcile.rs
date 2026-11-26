use super::{Ledger, RunLock, evidence, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use same_file::Handle;
use serde_json::{Value, json};
use std::path::Path;

#[cfg(test)]
#[path = "../../../../tests/unit/data/performance_identities/ledger/reconcile_tests.rs"]
mod tests;

const OPERATION: &str = "post_system_users";
const DIRECTORY: &str = "first-create-reconciliation";
const INTENT: &str = "first-create-reconciliation-intent.json";
const ARCHIVE: &str = "ledger-before.json";
const RECEIPT: &str = "first-create-reconciliation-result.json";

impl Ledger {
    #[expect(clippy::too_many_arguments, reason = "保持原账本与本次核对输入显式")]
    pub(in crate::data::performance_identities) fn reconcile_first_create(
        plan: &Value,
        root: &Path,
        output: &Path,
        expected_before_sha256: &str,
        slot: &str,
        user_id: &str,
        authority: &Value,
        check: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Value> {
        let backend = Path::new(
            plan["environment"]["backend_dir"]
                .as_str()
                .ok_or("身份计划缺少后端路径")?,
        );
        validate_local_test_path(root, backend, LocalTestPathKind::ExistingDirectory)?;
        validate_local_test_path(output, backend, LocalTestPathKind::ExistingDirectory)?;
        if output != root.join(DIRECTORY) {
            return Err("首个用户和解必须使用原身份账本的固定证据目录".into());
        }
        let root_id = Handle::from_path(root)?;
        let output_id = Handle::from_path(output)?;
        check()?;
        let token = uuid::Uuid::new_v4().to_string();
        let lock = RunLock::acquire(root, plan, &token)?;
        let result = (|| {
            let lock_path = root.join("lock");
            let lock_id = Handle::from_path(&lock_path)?;
            let lock_bytes = read_required(&lock_path)?;
            if evidence::parse(&lock_bytes)?["owner_token"] != token {
                return Err("首个用户和解的原身份账本锁发生变化".into());
            }
            let mut guard = || {
                check()?;
                if Handle::from_path(root)? != root_id
                    || Handle::from_path(output)? != output_id
                    || Handle::from_path(&lock_path)? != lock_id
                    || read_required(&lock_path)? != lock_bytes
                {
                    return Err("首个用户和解期间身份账本目录、证据目录或锁发生变化".into());
                }
                Ok(())
            };
            guard()?;
            reconcile_locked(
                plan,
                root,
                output,
                expected_before_sha256,
                slot,
                user_id,
                authority,
                &mut guard,
            )
        })();
        let release = lock.release();
        match (result, release) {
            (Ok(receipt), Ok(())) => Ok(receipt),
            (Err(error), Err(release_error)) => {
                Err(format!("{error}；身份账本锁释放也失败：{release_error}").into())
            }
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        }
    }
}

pub(super) fn require_finalized(ledger: &Ledger) -> Result<()> {
    let output = ledger.root.join(DIRECTORY);
    let intent_path = ledger.root.join(INTENT);
    let Some(raw_intent) = evidence::regular(&intent_path)? else {
        if output.try_exists()?
            || evidence::regular(&intent_path.with_extension("json.pending"))?.is_some()
        {
            return Err("首个用户和解缺少已发布意图，禁止继续身份写入".into());
        }
        return Ok(());
    };
    let backend = Path::new(
        ledger.plan["environment"]["backend_dir"]
            .as_str()
            .ok_or("身份计划缺少后端路径")?,
    );
    validate_local_test_path(&output, backend, LocalTestPathKind::ExistingDirectory)?;
    let intent = evidence::parse(&raw_intent)?;
    let slot = intent["slot"].as_str().ok_or("首个用户和解缺少身份位置")?;
    let user_id = state::id(&intent["user_id"])?;
    let before = read_required(&output.join(ARCHIVE))?;
    let after = project_first_create(&ledger.plan, &evidence::parse(&before)?, slot, user_id)?;
    let expected = make_intent(
        &ledger.plan,
        &ledger.root,
        &intent["authority"],
        (slot, user_id),
        &before,
        &evidence::encode(&after)?,
    );
    let entries = ledger.state["entries"]
        .as_array()
        .ok_or("身份账本缺少操作序列")?;
    let prefix = after["entries"]
        .as_array()
        .ok_or("首个用户和解缺少操作序列")?;
    if raw_intent != evidence::encode(&expected)? || !entries.starts_with(prefix) {
        return Err("首个用户和解与已登记创建不同，禁止继续身份写入".into());
    }
    require_bytes(
        &output.join(RECEIPT),
        &evidence::encode(&make_receipt(&intent_path, &raw_intent, &intent))?,
    )
}

#[expect(clippy::too_many_arguments, reason = "锁内使用同一组已登记和解输入")]
fn reconcile_locked(
    plan: &Value,
    root: &Path,
    output: &Path,
    expected_before_sha256: &str,
    slot: &str,
    user_id: &str,
    authority: &Value,
    guard: &mut dyn FnMut() -> Result<()>,
) -> Result<Value> {
    let ledger_path = root.join("ledger.json");
    let intent_path = root.join(INTENT);
    let archive_path = output.join(ARCHIVE);
    let receipt_path = output.join(RECEIPT);
    let current = read_required(&ledger_path)?;
    let is_before = evidence::hash(&current) == expected_before_sha256;
    let archived;
    let before = if is_before {
        current.as_slice()
    } else {
        archived = read_required(&archive_path)?;
        archived.as_slice()
    };
    let original = evidence::parse(before)?;
    if evidence::hash(before) != expected_before_sha256 || evidence::encode(&original)? != before {
        return Err("首个用户和解的原账本字节与登记前像不同".into());
    }
    let after = project_first_create(plan, &original, slot, user_id)?;
    let after_bytes = evidence::encode(&after)?;
    let intent = make_intent(plan, root, authority, (slot, user_id), before, &after_bytes);
    let intent_bytes = evidence::encode(&intent)?;
    if !is_before && (current != after_bytes || read_required(&intent_path)? != intent_bytes) {
        return Err("首个用户和解仅接受同一意图的原账本或精确后像".into());
    }
    let receipt = make_receipt(&intent_path, &intent_bytes, &intent);
    guard()?;
    evidence::immutable(&intent_path, &intent)?;
    evidence::immutable(&archive_path, &original)?;
    guard()?;
    require_bytes(&intent_path, &intent_bytes)?;
    require_bytes(&archive_path, before)?;
    if is_before {
        if evidence::regular(&receipt_path)?.is_some()
            || evidence::regular(&receipt_path.with_extension("json.pending"))?.is_some()
        {
            return Err("首个用户和解仍在前像却已出现结果，保留证据".into());
        }
        evidence::replace(&ledger_path, &after, before)?;
    }
    guard()?;
    require_bytes(&ledger_path, &after_bytes)?;
    require_bytes(&intent_path, &intent_bytes)?;
    require_bytes(&archive_path, before)?;
    evidence::immutable(&receipt_path, &receipt)?;
    guard()?;
    require_bytes(&ledger_path, &after_bytes)?;
    require_bytes(&intent_path, &intent_bytes)?;
    require_bytes(&archive_path, before)?;
    require_bytes(&receipt_path, &evidence::encode(&receipt)?)?;
    Ok(receipt)
}

fn make_intent(
    plan: &Value,
    root: &Path,
    authority: &Value,
    (slot, user_id): (&str, &str),
    before: &[u8],
    after: &[u8],
) -> Value {
    let output = root.join(DIRECTORY);
    json!({
        "format_version": 1,
        "kind": "devex-identity-first-create-reconciliation",
        "plan_sha256": plan["plan_sha256"],
        "identity_state": root,
        "output": output,
        "operation": OPERATION,
        "slot": slot,
        "user_id": user_id,
        "authority": authority,
        "before": byte_binding(&output.join(ARCHIVE), before),
        "after": byte_binding(&root.join("ledger.json"), after),
    })
}

fn make_receipt(intent_path: &Path, intent_bytes: &[u8], intent: &Value) -> Value {
    json!({
        "format_version": 1,
        "kind": "devex-identity-first-create-reconciliation-result",
        "status": "confirmed",
        "intent": byte_binding(intent_path, intent_bytes),
        "before": intent["before"],
        "after": intent["after"],
        "remote_writes": 0,
    })
}

fn project_first_create(
    plan: &Value,
    original: &Value,
    slot: &str,
    user_id: &str,
) -> Result<Value> {
    state::id(&json!(user_id))?;
    let operations = state::expected(plan)?;
    if original["status"] != "needs-reconciliation"
        || !operations
            .get(2)
            .is_some_and(|(operation, planned_slot)| operation == OPERATION && planned_slot == slot)
    {
        return Err("只接受计划中首个待和解的用户创建".into());
    }
    let started = json!({"operation": OPERATION, "slot": slot, "phase": "started"});
    let mut after = original.clone();
    let entries = after["entries"]
        .as_array_mut()
        .ok_or("身份账本缺少操作序列")?;
    if entries.len() != 3 || entries.last() != Some(&started) {
        return Err("首个用户和解要求两条已确认角色操作和一条待确认的创建".into());
    }
    entries.pop();
    after["status"] = json!("applying");
    state::validate(plan, &after, false)?;
    let confirmed = json!({
        "operation": OPERATION, "slot": slot, "phase": "confirmed", "id": user_id
    });
    after["entries"]
        .as_array_mut()
        .ok_or("身份账本缺少操作序列")?
        .push(confirmed);
    after["users"][slot] = json!(user_id);
    state::validate(plan, &after, false)?;
    Ok(after)
}

fn read_required(path: &Path) -> Result<Vec<u8>> {
    evidence::regular(path)?.ok_or_else(|| "首个用户和解缺少已登记的原件".into())
}

fn require_bytes(path: &Path, expected: &[u8]) -> Result<()> {
    if read_required(path)? != expected {
        return Err("首个用户和解原件或结果变化，保留现场".into());
    }
    Ok(())
}

fn byte_binding(path: &Path, bytes: &[u8]) -> Value {
    json!({"path": path, "bytes": bytes.len(), "sha256": evidence::hash(bytes)})
}
