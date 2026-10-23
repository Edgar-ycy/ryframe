use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::super::{exact_object, text_field, valid_sha256};
use crate::Result;

pub(super) const SHIFT_HOURS: i64 = 169;
pub(super) const MIGRATION_TIMES: &[&str] = &[
    "prechecked_at",
    "queued_at",
    "quiesced_at",
    "frozen_at",
    "copy_started_at",
    "copy_completed_at",
    "verified_at",
    "cut_over_at",
    "activated_at",
    "succeeded_at",
    "retention_until",
    "created_at",
    "updated_at",
];
pub(super) const ITEM_TIMES: &[&str] = &[
    "copy_started_at",
    "copied_at",
    "verified_at",
    "created_at",
    "updated_at",
];
pub(super) const JOB_TIMES: &[&str] = &["available_at", "created_at", "updated_at", "completed_at"];
pub(super) const ATTEMPT_TIMES: &[&str] =
    &["available_at", "started_at", "finished_at", "closed_at"];

pub(super) fn id(value: &Value, field: &str) -> Result<i64> {
    let text = text_field(value, field, "迁移记录")?;
    let number = text.parse::<i64>().ok().filter(|v| *v > 0);
    if number.is_none() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("迁移记录 {field} 必须是正 i64").into());
    }
    Ok(number.expect("已验证正 i64"))
}

pub(super) fn attempts(row: &Value) -> Result<Vec<Value>> {
    let sequence = id(row, "job_claim_sequence")?;
    let mut values = row["attempt_records"]
        .as_array()
        .ok_or("任务尝试 ownership 缺失")?
        .clone();
    let mut ordered = Vec::new();
    for value in values.drain(..) {
        exact_object(
            &value,
            &["job_id", "sequence", "outcome", "times"],
            "任务尝试 ownership",
        )?;
        let index = id(&value, "sequence")?;
        if value["job_id"] != row["job_id"] {
            return Err("任务尝试 ownership 不匹配".into());
        }
        exact_object(&value["times"], ATTEMPT_TIMES, "任务尝试时间")?;
        let times = ATTEMPT_TIMES
            .iter()
            .map(|field| timestamp(&value["times"][*field]))
            .collect::<Result<Vec<_>>>()?;
        let [Some(available), Some(started), finished, Some(closed)] = times.as_slice() else {
            return Err("任务尝试未闭合".into());
        };
        if started < available || closed < started {
            return Err("任务尝试时间逆序".into());
        }
        match value["outcome"].as_str() {
            Some("lease_expired") if finished.is_none() => {}
            Some("succeeded" | "failed" | "dead" | "deferred")
                if finished.as_ref() == Some(closed) => {}
            _ => return Err("任务尝试结果或完成时间不匹配".into()),
        }
        ordered.push((index, value));
    }
    ordered.sort_by_key(|(index, _)| *index);
    if i64::try_from(ordered.len())? != sequence
        || ordered
            .iter()
            .enumerate()
            .any(|(i, (number, _))| *number != i as i64 + 1)
        || ordered
            .last()
            .is_none_or(|(_, value)| value["outcome"] != "succeeded")
    {
        return Err("任务尝试 ownership、领取序号或最终状态不匹配".into());
    }
    Ok(ordered.into_iter().map(|(_, value)| value).collect())
}

pub(super) fn validate(
    rows: Vec<Value>,
    tenant: &str,
    migration: i64,
    finalized: bool,
) -> Result<Value> {
    let [mut row]: [Value; 1] = rows
        .try_into()
        .map_err(|_| "历史 fixture 必须只有一条迁移")?;
    let expected = json!({
        "migration_id": migration.to_string(), "tenant_id":tenant,
        "source":"dedicated-a", "target":"dedicated-b",
        "state": if finalized { "finalized" } else { "retention_pending" },
        "retention_hours":168, "source_mode":"dedicated", "target_mode":"dedicated",
        "job_tenant_id":tenant, "retention_seconds":168*3600,
        "job_status":"succeeded", "job_migration_id":migration.to_string(), "job_lease_clear":1,
        "item_state":"verified", "item_source_count":3, "item_target_count":3,
        "cleanup_state":if finalized { "cleaned" } else { "pending" },
        "cleanup_count":if finalized { 3 } else { 0 },
        "placement_matches":1, "target_fence_matches":1, "target_slot_matches":1,
        "source_fence_matches":if finalized {0} else {1},
        "source_fences":if finalized {0} else {1}, "source_slots":if finalized {0} else {1},
        "source_slot_matches":if finalized {0} else {1},
        "source_count":if finalized {0} else {3}, "source_total":if finalized {0} else {3},
        "target_count":3,"target_total":3,"other_migrations":0,"operation_leases":0
    });
    if expected
        .as_object()
        .expect("常量对象")
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
        || (!finalized && row["intent_clear"] != 1)
    {
        return Err("历史 fixture 的迁移、任务、检查点、放置或独占目标 ownership 不匹配".into());
    }
    for field in ["source_digest", "target_digest", "fingerprint"] {
        valid_sha256(text_field(&row, field, "历史 fixture")?, field)?;
    }
    if row["source_digest"] != row["target_digest"] {
        return Err("迁移数据摘要不匹配".into());
    }
    for field in [
        "job_id",
        "item_id",
        "source_generation",
        "target_generation",
    ] {
        id(&row, field)?;
    }
    for (field, columns) in [
        ("migration_times", MIGRATION_TIMES),
        ("item_times", ITEM_TIMES),
        ("job_times", JOB_TIMES),
    ] {
        exact_object(&row[field], columns, "迁移时间字段")?;
        for column in columns {
            timestamp(&row[field][*column])?;
        }
    }
    row["remaining_seconds"]
        .as_i64()
        .ok_or("保留期剩余秒数无效")?;
    row["attempt_records"] = json!(attempts(&row)?);
    Ok(row)
}

