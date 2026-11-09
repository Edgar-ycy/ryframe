//! 原代次回收不启动第二个产品或验收生产者；基线失败仍精确收树，但绝不发布零漂移证明。
use super::{existing, frame::Frame, lineage, prelaunch, trees, window::Window};
use crate::{
    Result,
    recovery::{
        clone::native::{
            export::transport::Transport, json as document, lifecycle::state, resources::common,
            seed::generation::coordinator,
        },
        source::evidence::{
            execution::saved,
            images,
            running::{self, history::lineage as lineage_model},
        },
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn preflight(backend: &Path, run: &Path, saved: &Value, request: &Path) -> Result<()> {
    let prefix = state::attempts(saved)?;
    let number = prefix
        .last()
        .and_then(|row| row["number"].as_u64())
        .and_then(|n| n.checked_add(1))
        .ok_or("来源恢复缺少原阶段或下一序号溢出")?;
    if pending(prefix)? {
        lineage::prepare(backend, run, prefix, number, request)?;
        return Ok(());
    }
    reject_retry(prefix)?;
    let starts = running::history::starts(backend, run, prefix)?;
    let [start] = starts.as_slice() else {
        return Err("源代次回收必须继承唯一已登记 start".into());
    };
    let started = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("原 START 序号无效")?;
    if !run.join(format!("g{started:04}")).try_exists()? {
        prelaunch::prepare(backend, run, saved, number, request)?;
    } else if prefix.iter().any(|row| {
        row["number"].as_u64().is_some_and(|n| n > started)
            && row["stage"] == "seed-runtime"
            && matches!(
                row["mode"].as_str(),
                Some("source-generation-stop" | "source-generation-recover")
            )
            && (row["mode"] == "source-generation-recover" || row["status"] == "passed")
    }) {
        return Err("源代次已停止或执行回收，不能重放".into());
    }
    Ok(())
}

pub(super) fn run<T: Transport + Sync>(
    frame: &Frame<'_>,
    request: &Path,
    transport: &T,
) -> Result<Value> {
    frame.checkpoint()?;
    if pending(&frame.prefix)? {
        return lineage::run(frame, request, transport);
    }
    reject_retry(&frame.prefix)?;
    let starts = running::history::starts(frame.backend, frame.run, &frame.prefix)?;
    let [start] = starts.as_slice() else {
        return Err("回收必须继承唯一实际来源启动".into());
    };
    let number = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("原 START 序号无效")?;
    if !frame.run.join(format!("g{number:04}")).try_exists()? {
        prelaunch::run(frame, request, transport)
    } else {
        existing(frame, request, transport)
    }
}

fn pending(prefix: &[Value]) -> Result<bool> {
    document::boolean(
        &lineage_model::model("pending", json!({"attempts":prefix,"previous":null}))?["pending"],
        "谱系恢复选择",
    )
}

fn reject_retry(prefix: &[Value]) -> Result<()> {
    let plan = lineage_model::model("replay-plan", json!({"attempts":prefix}))?;
    if prefix.last().is_some_and(|last| {
        !plan["record"].is_null()
            && *last == plan["record"]
            && last["status"] == "failed"
            && last["result"].is_null()
            && last["error_type"] == "ValueError"
    }) {
        return Err("C69 已形成固定只读授权证据；禁止重复采集，下一阶段只能执行 C70 START".into());
    }
    Ok(())
}

pub(super) fn existing<T: Transport + Sync>(
    frame: &Frame<'_>,
    request_path: &Path,
    transport: &T,
) -> Result<Value> {
    let starts = running::history::starts(frame.backend, frame.run, &frame.prefix)?;
    let [start] = starts.as_slice() else {
        return Err("回收必须绑定唯一实际来源启动".into());
    };
    let observed = existing::status(frame, start)?;
    if document::object(&observed["roles"], "原源角色")?
        .values()
        .any(|row| row["state"] == "unknown")
    {
        return Err(
            "启动在完整树发布前中断，缺少完整归属，不能推断停止或自动回收；禁止重放".into(),
        );
    }
    let verifier = frame.verifier_stopped()?;
    let descriptor = common::binding(request_path)?;
    let request = common::read_bound(frame.backend, &descriptor, "源回收请求")?;
    coordinator::request(&request)?;
    let window = if start["status"] == "passed" {
        Window::new(
            frame,
            running::load(frame.backend, &start["result"], false, true)?,
            transport,
        )?
    } else {
        Window::unpublished(frame, start, &descriptor, transport)?
    };
    if window.facts["request"] != request {
        return Err("回收必须显式绑定原 source-generation 请求".into());
    }
    existing::matches_source(&request, &window.facts["source"])?;
    let mut before_image = Value::Null;
    let before = before(&window, start, &verifier, &mut before_image);
    frame.checkpoint()?;
    common::bound_file(frame.backend, &descriptor, "源回收请求")?;
    if frame.verifier_stopped()? != verifier {
        return Err("回收前来源验收生产者退出事实变化".into());
    }
    let after = (|| {
        trees::stop(frame, &window.facts)?;
        let (descriptor, image) = window.capture("recover-after")?;
        if !before_image.is_null() && before_image == image["image"] {
            Ok(descriptor)
        } else {
            Err("源回收前后完整像变化，保留现场并禁止发布或重放".into())
        }
    })();
    let (before, baseline, after) = match (before, after) {
        (Ok((before, baseline, _)), Ok(after)) => (before, baseline, after),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => return Err(error),
        (Err(error), Err(stopped)) => {
            return Err(format!(
                "源回收基线失败：{error}；精确树回收或后像也失败：{stopped}；保留现场，禁止重放"
            )
            .into());
        }
    };
    let stopped = existing::status(frame, start)?;
    if document::object(&stopped["roles"], "回收后源角色")?
        .values()
        .any(|row| row["state"] != "stopped")
    {
        return Err("源回收后缺少完整双角色退出证明".into());
    }
    if frame.verifier_stopped()? != verifier {
        return Err("回收后来源验收生产者退出事实变化".into());
    }
    common::bound_file(frame.backend, &baseline, "源恢复完整基线")?;
    common::bound_file(frame.backend, &descriptor, "源回收请求")?;
    window.checkpoint()?;
    Ok(
        json!({"status":"seed_source_generation_abandoned","request":descriptor,"intent":observed["intent"],
        "baseline":baseline,"source_verifier":verifier,"before":before,"after":after,"runtime":stopped,
        "remote_writes":0,"source_generation_published":false,"replay_allowed":false,"restore_qualified":false}),
    )
}

fn before<T: Transport + Sync>(
    window: &Window<'_, '_, T>,
    start: &Value,
    verifier: &Value,
    observed: &mut Value,
) -> Result<(Value, Value, Value)> {
    let (before, image) = window.capture("recover-before")?;
    *observed = image["image"].clone();
    if start["status"] != "passed" {
        return Err("未发布 START 的失败代次缺少不可变运行前像；只回收树，不能声明零漂移".into());
    }
    let receipt = common::read_bound(window.frame.backend, &start["result"], "原 START 结果")?;
    let mut baseline = receipt["running"].clone();
    if verifier["status"] == "verified_stopped" {
        let output = window.output()?;
        let descriptor = common::binding(&output.join("verification/source-runtime.json"))?;
        let verified = saved::verify(window.frame.backend, &descriptor, false)?;
        if verified["receipt"]["source_generation"] != start["result"] {
            return Err("回收会话后像没有绑定同一 START".into());
        }
        baseline = verified["receipt"]["after"].clone();
    }
    let original = images::verify(
        window.frame.backend,
        &baseline,
        &window.facts["selected"],
        &window.facts["source"]["request"],
        &receipt["source_registration"],
    )?;
    if image["image"] != original["image"] {
        return Err(
            "START 运行像或已验证会话后像之后存在未知写入；仅回收树，禁止声明零漂移或重放".into(),
        );
    }
    Ok((before, baseline, image["image"].clone()))
}
