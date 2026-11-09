//! 按既有顺序解释已落盘阶段证据，全部续作资格仍由原模型核验。
use super::super::super::{prepare_state, resume, resume_baseline, resume_state};
use super::*;

pub(super) fn observe(window: &Window<'_>) -> Result<(&'static str, Value)> {
    let target = &window.target;
    if target.join("reconciliation-completed.json").is_file() {
        return Ok((
            "preflight-reconciled",
            binding::reconciliation(window.backend, target)?["binding"].clone(),
        ));
    }
    if target.join("initialized.json").is_file() {
        if published::unresolved(window.backend, target) {
            return Ok(("initialized-failure", Value::Null));
        }
        super::super::snapshot::verify(
            window.backend,
            window.workspace,
            &window.target,
            &window.binding,
            "initialized",
        )?;
        published::unlocked_history(window.backend, &target.join("initialized.json"))?;
        return Ok(("initialized", Value::Null));
    }
    if target.join("initialize.lock").exists() || target.join("initialize.started.json").exists() {
        let state = initialize_resume(window)?;
        return Ok(if state["resumable"] == true {
            ("initialize-resume", state)
        } else {
            (
                "initialize-blocked",
                json!(window.workspace.join("prepared-files.json").exists()),
            )
        });
    }
    if target.join("prepare.json").is_file() {
        if window.workspace.join("prepared-files.json").is_file() {
            if published::unresolved(window.backend, target) {
                return Err("prepare 失败尚未由显式续作确认".into());
            }
            let descriptor = &window.value["request"];
            let prepared = document::read(&target.join("prepare.json"))?;
            if prepared["request"] != *descriptor {
                return Err("prepare 不属于固定 registration".into());
            }
            let files = prepared_files(window)?;
            resume_baseline::prepared(window.backend, target, &files, descriptor)?;
            super::super::snapshot::verify(
                window.backend,
                window.workspace,
                &window.target,
                &window.binding,
                "prepared",
            )?;
            return Ok(("prepared", Value::Null));
        }
        let state = prepare_state::state(window.backend, target, &window.value["request"])?;
        if state["resumable"] != true {
            return Err("prepare 发布前证据无效".into());
        }
        return Ok(("prepare-publication", Value::Null));
    }
    let state = prepare_state::state(window.backend, target, &window.value["request"])?;
    Ok(if state["resumable"] == true {
        ("prepare-interrupted", Value::Null)
    } else {
        ("prepare-blocked", state["reason"].clone())
    })
}

fn prepared_files(window: &Window<'_>) -> Result<Value> {
    let path = window.workspace.join("prepared-files.json");
    let descriptor = common::binding(&path)?;
    let value = common::read_bound(window.backend, &descriptor, "prepare 文件快照")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "prepare 文件快照",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-fresh-target-prepared-files"
        || value["registration"] != window.binding
        || !value["predecessor"].is_null()
        || !value["files"].is_array()
        || common::binding(&path)? != descriptor
    {
        return Err("fresh prepare 文件快照绑定无效".into());
    }
    Ok(
        json!({"descriptor":descriptor,"registration":value["registration"],"predecessor":value["predecessor"]}),
    )
}

fn initialize_resume(window: &Window<'_>) -> Result<Value> {
    let files = prepared_files(window)?;
    let bytes = serde_json::to_vec(
        &json!({"format_version":1,"operation":"inventory-resume-state",
        "backend":window.backend,"target":window.target,"request_descriptor":window.value["request"],
        "prepared_files":files,"owned_lock":false}),
    )?;
    let mut inventory = resume::execute(&bytes).ok_or("库存续作模型未接线")??;
    if inventory["resumable"] == true {
        inventory["mode"] = json!("inventory");
        return Ok(inventory);
    }
    resume_state::state(
        window.backend,
        &window.target,
        &window.value["request"],
        &files,
        false,
    )
}
