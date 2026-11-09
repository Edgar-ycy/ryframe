//! 只接受 tree 发布前已经完整关闭的原 actor；不补树、不发 stop、不续建。
use super::*;

pub(in crate::recovery::fixture_control::services) fn read(
    backend: &Path,
    run: &Path,
    attempt: &Value,
) -> Result<Value> {
    let output = run.join("rustfs");
    check(
        attempt["status"] == "failed"
            && attempt["result"].is_null()
            && !output.join("rustfs-tree.json").try_exists()?
            && !output.join("ready.json").try_exists()?,
        "早期关闭只接受未发布完整树或成功就绪的失败原代次",
    )?;
    let records = records::load(backend, run, attempt)?;
    let operation = document::text(&records["operation_id"], "原操作 ID")?;
    for suffix in ["control", "result"] {
        check(
            !output
                .join(format!("rustfs-tree-{operation}-{suffix}.json"))
                .try_exists()?,
            "早期关闭包含完整树控制或结果，副作用范围未知",
        )?;
    }
    let stopped = output.join(format!("rustfs-members-{operation}-stopped.json"));
    let proof = document::read(&stopped)?;
    check(
        proof["status"] == "stopped" && proof["error_type"].is_null(),
        "早期关闭缺少实际完整成员成功退出证明",
    )?;
    // records 已核对每个原成员与 monitor 的身份消失；此处只独立绑定原关闭文件。
    let mut bindings = records["bindings"].clone();
    bindings["rustfs_intent.json"] = common::binding(&output.join("intent.json"))?;
    bindings["rustfs_early_stopped"] = common::binding(&stopped)?;
    for name in ["actor-cancel.json", "actor-failed.json"] {
        if output.join(name).try_exists()? {
            bindings[format!("rustfs_{name}")] = common::binding(&output.join(name))?;
        }
    }
    for key in ["api_url", "console_url"] {
        common::require_closed_port(document::text(
            &proof_request(backend, attempt)?[key],
            "原 RustFS 端点",
        )?)?;
    }
    Ok(
        json!({"kind":"native-rustfs-early-closed","state":"stopped","backend":backend,"run":run,
        "attempt":attempt,"operation_id":operation,"bindings":bindings}),
    )
}

fn proof_request(backend: &Path, attempt: &Value) -> Result<Value> {
    common::read_bound(backend, &attempt["sources"]["request"], "早期关闭原请求")
}

pub(in crate::recovery::fixture_control::services) fn recheck(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "kind",
            "state",
            "backend",
            "run",
            "attempt",
            "operation_id",
            "bindings",
        ],
        "早期实际关闭",
    )?;
    let current = read(
        Path::new(document::text(&value["backend"], "早期协调器")?),
        Path::new(document::text(&value["run"], "早期运行目录")?),
        &value["attempt"],
    )?;
    check(current == *value, "原早期关闭证据在复核期间变化")
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/fixture_control/services/storage_closed.rs"]
mod tests;
