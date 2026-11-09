//! 不可变 post-copy 登记代次及 seed 的历史前置证明。
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{FIELDS, latest};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, lifecycle::state, resources::common},
};

pub(crate) struct Post {
    pub(crate) request: Value,
    pub(crate) descriptor: Value,
    pub(crate) runtime: PathBuf,
}

pub(crate) fn post(
    backend: &Path,
    run: &Path,
    requested: Option<&Value>,
    current: Option<u64>,
    cleanup: bool,
) -> Result<Post> {
    let saved = state::load(run, !cleanup)?;
    let attempts = state::attempts(&saved)?;
    let mut candidates = post_generations(backend, run, &saved, cleanup)?;
    let selected = if let Some(requested) = requested {
        candidates
            .iter()
            .position(|value| value.descriptor == *requested)
            .ok_or("历史 post-copy 登记不属于本 run 的已发布代次")?
    } else {
        candidates.len() - 1
    };
    if !cleanup {
        let last = attempts.iter().rfind(|row| {
            row["stage"] == "post-copy"
                && matches!(row["mode"].as_str(), Some("register" | "amend"))
                && row["number"].as_u64() != current
        });
        if last.is_none_or(|row| row["status"] != "passed") || selected + 1 != candidates.len() {
            return Err("最新 post-copy 登记或 amendment 未通过，禁止回退".into());
        }
    }
    if state::load(run, !cleanup)? != saved {
        return Err("post-copy 登记历史在读取期间变化".into());
    }
    Ok(candidates.remove(selected))
}

pub(super) fn post_generations(
    backend: &Path,
    run: &Path,
    saved: &Value,
    cleanup: bool,
) -> Result<Vec<Post>> {
    let attempts = state::attempts(saved)?;
    let mut candidates = vec![base(backend, run, attempts)?];
    for record in attempts.iter().filter(|row| {
        row["stage"] == "post-copy" && row["mode"] == "amend" && row["status"] == "passed"
    }) {
        if candidates.len() > 9999 {
            return Err("post-copy amendment 代次无效".into());
        }
        let next = amended(
            backend,
            run,
            saved,
            candidates.last().ok_or("缺少原始登记")?,
            record,
            candidates.len(),
            cleanup,
        )?;
        candidates.push(next);
    }
    Ok(candidates)
}

fn base(backend: &Path, run: &Path, attempts: &[Value]) -> Result<Post> {
    let passed = attempts
        .iter()
        .filter(|row| {
            row["stage"] == "post-copy" && row["mode"] == "register" && row["status"] == "passed"
        })
        .collect::<Vec<_>>();
    let [record] = passed.as_slice() else {
        return Err("post-copy 原始登记必须恰有一个发布成功结果".into());
    };
    let path = run.join("post-copy.json");
    let descriptor = common::binding(&path)?;
    let result = common::read_bound(backend, &record["result"], "post-copy 登记结果")?;
    if result["status"] != "post_copy_registered" || result["registration"] != descriptor {
        return Err("post-copy 原始登记与发布结果不同".into());
    }
    let request = document::read(&path)?;
    if common::binding(&path)? != descriptor {
        return Err("post-copy 原始登记在读取期间变化".into());
    }
    Ok(Post {
        request,
        descriptor,
        runtime: run.join("post-copy").join("runtime"),
    })
}

fn amended(
    backend: &Path,
    run: &Path,
    saved: &Value,
    previous: &Post,
    record: &Value,
    sequence: usize,
    cleanup: bool,
) -> Result<Post> {
    let result = common::read_bound(backend, &record["result"], "post-copy amendment 发布结果")?;
    document::exact(
        &result,
        &[
            "status",
            "registration",
            "sequence",
            "predecessor",
            "runtime_directory",
            "remote_writes",
            "worker_must_remain_stopped",
            "restore_qualified",
        ],
        "post-copy amendment 发布结果",
    )?;
    let descriptor = result["registration"].clone();
    let path = common::bound_file(backend, &descriptor, "post-copy amendment 登记")?;
    let amendment = document::read(&path)?;
    document::exact(
        &amendment,
        &[
            "format_version",
            "kind",
            "sequence",
            "run_manifest",
            "predecessor",
            "request_sha256",
            "api_environment",
            "invalidated_runtime",
        ],
        "post-copy amendment 登记",
    )?;
    let runtime = run.join("post-copy").join(format!("runtime-{sequence:04}"));
    if path
        != run
            .join("post-copy/amendments")
            .join(format!("{sequence:04}.json"))
        || amendment["format_version"].as_u64() != Some(1)
        || amendment["kind"] != "devex-clone-post-copy-amendment"
        || amendment["sequence"].as_u64() != Some(sequence as u64)
        || amendment["run_manifest"] != state::manifest_binding(run)?
        || amendment["predecessor"] != previous.descriptor
        || result["status"] != "post_copy_amended"
        || result["sequence"] != amendment["sequence"]
        || result["predecessor"] != previous.descriptor
        || result["runtime_directory"].as_str() != runtime.to_str()
        || result["remote_writes"].as_u64() != Some(0)
        || result["worker_must_remain_stopped"] != true
        || result["restore_qualified"] != false
    {
        return Err("post-copy amendment 顺序、来源或发布结果无效".into());
    }
    if !cleanup {
        invalidation(
            backend,
            run,
            saved,
            previous,
            &amendment["invalidated_runtime"],
        )?;
        common::bound_file(backend, &amendment["api_environment"], "amendment API 配置")?;
    }
    let mut request = previous.request.clone();
    request["api_environment"] = amendment["api_environment"].clone();
    if amendment["request_sha256"].as_str() != Some(digest::plan_hash(&request)?.as_str()) {
        return Err("post-copy amendment 不能还原完整修正请求".into());
    }
    common::bound_file(backend, &descriptor, "post-copy amendment 登记")?;
    Ok(Post {
        request,
        descriptor,
        runtime,
    })
}

