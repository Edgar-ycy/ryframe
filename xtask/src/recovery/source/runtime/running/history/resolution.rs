//! 发布后历史的证明任务逐项消费；纯模型选择不能代替文件、缓存和控制锁核验。
use super::{model, prelaunch};
use crate::{
    Result,
    recovery::clone::native::{
        cache, json as document,
        lifecycle::{identity, lock, state},
        resources::common,
        seed::{
            arm_input::history as arm_history,
            rebind::{Controller, Phase, classify},
        },
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn inspect(
    backend: &Path,
    run: &Path,
    saved: &Value,
    descriptor: &Value,
    arm: Option<&arm_history::Active<'_>>,
) -> Result<Value> {
    let attempts = state::attempts(saved)?;
    let current = model("active-read", json!({"attempts":attempts}))?["current"].clone();
    inspect_current(backend, run, saved, descriptor, &current, false, arm)
}

// 明确 attempt 只能由已核对原 RunLock 与当前账本的正式控制器提供。
pub(in crate::recovery::source::evidence) fn source_rebind(
    controller: &Controller<'_>,
    descriptor: &Value,
) -> Result<Value> {
    controller.checked(|| {
        classify(
            Phase::History,
            inspect_current(
                controller.backend(),
                controller.run(),
                controller.saved(),
                descriptor,
                &json!(controller.number()),
                false,
                None,
            ),
        )
    })
}

pub(super) fn inspect_current(
    backend: &Path,
    run: &Path,
    saved: &Value,
    descriptor: &Value,
    current: &Value,
    cache_recovery: bool,
    arm: Option<&arm_history::Active<'_>>,
) -> Result<Value> {
    let attempts = state::attempts(saved)?;
    let archive = prelaunch::closed(backend, run, attempts)?;
    let segment = if archive.is_null() {
        Value::Null
    } else {
        super::segment::read(backend, run, attempts, &archive, descriptor, current)?
    };
    let prefix = if attempts.last().is_some_and(|row| row["number"] == *current) {
        &attempts[..attempts.len() - 1]
    } else {
        attempts
    };
    let pending = if archive.is_null()
        && prefix.last().is_some_and(|row| {
            row["stage"] == "seed-runtime"
                && row["mode"] == "source-generation-recover"
                && row["status"] == "failed"
        }) {
        if prefix
            .last()
            .is_some_and(prelaunch::partial::applies_recovery_failure)
        {
            partial_pending(backend, run, attempts, current)?
        } else {
            let (start, _) = prelaunch::origin(backend, run, prefix)?;
            let index = prefix
                .iter()
                .position(|row| *row == start)
                .ok_or("未启动恢复缺少原 START")?;
            json!(&prefix[index..])
        }
    } else {
        json!([])
    };
    let selected_archive = if archive.is_null() {
        Value::Null
    } else {
        json!({"receipt":archive["receipt"],"records":archive["records"]})
    };
    let selected_segment = if segment.is_null() {
        Value::Null
    } else {
        json!({"records":segment["records"]})
    };
    let exists = if let Some(row) = attempts.last() {
        let number = row["number"].as_u64().ok_or("后续历史序号无效")?;
        run.join(format!("g{number:04}")).try_exists()?
    } else {
        false
    };
    let planned = model(
        "history",
        json!({"attempts":attempts,"descriptor":descriptor,"current":current,
        "archive":selected_archive,"segment":selected_segment,"pending":pending,
        "backend_available":true,"cache_recovery":cache_recovery,"generation_output_exists":exists}),
    )?;
    let mut proofs = Vec::new();
    let context = History {
        backend,
        run,
        saved,
        descriptor,
        current,
        arm,
    };
    for action in document::array(&planned["actions"], "完整历史证明任务")? {
        consume(&context, &archive, action, &mut proofs)?;
    }
    Ok(json!({"later":planned["later"],"archive":archive,"segment":segment}))
}

fn partial_pending(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    current: &Value,
) -> Result<Value> {
    let [.., start, failed, active] = attempts else {
        return Err("部分未启动接续缺少连续历史".into());
    };
    if active["number"] != *current
        || active["stage"] != "seed-runtime"
        || active["mode"] != "source-generation-recover"
        || active["status"] != "running"
        || failed["number"].as_u64().and_then(|n| n.checked_add(1)) != active["number"].as_u64()
    {
        return Err("原部分采集失败仅由紧邻的正式持锁恢复接续".into());
    }
    let request = common::binding(&prelaunch::partial::expected_request_path(run)?)?;
    prelaunch::partial::recovery_failure(backend, run, failed, start, &request)?;
    Ok(json!([start, failed]))
}

struct History<'a> {
    backend: &'a Path,
    run: &'a Path,
    saved: &'a Value,
    descriptor: &'a Value,
    current: &'a Value,
    arm: Option<&'a arm_history::Active<'a>>,
}

fn consume(
    context: &History<'_>,
    archive: &Value,
    action: &Value,
    proofs: &mut Vec<Value>,
) -> Result<()> {
    let History {
        backend,
        run,
        saved,
        descriptor,
        current,
        arm,
    } = *context;
    let frozen = &archive["receipt"]["review_successor"];
    match document::text(&action["kind"], "历史证明类型")? {
        "cache" => {
            let successor = cache_call(
                backend,
                run,
                "history-proof",
                json!({"attempt":action["record"],
                "source_result":descriptor,"frozen_successor":frozen}),
            )?;
            proofs.push(json!({"kind":"cache","successor":successor}));
            model("history-proofs", json!({"proofs":proofs}))?;
            if document::boolean(&action["owner"], "当前缓存控制器")? {
                owner(run)?;
            }
        }
        "registered-cache" => {
            let frozen = if frozen.is_null() {
                Value::Null
            } else {
                json!([descriptor, frozen])
            };
            cache::binding::query(backend, run, None, &archive["recovery"]["number"], &frozen)?;
        }
        "rebind-cache" => {
            let value = common::read_bound(backend, &action["record"]["result"], "缓存重绑定结果")?;
            proofs.push(json!({"kind":"rebind-cache","successor":value["review_successor"]}));
            model("history-proofs", json!({"proofs":proofs}))?;
        }
        "owner" => owner(run)?,
        "zero-output" => {
            prelaunch::proof(backend, run, &action["record"])?;
        }
        "export-reconcile" => {
            if !super::export::reconciles(
                backend,
                run,
                saved,
                &action["origin"],
                current,
                descriptor,
            )? {
                return Err("seed 发布后存在失败或未收尾阶段".into());
            }
        }
        "arm-prefix" => {
            arm_history::prove(backend, run, saved, &action["record"], descriptor, arm)?
        }
        _ => return Err("重绑定模型返回未知证明任务".into()),
    }
    Ok(())
}

pub(super) fn cache_call(
    backend: &Path,
    run: &Path,
    action: &str,
    parameters: Value,
) -> Result<Value> {
    cache::binding::execute_stage(
        backend,
        &json!({"action":action,"directory":run,
        "caller":std::process::id(),"parameters":parameters}),
        None,
    )
}

pub(super) fn owner(run: &Path) -> Result<()> {
    let before = lock::owner_observation(run)?.ok_or("当前历史控制阶段缺少运行锁")?;
    if before.1["identity"] != identity::current()? {
        return Err("当前历史控制阶段必须由实际进程持有运行锁，不能委托或提升资格".into());
    }
    if lock::owner_observation(run)? != Some(before) {
        return Err("历史证明期间控制锁身份变化；保留现场，禁止重放".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_history_still_rejects_running_rebind_before_observing_files() {
        let saved = json!({"attempts":[{"number":1,"stage":"seed-runtime",
            "mode":"source-rebind","status":"running"}]});
        let error = inspect(
            Path::new("absent"),
            Path::new("absent"),
            &saved,
            &Value::Null,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("未收尾写入或控制动作"));
    }
}
