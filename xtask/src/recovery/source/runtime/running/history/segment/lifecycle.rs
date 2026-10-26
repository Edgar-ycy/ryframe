//! 资源恢复后的单次生成与导出序列；失败授权和清理不改变原有写入资格。
use super::super::{lineage, prelaunch, stops};
use crate::{Result, recovery::clone::native::json as document};
use serde_json::{Value, json};

pub(super) fn read(
    context: &super::Context<'_>,
    tail: &[Value],
    suffix: &[Value],
    cache: &Value,
    storage: &Value,
    resources: &Value,
) -> Result<Value> {
    if !tail.is_empty()
        && let Some(reboot) = super::super::reboot::tail(
            context.backend,
            context.run,
            context.attempts,
            tail,
            context.archive,
            context.descriptor,
        )?
    {
        return Ok(
            json!({"phase":"ready","records":reboot["records"],"storage":reboot["storage"],
            "cache":reboot["cache"],"replay":null,"reboot":reboot}),
        );
    }
    let cleanup_at = tail
        .iter()
        .position(|row| {
            matches!(
                row["stage"].as_str(),
                Some("cache-target" | "storage-target")
            )
        })
        .unwrap_or(tail.len());
    let (lifecycle, cleanup) = tail.split_at(cleanup_at);
    for row in lifecycle {
        super::product(&context.archive["recovery"]["sources"], row)?;
    }
    let count = resources["count"].as_u64().ok_or("分段资源数量无效")? as usize;
    let previous = super::number(if count > 0 {
        &suffix[2 + count]
    } else {
        &suffix[2]
    })?;
    let mut generation = Generation {
        rows: lifecycle,
        previous,
        records: Vec::new(),
        replay: Value::Null,
        pending: false,
    };
    reregistration(context, &mut generation)?;
    replay(context, &mut generation)?;
    let exported = if generation.pending {
        None
    } else {
        generation_tail(generation.rows, generation.previous)?
    };
    if generation.pending && !cleanup.is_empty() {
        return Err("C69 重试授权完成前不能清理资源".into());
    }
    let phase = if cleanup.is_empty() {
        resources["phase"].clone()
    } else {
        json!(cleanup_phase(
            context,
            cleanup,
            exported.ok_or("成功导出后才允许资源停机")?,
            cache,
            storage
        )?)
    };
    generation.records.extend_from_slice(cleanup);
    Ok(
        json!({"phase":phase,"records":generation.records,"storage":resources["storage"],
        "cache":resources["cache"],"replay":generation.replay,"reboot":null}),
    )
}

struct Generation<'a> {
    rows: &'a [Value],
    previous: u64,
    records: Vec<Value>,
    replay: Value,
    pending: bool,
}

fn reregistration(context: &super::Context<'_>, generation: &mut Generation<'_>) -> Result<()> {
    let Some(first) = generation.rows.first().filter(|row| {
        super::operation(row) == ("seed-runtime", "source-generation-start")
            && row["status"] == "failed"
    }) else {
        return Ok(());
    };
    if context
        .run
        .join(format!("g{:04}", super::number(first)?))
        .try_exists()?
    {
        return Ok(());
    }
    let prior = context
        .attempts
        .iter()
        .position(|row| row == first)
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| context.attempts.get(index))
        .ok_or("失败生成重登记缺少前序阶段")?;
    prelaunch::reregistration(context.backend, context.run, first, prior)?;
    generation.records.push(first.clone());
    generation.previous = super::number(first)?;
    generation.rows = &generation.rows[1..];
    Ok(())
}

fn replay(context: &super::Context<'_>, generation: &mut Generation<'_>) -> Result<()> {
    let rows = generation.rows;
    let Some(first) = rows.first() else {
        return Ok(());
    };
    let recognized = lineage::model(
        "pending",
        json!({"attempts":[first],"previous":generation.previous}),
    )?;
    if !document::boolean(&recognized["pending"], "C68 待恢复失败")? {
        return Ok(());
    }
    if rows.len() == 1 {
        generation.pending = true;
        return Ok(());
    }
    if rows.len() == 2
        && super::number(first)?.checked_add(1) == Some(super::number(&rows[1])?)
        && super::operation(&rows[1]) == ("seed-runtime", "source-generation-recover")
        && rows[1]["status"] == "running"
        && rows[1]["result"].is_null()
        && rows[1]["error_type"].is_null()
    {
        super::owner(context.run, context.attempts, &rows[1], context.current)?;
        generation.pending = true;
        return Ok(());
    }
    generation.replay = lineage::authority(context.backend, context.run, context.attempts)?;
    if generation.replay.is_null() || generation.replay["records"] != json!(&rows[..2]) {
        return Err("C68 失败后缺少唯一相邻且完整的 C69 重试授权".into());
    }
    generation.records.extend(
        document::array(&generation.replay["records"], "C69 原失败和授权历史")?
            .iter()
            .cloned(),
    );
    generation.previous = super::number(&generation.replay["record"])?;
    generation.rows = &rows[2..];
    Ok(())
}

