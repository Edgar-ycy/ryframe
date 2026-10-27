//! 原生生命周期账本写入：保留 Python 前缀键序，仍使用相同阶段和结果校验。
use super::*;
use crate::recovery::clone::native::digest;
use std::io::{Read, Write};

pub(super) fn begin(run: &Path, operation: &str, sources: &Value) -> Result<(u64, Value)> {
    let mut value = state::load(run, true)?;
    check(
        state::running_attempt(&value)?.is_none(),
        "存在未收尾阶段，必须先显式恢复原控制器",
    )?;
    let number = u64::try_from(state::attempts(&value)?.len())?
        .checked_add(1)
        .ok_or("生命周期编号溢出")?;
    let attempt = json!({"number":number,"stage":"fixture-services","mode":operation,
        "started_at":now(),"finished_at":null,"status":"running","sources":sources,"result":null,"error_type":null});
    let previous = value.clone();
    value["attempts"]
        .as_array_mut()
        .ok_or("生命周期账本格式无效")?
        .push(attempt.clone());
    write(run, &previous, &value)?;
    Ok((number, attempt))
}

pub(super) fn finish(
    run: &Path,
    number: u64,
    result: Option<&Value>,
    error: Option<&str>,
) -> Result<()> {
    let mut value = state::load(run, true)?;
    let previous = value.clone();
    let attempt = value["attempts"]
        .as_array_mut()
        .and_then(|items| items.last_mut())
        .ok_or("生命周期账本为空")?;
    check(
        attempt["number"] == number && attempt["status"] == "running",
        "不能收尾其他控制器或已结束阶段",
    )?;
    check(
        error.is_some() || result.is_some(),
        "成功阶段必须发布实际结果",
    )?;
    if let Some(result) = result {
        let path = state::result_path(run, number);
        if path.exists() {
            check(read(&path)? == *result, "已保存结果不同，拒绝覆盖未知写入")?;
        } else {
            json_io::write_new(&path, result)?;
        }
        attempt["result"] = binding(&path)?;
    }
    attempt["status"] = json!(if error.is_some() { "failed" } else { "passed" });
    attempt["finished_at"] = json!(now());
    attempt["error_type"] = error.map_or(Value::Null, |error| json!(error));
    write(run, &previous, &value)
}

pub(super) fn write(run: &Path, previous: &Value, next: &Value) -> Result<()> {
    let path = run.join("state.json");
    let before = binding(&path)?;
    let mut raw = Vec::new();
    std::fs::File::open(&path)?
        .take(json_io::MAX_JSON_BYTES + 1)
        .read_to_end(&mut raw)?;
    check(
        raw.len() as u64 <= json_io::MAX_JSON_BYTES,
        "生命周期账本超过读取上限",
    )?;
    check(
        serde_json::from_slice::<Value>(&raw)? == *previous && binding(&path)? == before,
        "生命周期账本写入前像变化，保留现场",
    )?;
    let bytes = history::prefix::updated_serialization(&raw, next)?;
    let mut temporary = tempfile::NamedTempFile::new_in(run)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    check(
        binding(&path)? == before && !digest::linked_path(&path)?.contains(&true),
        "发布生命周期账本前来源变化",
    )?;
    temporary.persist(&path)?;
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}
