//! 后继工具、历史代次与来源交接的缓存侧判定。
use super::super::super::{json as doc, lifecycle::state, resources::common};
use super::super::rejected;
use super::super::{history, markers};
use super::{Context, STAGE, number, path};
use crate::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(in super::super) fn tool_paths(context: &Context<'_>) -> Result<Vec<PathBuf>> {
    let root = context.directory.join(STAGE);
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('a') && name[1..].bytes().all(|v| v.is_ascii_digit()) {
            let file = entry.path().join("tool-successor.json");
            if file.exists() {
                common::common_file(context.backend, &file, "缓存工具后继")?;
                paths.push(file);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

pub(in super::super) fn plan(
    context: &mut Context<'_>,
    successor: &Value,
    lock: &Value,
    observe: bool,
) -> Result<(Value, Value, Value, Value)> {
    let (request, descriptor) = context.registration()?;
    let response=context.call("successor",json!({"request":request,"successor":successor,"owned_lock_identity":lock,"observe_current_tools":observe}))?;
    let values = doc::array(&response, "缓存后继资源")?;
    if values.len() != 3 {
        return Err(rejected("缓存后继资源端口响应无效"));
    }
    let (effective, private, source) = (&values[0], &values[1], &values[2]);
    let state = state::load(&context.directory, true)?;
    let published = state::attempts(&state)?
        .iter()
        .filter(|item| {
            item["result"] == source["review_successor"]["source_result"]
                && item["status"] == "passed"
        })
        .collect::<Vec<_>>();
    if published.len() != 1 {
        return Err(rejected("缓存后继缺少唯一已发布 seed 来源"));
    }
    let published_number = number(published[0])?;
    let previous = context
        .records(false)?
        .into_iter()
        .filter(|r| number(r).is_ok_and(|n| n < published_number))
        .collect::<Vec<_>>();
    let prior = previous
        .last()
        .ok_or("缓存工具后继必须继承 seed 发布前已确认的缓存代次")?;
    if prior["status"] != "passed"
        || !matches!(
            prior["mode"].as_str(),
            Some("restart" | "resume" | "reconcile")
        )
        || common::read_bound(context.backend, &prior["result"], "缓存原结果")?["status"]
            != "cache_ready"
    {
        return Err(rejected("缓存工具后继必须继承 seed 发布前已确认的缓存代次"));
    }
    let created = history::starts_with_tools(
        context,
        &request,
        &descriptor,
        Some(published_number),
        &Value::Null,
        observe,
    )?;
    let (_, process_output, base) = created.last().ok_or("seed 原缓存缺少已登记的创建收据")?;
    let (runtime, runtime_binding) = history::owned(context, process_output, base)?;
    let ready_path = context.output(number(prior)?).join("ready.json");
    let ready = doc::read(&ready_path)?;
    let expected = json!({"request":descriptor,"runtime":runtime_binding,"redis":runtime["redis"],"owner":ready["owner"],"controller":context.controller(prior)?,"attempt":prior["number"]});
    let result = common::read_bound(context.backend, &prior["result"], "缓存原结果")?;
    if ready != expected
        || result["ready"] != common::binding(&ready_path)?
        || !contains(&result, &expected)
    {
        return Err(rejected("seed 原缓存发布结果与实际创建代次不符"));
    }
    let owner = common::bound_file(context.backend, &ready["owner"], "缓存确认")?;
    markers::confirmation(
        context,
        owner.parent().ok_or("缓存确认目录缺失")?,
        &request,
        &descriptor,
        &runtime_binding,
    )?;
    let mut public = effective.clone();
    doc::object(effective, "缓存工具")?;
    public.as_object_mut().unwrap().remove("password");
    let proof = json!({"request":descriptor,"successor":successor,"source_result":source["review_successor"]["source_result"],"previous_result":prior["result"],"process":public,"credential_source":request["target_environment"]});
    Ok((proof, private.clone(), source.clone(), effective.clone()))
}
pub(in super::super) fn contains(value: &Value, expected: &Value) -> bool {
    expected
        .as_object()
        .is_some_and(|items| items.iter().all(|(k, v)| value.get(k) == Some(v)))
}
pub(in super::super) fn generation(
    context: &mut Context<'_>,
    request: &Value,
    output: &Path,
    lock: &Value,
    observe: bool,
) -> Result<(Value, Value)> {
    let file = output.join("tool-successor.json");
    if !file.exists() {
        return Ok((request["process"].clone(), Value::Null));
    }
    common::common_file(context.backend, &file, "缓存工具后继")?;
    let saved = doc::read(&file)?;
    let (expected, _, _, effective) = plan(context, &saved["successor"], lock, observe)?;
    if saved != expected {
        return Err(rejected("缓存工具后继收据与原请求、来源或当前工具不符"));
    }
    Ok((effective, common::binding(&file)?))
}
pub(in super::super) fn frozen_generation(
    context: &mut Context<'_>,
    request: &Value,
    descriptor: &Value,
    output: &Path,
    frozen: &Value,
) -> Result<(Value, Value)> {
    let pair = doc::array(frozen, "冻结缓存来源")?;
    if pair.len() != 2 {
        return Err(rejected("冻结缓存来源无效"));
    }
    let (process, tools) = generation(context, request, output, &Value::Null, false)?;
    if !tools.is_null() {
        let saved = common::read_bound(context.backend, &tools, "冻结缓存后继")?;
        if saved["request"] != *descriptor
            || saved["source_result"] != pair[0]
            || saved["successor"] != pair[1]
        {
            return Err(rejected("冻结缓存工具后继不属于同一发布来源和请求"));
        }
    }
    Ok((process, tools))
}
pub(in super::super) fn history_proof(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    doc::exact(
        p,
        &["attempt", "source_result", "frozen_successor"],
        "缓存历史证明",
    )?;
    let output = context.output(number(&p["attempt"])?);
    let (request, descriptor) = context.registration()?;
    let (_, tools) = if p["frozen_successor"].is_null() {
        generation(context, &request, &output, &Value::Null, true)?
    } else {
        frozen_generation(
            context,
            &request,
            &descriptor,
            &output,
            &json!([p["source_result"], p["frozen_successor"]]),
        )?
    };
    if tools.is_null() {
        return Err(rejected("seed 发布后缓存操作缺少同一来源的工具后继证明"));
    }
    let saved = common::read_bound(context.backend, &tools, "缓存历史证明")?;
    if saved["source_result"] != p["source_result"] {
        return Err(rejected("seed 发布后缓存操作缺少同一来源的工具后继证明"));
    }
    Ok(saved["successor"].clone())
}
fn source_history(
    context: &mut Context<'_>,
    source: &Value,
    current: &Value,
    mode: &str,
) -> Result<Value> {
    context.call(
        "seed-history",
        json!({"source":source,"current":current,"mode":mode}),
    )
}
pub(in super::super) fn preflight(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    doc::exact(p, &["filename", "mode"], "缓存后继预检")?;
    let mode = doc::text(&p["mode"], "缓存动作")?;
    if !context
        .directory
        .join(STAGE)
        .join("registration.json")
        .exists()
    {
        return Ok(Value::Null);
    }
    let (request, _) = context.registration()?;
    let Some(selected) = selected(context, &request, p)? else {
        return Ok(Value::Null);
    };
    let (proof, _, source, _) = plan(context, &selected, &Value::Null, true)?;
    let state = state::load(&context.directory, true)?;
    let attempts = state::attempts(&state)?;
    let later = source_history(context, &proof["source_result"], &Value::Null, mode)?;
    for item in attempts {
        if item["stage"] == STAGE
            && context
                .output(number(item)?)
                .join("tool-successor.json")
                .exists()
            && history_proof(
                context,
                &json!({"attempt":item,"source_result":proof["source_result"],"frozen_successor":null}),
            )? != selected
        {
            return Err(rejected("缓存恢复必须继续已经登记的同一 successor"));
        }
    }
    let published = attempts
        .iter()
        .find(|r| r["result"] == proof["source_result"])
        .ok_or("缓存发布来源缺失")?;
    let handed = attempts.iter().any(|r| {
        r["stage"] == "seed-runtime"
            && matches!(r["mode"].as_str(), Some("source-rebind" | "arm-input"))
            && r["number"].as_u64() > published["number"].as_u64()
    });
    if mode == "restart"
        && attempts
            .iter()
            .any(|r| r["stage"] == "seed-runtime" && r["mode"] == "source-generation-reboot-close")
    {
        context.call("restart-authorization", json!({"current":null}))?;
    } else {
        let segment = context.call(
            "seed-segment",
            json!({"source":proof["source_result"],"current":null}),
        )?;
        if handed && (mode != "restart" || segment["phase"] != "storage-ready")
            || doc::array(&later, "缓存后继历史")?
                .iter()
                .any(|r| r["mode"] == "arm-input")
        {
            return Err(rejected("已交接 seed 来源只能从封存存储后执行唯一缓存重启"));
        }
    }
    context.call("quiet", source)?;
    Ok(proof)
}
pub(in super::super) fn restart_guard(context: &mut Context<'_>, p: &Value) -> Result<()> {
    doc::exact(p, &["number"], "缓存重启守卫")?;
    let number = p["number"].as_u64().ok_or("缓存阶段序号无效")?;
    let state = state::load(&context.directory, true)?;
    let attempts = state::attempts(&state)?;
    let attempt = attempts.last().ok_or("缓存重启没有阶段")?;
    if attempt["number"].as_u64() != Some(number)
        || attempt["stage"] != STAGE
        || attempt["status"] != "running"
    {
        return Err(rejected("seed 缓存恢复必须处于当前唯一持锁阶段"));
    }
    if attempts
        .iter()
        .any(|r| r["stage"] == "seed-runtime" && r["mode"] == "source-generation-reboot-close")
    {
        context.call(
            "restart-authorization",
            json!({"current":"cache-target:restart"}),
        )?;
        return Ok(());
    }
    let saved = doc::read(&context.output(number).join("tool-successor.json"))?;
    let (proof, _, source, _) = plan(context, &saved["successor"], &Value::Null, true)?;
    if proof != saved {
        return Err(rejected("当前缓存工具后继登记变化"));
    }
    source_history(context, &proof["source_result"], &json!(number), "restart")?;
    let segment = context.call(
        "seed-segment",
        json!({"source":proof["source_result"],"current":number}),
    )?;
    if attempts
        .iter()
        .any(|r| r["stage"] == "seed-runtime" && r["mode"] == "source-rebind")
        && segment["phase"] != "cache-running"
    {
        return Err(rejected("缓存重启不属于封存来源的唯一分段续作"));
    }
    context.call("quiet", source)?;
    Ok(())
}
fn selected(context: &Context<'_>, request: &Value, p: &Value) -> Result<Option<Value>> {
    let selected = if !p["filename"].is_null() {
        let file = path(&p["filename"])?;
        common::common_file(context.backend, &file, "缓存后继输入")?;
        let supplied = doc::read(&file)?;
        if supplied == *request {
            if !tool_paths(context)?.is_empty() {
                return Err(rejected("缓存工具已换代，不能重新使用原工具请求"));
            }
            return Ok(None);
        }
        if supplied["kind"] != "devex-clone-seed-review-successor" {
            return Err(rejected("缓存请求只接受原登记或正式 seed successor"));
        }
        common::binding(&file)?
    } else {
        let paths = tool_paths(context)?;
        let Some(last) = paths.last() else {
            return Ok(None);
        };
        doc::read(last)?["successor"].clone()
    };
    Ok(Some(selected))
}