fn generation_tail(tail: &[Value], previous: u64) -> Result<Option<&Value>> {
    if tail.is_empty() {
        return Ok(None);
    }
    for (offset, row) in tail.iter().enumerate() {
        if previous.checked_add(u64::try_from(offset + 1)?) != Some(super::number(row)?) {
            return Err("分段资源续作后的生成阶段不相邻".into());
        }
    }
    let start = ("seed-runtime", "source-generation-start");
    let stop = ("seed-runtime", "source-generation-stop");
    let recover = ("seed-runtime", "source-generation-recover");
    if super::operation(&tail[0]) != start
        || tail
            .iter()
            .filter(|row| super::operation(row) == start)
            .count()
            != 1
    {
        return Err("分段资源恢复后只允许唯一实际 START".into());
    }
    if tail.len() == 1 {
        return Ok(None);
    }
    if super::operation(&tail[1]) == recover {
        if tail[0]["status"] == "passed" || tail.len() != 2 {
            return Err("成功 START 不能恢复，恢复后也不能重放".into());
        }
        return Ok(None);
    }
    if super::operation(&tail[1]) != stop || tail[0]["status"] != "passed" {
        return Err("只有成功 START 才能进入唯一 STOP".into());
    }
    if tail.len() == 2 {
        return Ok(None);
    }
    if super::operation(&tail[2]) == recover {
        if tail[1]["status"] == "passed" || tail.len() != 3 {
            return Err("成功 STOP 不能恢复，恢复后也不能重放".into());
        }
        return Ok(None);
    }
    if super::operation(&tail[2]) != ("seed-runtime", "source-export")
        || tail[1]["status"] != "passed"
    {
        return Err("只有成功 STOP 才能进入唯一 source-export".into());
    }
    if tail.len() == 3 {
        return Ok((tail[2]["status"] == "passed").then_some(&tail[2]));
    }
    let following = &tail[3..];
    if tail[2]["status"] != "failed"
        || following
            .iter()
            .any(|row| super::operation(row) != ("seed-runtime", "source-export-reconcile"))
    {
        return Err("成功或未收尾导出后不能重复导出".into());
    }
    let terminal = following
        .iter()
        .filter(|row| row["status"] != "failed")
        .collect::<Vec<_>>();
    if terminal.len() > 1
        || terminal
            .first()
            .is_some_and(|row| Some(*row) != following.last())
    {
        return Err("失败导出只允许一个最终 reconcile".into());
    }
    Ok(terminal
        .first()
        .copied()
        .filter(|row| row["status"] == "passed"))
}

fn cleanup_phase(
    context: &super::Context<'_>,
    cleanup: &[Value],
    exported: &Value,
    cache: &Value,
    storage: &Value,
) -> Result<&'static str> {
    let super::Context {
        backend,
        run,
        attempts,
        archive,
        descriptor,
        current,
    } = *context;
    if !(1..=2).contains(&cleanup.len())
        || super::number(exported)?.checked_add(1) != Some(super::number(&cleanup[0])?)
        || super::operation(&cleanup[0]) != ("cache-target", "stop")
    {
        return Err("成功导出后必须先执行唯一缓存停机".into());
    }
    for (offset, row) in cleanup.iter().enumerate() {
        if super::number(&cleanup[0])?.checked_add(u64::try_from(offset)?)
            != Some(super::number(row)?)
            || row["sources"] != exported["sources"]
        {
            return Err("导出后的资源停机必须相邻且使用同一完整来源".into());
        }
        super::product(&archive["recovery"]["sources"], row)?;
    }
    if cleanup[0]["status"] == "running" {
        super::owner(run, attempts, &cleanup[0], current)?;
        if cleanup.len() != 1 {
            return Err("缓存停机未完成前不能继续存储停机".into());
        }
        return Ok("cache-stopping");
    }
    if cleanup[0]["status"] != "passed" || !cleanup[0]["error_type"].is_null() {
        return Err("导出后的缓存停机失败或未收尾".into());
    }
    if stops::cache(
        backend,
        run,
        &cleanup[0],
        descriptor,
        &archive["receipt"]["review_successor"],
    )?["request"]
        != cache["request"]
    {
        return Err("导出后缓存停机没有消费相同请求".into());
    }
    if cleanup.len() == 1 {
        return Ok("cache-stopped");
    }
    if super::operation(&cleanup[1]) != ("storage-target", "stop") {
        return Err("缓存停机后必须立即执行唯一存储停机".into());
    }
    if cleanup[1]["status"] == "running" {
        super::owner(run, attempts, &cleanup[1], current)?;
        return Ok("storage-stopping");
    }
    if cleanup[1]["status"] != "passed" || !cleanup[1]["error_type"].is_null() {
        return Err("导出后的存储停机失败或未收尾".into());
    }
    if stops::storage(backend, run, &cleanup[1])?["request"] != storage["request"] {
        return Err("导出后存储停机没有消费相同请求".into());
    }
    Ok("closed")
}
