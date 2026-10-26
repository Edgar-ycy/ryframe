//! 正常源停止只发布已验证会话的零漂移后继；初始像失败仍按原规则尝试精确停止树。
use super::{frame::Frame, publication, trees, window::Window};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, export::transport::Transport, resources::common},
        source::evidence::execution::saved,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run<T: Transport + Sync>(
    frame: &Frame<'_>,
    receipt: &Path,
    transport: &T,
) -> Result<Value> {
    frame.checkpoint()?;
    let descriptor = common::binding(receipt)?;
    let verified = saved::verify(frame.backend, &descriptor, true)?;
    let window = Window::new(frame, verified["facts"].clone(), transport)?;
    let mut before_image = Value::Null;
    let before = before(&window, &descriptor, &verified, &mut before_image);
    let after = (|| {
        trees::stop(frame, &window.facts)?;
        window.checkpoint()?;
        let (after, value) = window.capture("after")?;
        if !before_image.is_null() && before_image == value["image"] {
            Ok(after)
        } else {
            Err("停止前后完整数据库、对象或 Redis 像变化，禁止发布或重放".into())
        }
    })();
    let (before, after) = match (before, after) {
        (Ok((before, _)), Ok(after)) => (before, after),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => return Err(error),
        (Err(error), Err(stopped)) => return Err(format!(
            "源停止前核验失败：{error}；精确停止或完整后像也失败：{stopped}；保留全部现场，禁止重放"
        )
        .into()),
    };
    if common::binding(receipt)? != descriptor
        || saved::verify(frame.backend, &descriptor, false)? != verified
    {
        return Err("停止后 source-runtime 静态证明与停止前不同，禁止发布".into());
    }
    publication::finish(&window)?;
    let output = window.output()?;
    let request = &window.facts["request"];
    let mut value = json!({"status":"seed_source_generation_published",
        "request":window.facts["receipt"]["request"],
        "history_length":frame.prefix.len(),"history_sha256":digest::plan_hash(&serde_json::to_value(&frame.prefix)?)?,
        "start":verified["receipt"]["source_generation"],"source_runtime":descriptor,
        "dataset_lineage":window.facts["receipt"]["dataset_lineage"],
        "source_request":common::binding(&output.join("source-request.json"))?,
        "generation_verified":common::binding(&output.join("generation-verified.json"))?,
        "runtime_evidence":common::binding(&output.join("runtime-evidence.json"))?,
        "before":window.facts["receipt"]["before"],"running":window.facts["receipt"]["running"],
        "stop_before":before,"after":after,"remote_writes":0,"restore_qualified":false});
    for key in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        value[key] = request[key].clone();
    }
    window.checkpoint()?;
    Ok(value)
}

fn before<T: Transport + Sync>(
    window: &Window<'_, '_, T>,
    descriptor: &Value,
    verified: &Value,
    observed: &mut Value,
) -> Result<(Value, Value)> {
    window.checkpoint()?;
    let (binding, image) = window.capture("stop-before")?;
    *observed = image["image"].clone();
    if image["image"] != verified["after"]["image"] {
        return Err("source verify 后存在未披露写入；停止并保留现场，禁止发布或重放".into());
    }
    window.checkpoint()?;
    if saved::verify(window.frame.backend, descriptor, true)? != *verified {
        return Err("停止前 source-runtime 验证事实变化".into());
    }
    Ok((binding, image))
}
