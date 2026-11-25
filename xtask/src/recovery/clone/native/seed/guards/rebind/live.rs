//! 只读重绑定现场核验；不会向进程发送信号，也不写远端资源。
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::Value;
use std::path::Path;

pub(crate) fn transition(backend: &Path, input: &Value) -> Result<Value> {
    document::exact(input, &["original", "current"], "重绑定现场")?;
    let original = &input["original"];
    let current = &input["current"];
    super::super::transition(original, current)?;
    let directory = directory_snapshot(backend, original)?;
    for _ in 0..2 {
        if directory_snapshot(backend, original)? != directory {
            return Err("重绑定期间目录身份变化".into());
        }
        stopped(&original["storage"]["identity"])?;
        for binding in [
            &original["request"],
            &original["restart_result"],
            &current["restart_result"],
        ] {
            common::bound_file(backend, binding, "重绑定原始文件")?;
        }
    }
    Ok(Value::Null)
}

pub(crate) fn quiet(backend: &Path, input: &Value) -> Result<Value> {
    document::exact(input, &["registration", "request"], "重绑定生产者静默")?;
    let descriptor = &input["registration"]["producer_lineage"];
    let lineage = common::read_bound(backend, descriptor, "生产者完整谱系")?;
    bindings(backend, &lineage)?;
    for item in document::array(&lineage["identities"], "生产者身份集合")? {
        stopped(&item["identity"])?;
    }
    let runtime_descriptor = &input["request"]["runtime"];
    let runtime = common::read_bound(backend, runtime_descriptor, "生产者运行绑定")?;
    common::require_closed_runtime_ports(
        document::text(&input["request"]["source"]["api_url"], "来源 API 端点")?,
        document::text(&runtime["worker_ready_url"], "来源 Worker 端点")?,
    )?;
    // 端口观察后复核谱系字节，不能以早先的停止身份掩盖刚发生的历史变化。
    common::bound_file(backend, descriptor, "生产者完整谱系")?;
    common::bound_file(backend, runtime_descriptor, "生产者运行绑定")?;
    bindings(backend, &lineage)?;
    Ok(Value::Null)
}

pub(in super::super) fn restart(backend: &Path, input: &Value) -> Result<Value> {
    document::exact(input, &["original"], "发布源重启现场")?;
    let original = &input["original"];
    let directory = directory_snapshot(backend, original)?;
    for _ in 0..2 {
        if directory_snapshot(backend, original)? != directory {
            return Err("重启核验期间目录身份变化".into());
        }
        common::bound_file(backend, &original["request"], "发布源固定存储请求")?;
        stopped(&original["storage"]["identity"])?;
    }
    Ok(Value::Null)
}

fn directory_snapshot(backend: &Path, original: &Value) -> Result<(u64, u64)> {
    let value = &original["data_directory"];
    document::exact(value, &["path", "device", "inode"], "原目录身份")?;
    // 原收据的 Python 卷标身份由采集器完整核对；本侧仅防止原生观察期间目录替换。
    let path = Path::new(document::text(&value["path"], "原目录路径")?);
    let path = common::local_directory(backend, path, "原目录")?;
    common::device_inode_for(&path, &std::fs::symlink_metadata(&path)?)
}

fn bindings(backend: &Path, value: &Value) -> Result<()> {
    match value {
        Value::Object(object)
            if object.len() == 3
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|key| object.contains_key(*key)) =>
        {
            common::bound_file(backend, value, "生产者谱系文件")?;
        }
        Value::Object(object) => {
            for item in object.values() {
                bindings(backend, item)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                bindings(backend, item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn stopped(expected: &Value) -> Result<()> {
    stopped_with(expected, common::process_identity)
}

fn stopped_with(
    expected: &Value,
    inspect: impl FnOnce(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let identity = common::validate_process_identity(expected)?;
    let started = document::text(&identity["started"], "历史生产者创建代次")?;
    if started.len() > 18 || started.starts_with('0') {
        return Err("历史生产者创建代次无效".into());
    }
    let original: u128 = started.parse()?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("历史生产者 PID 无效")?)?;
    let Some(current) = inspect(pid)? else {
        return Ok(());
    };
    common::validate_process_identity(&current)?;
    let now = document::text(&current["started"], "当前生产者创建代次")?;
    if now.len() > 18
        || now.starts_with('0')
        || current["pid"] != identity["pid"]
        || now.parse::<u128>()? <= original
    {
        return Err("登记 producer 尚未停止或当前创建代次不明确".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/guards/rebind/live/tests.rs"]
mod tests;
