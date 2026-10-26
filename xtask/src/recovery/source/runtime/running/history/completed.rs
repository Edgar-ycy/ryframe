//! C70 已收尾时读取它冻结的请求；沿用既有历史规则，不重建 C69 证据。
use crate::{
    Result,
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

const REQUEST: &str = ".local-tests/stable-readiness/source-r26-current/generation-request-observed-b9af49bd-current-20260916-r3.json";
const SHA: &str = "93be10307c83a794362c74dad7d2be84ed8e6138aff92b6e282127de2a514c4c";

pub(super) fn read(backend: &Path, run: &Path, attempts: &[Value]) -> Result<Option<Value>> {
    let matches = attempts
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["number"].as_u64() == Some(69)
                && row["stage"] == "seed-runtime"
                && row["mode"] == "source-generation-recover"
        })
        .collect::<Vec<_>>();
    let [(position, record)] = matches.as_slice() else {
        return Ok(None);
    };
    let Some(successor) = attempts.get(position + 1) else {
        return Ok(None);
    };
    let previous = position
        .checked_sub(1)
        .and_then(|index| attempts.get(index))
        .ok_or("C70 缺少原 C68 阶段")?;
    if !failed(record, "source-generation-recover")
        || !failed(previous, "source-generation-start")
        || previous["number"].as_u64() != Some(68)
        || successor["number"].as_u64() != Some(70)
        || successor["stage"] != "seed-runtime"
        || successor["mode"] != "source-generation-start"
    {
        return Err("C70 已收尾历史没有继承固定 C68/C69".into());
    }
    if successor["status"] == "running" {
        return Ok(None);
    }
    finished(successor)?;
    let state_binding = common::binding(&run.join("state.json"))?;
    if state::attempts(&state::load(run, true)?)? != attempts {
        return Err("C70 已收尾历史不是当前完整账本".into());
    }
    let request = fixed(backend)?;
    let result = if successor["status"] == "passed" {
        let value = common::read_bound(backend, &successor["result"], "C70 成功结果")?;
        if value["request"] != request {
            return Err("C70 成功结果没有绑定 C69 授权的同一请求".into());
        }
        Some(successor["result"].clone())
    } else {
        let copy = run.join("g0070/request.json");
        if copy.try_exists()?
            && document::read(&copy)? != common::read_bound(backend, &request, "C70 正式请求")?
        {
            return Err("C70 失败目录没有绑定 C69 授权的同一请求".into());
        }
        None
    };
    if common::binding(&run.join("state.json"))? != state_binding
        || state::attempts(&state::load(run, true)?)? != attempts
        || fixed(backend)? != request
    {
        return Err("C70 已收尾历史或请求在核验期间变化".into());
    }
    if let Some(result) = result {
        common::bound_file(backend, &result, "C70 成功结果")?;
    }
    Ok(Some(
        json!({"record":record,"failed":previous,"records":[previous,record],
        "request":request,"receipt":null,"completed_successor":successor}),
    ))
}

pub(super) fn fixed(backend: &Path) -> Result<Value> {
    let path = backend.join(REQUEST);
    let value = common::binding(&path)?;
    if value != json!({"path":path,"bytes":3724,"sha256":SHA}) {
        return Err("C69 失败授权绑定的正式请求变化".into());
    }
    common::bound_file(backend, &value, "C69 固定正式请求")?;
    Ok(value)
}

fn failed(row: &Value, mode: &str) -> bool {
    row["stage"] == "seed-runtime"
        && row["mode"] == mode
        && row["status"] == "failed"
        && row["result"].is_null()
        && row["error_type"] == "ValueError"
}

fn finished(row: &Value) -> Result<()> {
    document::text(&row["finished_at"], "C70 完成时间")?;
    if row["status"] == "passed" && row["result"].is_object() && row["error_type"].is_null()
        || row["status"] == "failed" && row["result"].is_null() && row["error_type"].is_string()
    {
        return Ok(());
    }
    Err("C70 已收尾状态、结果或失败类型无效".into())
}
