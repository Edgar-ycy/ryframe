//! 两个验收标记的完整前后像与未知写入恢复；从不自动重放。
use super::super::{digest, json as doc, resources::common};
use super::rejected;
use super::stage::{Context, STAGE};
use crate::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(super) fn images(markers: &Value) -> (Value, Value) {
    (
        json!({"keys":[],"owner":null,"sentinel":null}),
        json!({"keys":[markers["ownership_key"]],"owner":markers["ownership_value"],"sentinel":markers["sentinel_value"]}),
    )
}
pub(super) fn classify(markers: &Value, observed: &Value) -> &'static str {
    let (before, after) = images(markers);
    if *observed == before {
        "before"
    } else if *observed == after {
        "after"
    } else {
        "mismatch"
    }
}
fn intent(
    context: &Context<'_>,
    path: &Path,
    request: &Value,
    descriptor: &Value,
    runtime: &Value,
) -> Result<Value> {
    common::bound_file(context.backend, runtime, "缓存运行")?;
    let value = doc::read(&path.join("intent.json"))?;
    let before = common::bound_file(context.backend, &value["before"], "缓存完整前像")?;
    if before != path.join("before.json") || doc::read(&before)? != images(&request["markers"]).0 {
        return Err(rejected("缓存标记意图缺少当前写入的完整空前像"));
    }
    let expected = json!({"request":descriptor,"runtime":runtime,"before":common::binding(&before)?,"operation":"MSETNX","markers":request["markers"],"automatic_retry":false});
    if value != expected {
        return Err(rejected("缓存标记意图不属于固定请求和实际 Redis 代次"));
    }
    common::binding(&path.join("intent.json"))
}
pub(super) fn confirmation(
    context: &Context<'_>,
    path: &Path,
    request: &Value,
    descriptor: &Value,
    runtime: &Value,
) -> Result<Value> {
    let intent = intent(context, path, request, descriptor, runtime)?;
    let result = doc::read(&path.join("confirmed.json"))?;
    doc::exact(
        &result,
        &["intent", "observed", "response", "reconciled"],
        "缓存标记确认",
    )?;
    let observed = common::bound_file(context.backend, &result["observed"], "缓存完整后像")?;
    if result["intent"] != intent
        || observed.parent() != Some(path)
        || classify(&request["markers"], &doc::read(&observed)?) != "after"
    {
        return Err(rejected("缓存标记确认不是当前意图的完整后像"));
    }
    if !result["response"].is_null() {
        let response = common::bound_file(context.backend, &result["response"], "缓存原子响应")?;
        if response != path.join("response.json")
            || doc::read(&response)? != json!({"result":1})
            || result["reconciled"] != false
        {
            return Err(rejected("缓存标记原子写入响应证据不符"));
        }
    } else {
        let name = observed
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("缓存观察文件名无效")?;
        if result["reconciled"] != true
            || !regex::Regex::new(r"^observed-[0-9]{4}\.json$")?.is_match(name)
        {
            return Err(rejected("缓存标记确认缺少响应或显式核对证据"));
        }
        if doc::read(&path.join(name.replace("observed-", "reconcile-")))?
            != json!({"intent":intent,"observed":result["observed"],"state":"after"})
        {
            return Err(rejected("缓存标记确认不是同次完整后像核对"));
        }
    }
    common::binding(&path.join("confirmed.json"))
}
fn resumable(
    context: &Context<'_>,
    path: &Path,
    request: &Value,
    descriptor: &Value,
    runtime: &Value,
) -> Result<()> {
    let intent = intent(context, path, request, descriptor, runtime)?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let file = entry?.path();
        let name = file.file_name().and_then(|v| v.to_str()).unwrap_or("");
        if name.starts_with("reconcile-") && name.ends_with(".json") {
            common::common_file(context.backend, &file, "缓存核对证据")?;
            files.push(file);
        }
    }
    files.sort();
    let file = files.last().ok_or("未知缓存写入必须先显式 reconcile")?;
    let proof = doc::read(file)?;
    let observed = common::bound_file(context.backend, &proof["observed"], "缓存核对前像")?;
    let expected_name = file
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("缓存核对文件无效")?
        .replace("reconcile-", "observed-");
    if proof != json!({"intent":intent,"observed":proof["observed"],"state":"before"})
        || observed != path.join(expected_name)
        || classify(&request["markers"], &doc::read(&observed)?) != "before"
    {
        return Err(rejected("只有明确核对为完整前像才允许显式 resume"));
    }
    Ok(())
}
fn owners(
    context: &Context<'_>,
    request: &Value,
    descriptor: &Value,
    runtime: &Value,
) -> Result<Vec<PathBuf>> {
    let mut groups: BTreeMap<String, (Value, Vec<PathBuf>)> = BTreeMap::new();
    for attempt in context.records(true)? {
        let path = context
            .output(super::stage::number(&attempt)?)
            .join("owner");
        if path.join("intent.json").exists() {
            let value = doc::read(&path.join("intent.json"))?;
            if value["request"] != *descriptor {
                return Err(rejected("历史缓存标记意图不属于固定请求"));
            }
            let reference = &value["runtime"];
            let generation = common::bound_file(context.backend, reference, "缓存标记代次")?;
            if generation.file_name().and_then(|v| v.to_str()) != Some("runtime.json")
                || generation.parent().and_then(Path::parent)
                    != Some(context.directory.join(STAGE).as_path())
            {
                return Err(rejected("标记意图引用其他运行的缓存代次"));
            }
            groups
                .entry(digest::plan_hash(reference)?)
                .or_insert_with(|| (reference.clone(), vec![]))
                .1
                .push(path);
        }
    }
    let mut selected = Vec::new();
    for (reference, paths) in groups.values() {
        for earlier in &paths[..paths.len() - 1] {
            resumable(context, earlier, request, descriptor, reference)?;
        }
        if reference == runtime {
            selected = paths.clone();
        } else {
            confirmation(
                context,
                paths.last().unwrap(),
                request,
                descriptor,
                reference,
            )?;
        }
    }
    Ok(selected)
}
pub(super) fn require_owners(
    context: &Context<'_>,
    request: &Value,
    descriptor: &Value,
) -> Result<()> {
    owners(context, request, descriptor, &Value::Null).map(|_| ())
}
fn guard(context: &mut Context<'_>, guard: &Value) -> Result<()> {
    super::stage::execute::guard(context, guard)
}
fn call(context: &mut Context<'_>, request: &Value, parts: Value, guard: &Value) -> Result<Value> {
    context.call(
        "redis",
        json!({"request":request,"parts":parts,"guard":guard}),
    )
}
fn capture(context: &mut Context<'_>, request: &Value, guard: &Value) -> Result<Value> {
    let markers = &request["markers"];
    let namespace = doc::text(&markers["namespace"], "缓存 namespace")?;
    let mut cursor = "0".to_string();
    let mut seen = BTreeSet::new();
    let mut found = BTreeSet::new();
    let mut completed = false;
    for _ in 0..10000 {
        let response = call(
            context,
            request,
            json!([
                "SCAN",
                cursor,
                "MATCH",
                format!("{namespace}*"),
                "COUNT",
                "1000"
            ]),
            guard,
        )?;
        let rows = doc::array(&response, "Redis scoped SCAN 响应")?;
        if rows.len() != 2 {
            return Err(rejected("Redis scoped SCAN 响应结构不同"));
        }
        cursor = doc::text(&rows[0], "Redis SCAN 游标")?.into();
        if cursor.is_empty() || !cursor.bytes().all(|v| v.is_ascii_digit()) {
            return Err(rejected("Redis scoped SCAN 游标无效"));
        }
        for key in doc::array(&rows[1], "Redis scoped SCAN 键")? {
            let key = doc::text(key, "Redis scoped SCAN 键")?;
            if !key.starts_with(namespace) {
                return Err(rejected("Redis scoped SCAN 返回越界键"));
            }
            found.insert(key.to_string());
        }
        if found.len() > 10000 {
            return Err(rejected("Redis namespace 超出验收观察上限"));
        }
        if cursor == "0" {
            completed = true;
            break;
        }
        if !seen.insert(cursor.clone()) {
            return Err(rejected("Redis scoped SCAN 游标循环"));
        }
    }
    if !completed {
        return Err(rejected("Redis scoped SCAN 未完成"));
    }
    let owner = call(
        context,
        request,
        json!(["GET", markers["ownership_key"]]),
        guard,
    )?;
    let sentinel = call(
        context,
        request,
        json!(["GET", markers["sentinel_key"]]),
        guard,
    )?;
    Ok(json!({"keys":found,"owner":owner,"sentinel":sentinel}))
}
pub(super) fn restore(
    context: &mut Context<'_>,
    p: &Value,
    guard_spec: &Value,
) -> Result<Option<PathBuf>> {
    let request = &p["request"];
    let descriptor = &p["descriptor"];
    let runtime = &p["runtime_binding"];
    let mode = doc::text(&p["mode"], "缓存恢复动作")?;
    let previous = owners(context, request, descriptor, runtime)?;
    if let Some(last) = previous.last()
        && last.join("confirmed.json").exists()
    {
        confirmation(context, last, request, descriptor, runtime)?;
        guard(context, guard_spec)?;
        if classify(&request["markers"], &capture(context, request, guard_spec)?) != "after" {
            return Err(rejected("已确认缓存标记的完整后像发生漂移"));
        }
        return Ok(Some(last.clone()));
    }
    if mode == "reconcile" {
        return reconcile(context, p, guard_spec, &previous);
    }
    if let Some(last) = previous.last() {
        if mode != "resume" {
            return Err(rejected("未知缓存写入不能由 restart 自动重放"));
        }
        resumable(context, last, request, descriptor, runtime)?;
    }
    let output = super::stage::path(&p["output"])?.join("owner");
    common::ensure_new_directory(context.backend, &output, "缓存标记证据")?;
    guard(context, guard_spec)?;
    let before = capture(context, request, guard_spec)?;
    doc::write_new(&output.join("before.json"), &before)?;
    if classify(&request["markers"], &before) != "before" {
        return Err(rejected(
            "缓存恢复写入前 namespace 或 sentinel 不是原空前像",
        ));
    }
    guard(context, guard_spec)?;
    doc::write_new(
        &output.join("intent.json"),
        &json!({"request":descriptor,"runtime":runtime,"before":common::binding(&output.join("before.json"))?,"operation":"MSETNX","markers":request["markers"],"automatic_retry":false}),
    )?;
    let markers = &request["markers"];
    let response = call(
        context,
        request,
        json!([
            "MSETNX",
            markers["ownership_key"],
            markers["ownership_value"],
            markers["sentinel_key"],
            markers["sentinel_value"]
        ]),
        guard_spec,
    )?;
    doc::write_new(&output.join("response.json"), &json!({"result":response}))?;
    if response.as_i64() != Some(1) {
        return Err(rejected("缓存验收标记 MSETNX 未明确成功；先核对，禁止重放"));
    }
    guard(context, guard_spec)?;
    let after = capture(context, request, guard_spec)?;
    doc::write_new(&output.join("after.json"), &after)?;
    if classify(markers, &after) != "after" {
        return Err(rejected("缓存验收标记后像不匹配；必须显式核对"));
    }
    guard(context, guard_spec)?;
    doc::write_new(
        &output.join("confirmed.json"),
        &json!({"intent":common::binding(&output.join("intent.json"))?,"observed":common::binding(&output.join("after.json"))?,"response":common::binding(&output.join("response.json"))?,"reconciled":false}),
    )?;
    confirmation(context, &output, request, descriptor, runtime)?;
    Ok(Some(output))
}
fn reconcile(
    context: &mut Context<'_>,
    p: &Value,
    guard_spec: &Value,
    previous: &[PathBuf],
) -> Result<Option<PathBuf>> {
    let request = &p["request"];
    let descriptor = &p["descriptor"];
    let runtime = &p["runtime_binding"];
    let last = previous
        .last()
        .ok_or("该缓存代次尚无标记写入意图，使用显式 resume")?;
    let intent = intent(context, last, request, descriptor, runtime)?;
    guard(context, guard_spec)?;
    let observed = capture(context, request, guard_spec)?;
    let number = p["number"].as_u64().ok_or("缓存核对序号无效")?;
    let file = last.join(format!("observed-{number:04}.json"));
    doc::write_new(&file, &observed)?;
    let state = classify(&request["markers"], &observed);
    doc::write_new(
        &last.join(format!("reconcile-{number:04}.json")),
        &json!({"intent":intent,"observed":common::binding(&file)?,"state":state}),
    )?;
    guard(context, guard_spec)?;
    if state == "after" {
        if !last.join("confirmed.json").exists() {
            doc::write_new(
                &last.join("confirmed.json"),
                &json!({"intent":intent,"observed":common::binding(&file)?,"response":null,"reconciled":true}),
            )?;
        }
        confirmation(context, last, request, descriptor, runtime)?;
        return Ok(Some(last.clone()));
    }
    if state != "before" {
        return Err(rejected("缓存未知写入既非完整前像也非完整后像；拒绝继续"));
    }
    Ok(None)
}