fn invalidation(
    backend: &Path,
    run: &Path,
    saved: &Value,
    previous: &Post,
    value: &Value,
) -> Result<()> {
    document::exact(
        value,
        &[
            "prepare",
            "handoff",
            "runtime",
            "binaries",
            "api_process",
            "producer_history",
            "failed_start",
            "stopped",
        ],
        "post-copy 旧运行证据",
    )?;
    for (field, name) in [
        ("handoff", "handoff.json"),
        ("runtime", "runtime.json"),
        ("binaries", "binaries.json"),
        ("api_process", "api.json"),
        ("producer_history", "producer-history.json"),
    ] {
        if common::bound_file(backend, &value[field], "post-copy 旧运行文件")?
            != previous.runtime.join(name)
        {
            return Err("post-copy amendment 的旧 runtime 证据路径无效".into());
        }
    }
    document::exact(
        &value["failed_start"],
        &["attempt", "controller", "failure"],
        "失败启动证据",
    )?;
    document::exact(&value["stopped"], &["attempt", "result"], "停止证据")?;
    let failed = value["failed_start"]["attempt"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("失败启动序号无效")?;
    let stopped = value["stopped"]["attempt"]
        .as_u64()
        .filter(|v| *v > failed)
        .ok_or("停止序号无效")?;
    let attempts = state::attempts(saved)?;
    let row = |number| {
        attempts
            .iter()
            .find(|row| row["number"].as_u64() == Some(number))
    };
    let prepare = attempts
        .iter()
        .filter(|row| {
            row["stage"] == "post-copy"
                && row["mode"] == "prepare"
                && row["status"] == "passed"
                && row["result"] == value["prepare"]
        })
        .count();
    let check = |row: Option<&Value>, mode: &str, status: &str| {
        row.is_some_and(|row| {
            row["stage"] == "runtime-target" && row["mode"] == mode && row["status"] == status
        })
    };
    if prepare != 1
        || !check(row(failed), "start", "failed")
        || !check(row(stopped), "stop", "passed")
        || row(stopped).and_then(|row| row.get("result")) != Some(&value["stopped"]["result"])
        || common::bound_file(backend, &value["prepare"], "旧 prepare")?.parent()
            != Some(run.join("results").as_path())
        || common::bound_file(backend, &value["failed_start"]["controller"], "旧控制器")?
            != state::controller_path(run, failed)
        || common::bound_file(backend, &value["failed_start"]["failure"], "旧失败证据")?
            != run.join(format!("failure-{failed:04}.json"))
        || common::bound_file(backend, &value["stopped"]["result"], "旧停止结果")?
            != state::result_path(run, stopped)
    {
        return Err("post-copy amendment 的旧阶段证据链无效".into());
    }
    Ok(())
}

pub(crate) fn validate(backend: &Path, run: &Path, request: &Value) -> Result<Post> {
    document::exact(request, FIELDS, "seed 登记")?;
    if request["format_version"].as_u64() != Some(1)
        || request["kind"] != "devex-clone-seed-runtime"
        || request["run_manifest"] != state::manifest_binding(run)?
    {
        return Err("seed 必须绑定同 run 的固定 post-copy".into());
    }
    let active = post(backend, run, None, None, false)?;
    if request["post_copy"] != active.descriptor {
        return Err("seed 必须绑定当前 post-copy 登记代次".into());
    }
    let saved = state::load(run, true)?;
    let verified = latest(
        &saved,
        "post-copy",
        &["register", "prepare", "schedules", "reconcile", "verify"],
        None,
        false,
    )?;
    let result = common::read_bound(backend, &verified["result"], "post-copy 业务验证")?;
    if verified["mode"] != "verify"
        || verified["result"] != request["post_verify"]
        || result["status"] != "post_copy_existing_data_verified"
        || result["registration"] != request["post_copy"]
        || result["worker_must_remain_stopped"] != true
    {
        return Err("seed 必须取得最新同 run 的真实业务读取成功结果".into());
    }
    let copied = latest(&saved, "copy", &["run", "resume", "reconcile"], None, false)?;
    if copied["result"] != active.request["copy_stage_receipt"] {
        return Err("登记后复制阶段或账本发生变化".into());
    }
    for field in [
        "copy_stage_receipt",
        "copy_result",
        "ledger_head",
        "backend_build",
        "api_environment",
    ] {
        common::bound_file(backend, &active.request[field], "seed 历史文件")?;
    }
    if request["node"] != active.request["node"] {
        return Err("身份工具必须复用固定 post-copy 工具".into());
    }
    if state::load(run, true)? != saved {
        return Err("seed 历史在读取期间变化".into());
    }
    Ok(active)
}