pub(super) fn plan(contract: &Value, row: Value) -> Result<Value> {
    let tenant = text_field(contract, "tenant_id", "历史计划")?;
    let row = validate(vec![row], tenant, id(contract, "migration_id")?, false)?;
    if !(167 * 3600..=168 * 3600).contains(&row["remaining_seconds"].as_i64().ok_or("保留期无效")?)
    {
        return Err("仅允许本次刚完成、保留期尚未结束的迁移构造历史 fixture".into());
    }
    let mut payload = contract.clone();
    let fields = payload.as_object_mut().ok_or("历史计划必须为对象")?;
    fields.extend(json!({"artificial_history":true,"shift_hours":SHIFT_HOURS,"retention_hours":168,"natural_elapsed_7_days":false,"before":row}).as_object().expect("常量对象").clone());
    payload["before"]
        .as_object_mut()
        .ok_or("缺少迁移快照")?
        .remove("remaining_seconds");
    let hash = crate::devex::runtime_native::artifacts::sha(&serde_json::to_vec(&payload)?);
    payload["plan_sha256"] = json!(hash);
    payload["state"] = json!("history-planned");
    payload["before"] = row;
    Ok(payload)
}

pub(super) fn verify_shift(before: &Value, after: &Value) -> Result<()> {
    let retained = |value: &Value| -> Result<Value> {
        let mut value = value.as_object().ok_or("历史快照不是对象")?.clone();
        for key in [
            "remaining_seconds",
            "migration_times",
            "item_times",
            "job_times",
            "attempt_records",
        ] {
            value.remove(key);
        }
        Ok(Value::Object(value))
    };
    if retained(before)? != retained(after)? {
        return Err("历史调整改变了时间以外的迁移内容".into());
    }
    for field in ["migration_times", "item_times", "job_times"] {
        verify_timeline(&before[field], &after[field])?;
    }
    let old = attempts(before)?;
    let new = attempts(after)?;
    if old.len() != new.len() {
        return Err("任务尝试数量发生变化".into());
    }
    for (mut old, mut new) in old.into_iter().zip(new) {
        verify_timeline(&old["times"], &new["times"])?;
        old.as_object_mut().ok_or("尝试不是对象")?.remove("times");
        new.as_object_mut().ok_or("尝试不是对象")?.remove("times");
        if old != new {
            return Err("任务尝试时间以外的内容发生变化".into());
        }
    }
    if after["remaining_seconds"].as_i64().is_none_or(|v| v >= 0) {
        return Err("历史 fixture 未形成真实数据库时钟下的到期状态".into());
    }
    Ok(())
}

fn verify_timeline(before: &Value, after: &Value) -> Result<()> {
    let old = before.as_object().ok_or("原时间线无效")?;
    let new = after.as_object().ok_or("新时间线无效")?;
    if old.keys().ne(new.keys()) {
        return Err("时间字段没有完整平移".into());
    }
    for (field, value) in old {
        let expected = timestamp(value)?
            .map(|v| {
                v.checked_sub_signed(Duration::hours(SHIFT_HOURS))
                    .ok_or("历史时间溢出")
            })
            .transpose()?;
        if timestamp(&new[field])? != expected {
            return Err("迁移、检查点或任务尝试时间没有完整平移".into());
        }
    }
    Ok(())
}

fn timestamp(value: &Value) -> Result<Option<DateTime<Utc>>> {
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or("时间字段无效")?;
    Ok(Some(
        DateTime::parse_from_rfc3339(text)
            .map_err(|_| "时间必须是 RFC3339")?
            .with_timezone(&Utc),
    ))
}
