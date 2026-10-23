//! 部门写入意图、显式核对和唯一成功确认的只读证据链。
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};

use super::{common, document, model};
use crate::Result;

pub(super) fn dispatch(backend: &Path, run: &Path, method: &Value, args: &Value) -> Result<Value> {
    let method = document::text(method, "部门历史操作")?;
    let fields: &[&str] = match method {
        "intent" => &["plan", "action", "attempt"],
        "confirmation" => &["plan", "action", "target", "attempt"],
        "reconciliation" => &["plan", "action", "target", "attempt", "filename"],
        "action-history" => &["plan", "action", "target", "attempts"],
        _ => return Err("未知部门历史操作".into()),
    };
    model::exact_args(args, fields)?;
    if method == "action-history" {
        let attempts = document::array(&args["attempts"], "部门尝试目录")?
            .iter()
            .map(|value| Ok(PathBuf::from(document::text(value, "部门尝试路径")?)))
            .collect::<Result<Vec<_>>>()?;
        return action_history(backend, run, &args["action"], &args["target"], &attempts);
    }
    let attempt = Path::new(document::text(&args["attempt"], "部门尝试路径")?);
    attempt_path(backend, run, &args["action"], attempt)?;
    match method {
        "intent" => intent(backend, run, &args["action"], attempt),
        "confirmation" => confirmation(backend, run, &args["action"], &args["target"], attempt),
        "reconciliation" => {
            let filename = Path::new(document::text(&args["filename"], "部门核对路径")?);
            Ok(json!(reconciliation(
                backend,
                run,
                &args["action"],
                &args["target"],
                attempt,
                filename
            )?))
        }
        _ => Err("未知部门历史操作".into()),
    }
}

fn plan_binding(run: &Path) -> Result<Value> {
    common::binding(&run.join("seed-runtime/department-plan.json"))
}

pub(super) fn attempt_path(backend: &Path, run: &Path, action: &Value, path: &Path) -> Result<()> {
    let slot = document::text(&action["target"]["slot"], "部门目标槽")?;
    if !Regex::new(r"^tenant-(?:0[1-9]|10)$")?.is_match(slot)
        || path.parent() != Some(run.join("seed-runtime/departments").join(slot).as_path())
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                Regex::new(r"^[0-9]{4,}$").is_ok_and(|re| re.is_match(name))
                    && name.bytes().any(|b| b != b'0')
            })
    {
        return Err("部门尝试目录不属于当前租户动作".into());
    }
    common::local_directory(backend, path, "部门尝试目录")?;
    Ok(())
}

pub(super) fn intent(backend: &Path, run: &Path, action: &Value, attempt: &Path) -> Result<Value> {
    let value = read(backend, &attempt.join("intent.json"))?;
    let expected = json!({"plan":plan_binding(run)?,"target":action["target"],
        "before":common::binding(&attempt.join("before.json"))?,"body":action["body"],
        "operation":"post_system_depts","idempotency_key":value["idempotency_key"],"automatic_retry":false});
    if value != expected
        || !value["idempotency_key"].as_str().is_some_and(|key| {
            Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
                .is_ok_and(|re| re.is_match(key))
        })
    {
        return Err("部门写入意图不同于固定计划或缺少固定幂等键".into());
    }
    common::bound_file(backend, &value["before"], "部门前像")?;
    Ok(value)
}

fn read(backend: &Path, path: &Path) -> Result<Value> {
    let before = common::binding(path)?;
    let value = common::read_bound(backend, &before, "部门动作证据")?;
    if common::binding(path)? != before {
        return Err("部门动作证据读取期间发生变化".into());
    }
    Ok(value)
}

pub(super) fn reconciliation(
    backend: &Path,
    run: &Path,
    action: &Value,
    target: &Value,
    attempt: &Path,
    filename: &Path,
) -> Result<&'static str> {
    let name = filename
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("部门核对文件名无效")?;
    let suffix = name
        .strip_prefix("reconcile-")
        .ok_or("部门核对文件名无效")?;
    if filename.parent() != Some(attempt) || !Regex::new(r"^[0-9]{4,}\.json$")?.is_match(suffix) {
        return Err("部门核对不属于当前尝试".into());
    }
    let proof = read(backend, filename)?;
    document::exact(&proof, &["intent", "observed", "state"], "部门核对")?;
    let observed = common::bound_file(backend, &proof["observed"], "部门核对观察")?;
    let state = model::classify(
        action,
        &common::read_bound(backend, &proof["observed"], "部门核对观察")?,
        target,
    )?;
    if proof["intent"] != common::binding(&attempt.join("intent.json"))?
        || observed != attempt.join(format!("observed-{suffix}"))
        || proof["state"] != state
        || state == "mismatch"
    {
        return Err("部门核对不属于当前意图的完整前后像".into());
    }
    intent(backend, run, action, attempt)?;
    Ok(state)
}

pub(super) fn confirmation(
    backend: &Path,
    run: &Path,
    action: &Value,
    target: &Value,
    attempt: &Path,
) -> Result<Value> {
    let value = read(backend, &attempt.join("confirmed.json"))?;
    document::exact(
        &value,
        &[
            "plan",
            "target",
            "observed",
            "api_response_proven",
            "post_attempted",
            "evidence",
        ],
        "部门确认",
    )?;
    let response_proven = document::boolean(&value["api_response_proven"], "部门响应确认")?;
    if value["plan"] != plan_binding(run)?
        || value["target"] != action["target"]
        || value["post_attempted"] != true
        || model::classify(action, &value["observed"], target)? != "after"
    {
        return Err("部门确认缺少唯一完整后像".into());
    }
    intent(backend, run, action, attempt)?;
    let extra = if response_proven {
        "api-response"
    } else {
        "reconcile"
    };
    document::exact(
        &value["evidence"],
        &["observed", "intent", "before", extra],
        "部门确认证据集合",
    )?;
    let evidence = &value["evidence"];
    for (name, descriptor) in document::object(evidence, "部门确认证据")? {
        let path = common::bound_file(backend, descriptor, "部门确认证据")?;
        let valid_name = match name.as_str() {
            "observed" | "reconcile" => {
                path.file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|filename| {
                        Regex::new(&format!(r"^{name}-[0-9]{{4,}}\.json$"))
                            .is_ok_and(|re| re.is_match(filename))
                    })
            }
            _ => path.file_name().and_then(|v| v.to_str()) == Some(format!("{name}.json").as_str()),
        };
        if path.parent() != Some(attempt) || !valid_name {
            return Err("部门确认引用其他动作证据".into());
        }
    }
    if common::read_bound(backend, &evidence["observed"], "部门最终观察")? != value["observed"]
        || model::classify(
            action,
            &common::read_bound(backend, &evidence["before"], "部门前像")?,
            target,
        )? != "before"
    {
        return Err("部门确认内嵌后像不同或缺少原空目录前像".into());
    }
    if response_proven {
        let created = model::created_response(
            action,
            target,
            &common::read_bound(backend, &evidence["api-response"], "部门创建响应")?,
        )?;
        if created != model::image(action, &value["observed"], target)?["department"] {
            return Err("部门创建响应与最终唯一对象不同".into());
        }
    } else {
        let proof = common::read_bound(backend, &evidence["reconcile"], "部门未知写入核对")?;
        if proof
            != json!({"intent":common::binding(&attempt.join("intent.json"))?,"observed":evidence["observed"],"state":"after"})
        {
            return Err("未知部门写入缺少当前意图的后像核对".into());
        }
    }
    Ok(value)
}

pub(super) fn action_history(
    backend: &Path,
    run: &Path,
    action: &Value,
    target: &Value,
    attempts: &[PathBuf],
) -> Result<Value> {
    let slot = document::text(&action["target"]["slot"], "部门目标槽")?;
    let root = run.join("seed-runtime/departments").join(slot);
    common::local_directory(backend, &root, "部门动作根目录")?;
    let mut actual = fs::read_dir(&root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    actual.sort();
    if actual != attempts {
        return Err("部门动作目录包含未消费的尝试或证据".into());
    }
    let mut key = Value::Null;
    for (index, attempt) in attempts.iter().enumerate() {
        attempt_path(backend, run, action, attempt)?;
        if attempt.file_name().and_then(|v| v.to_str())
            != Some(format!("{:04}", index + 1).as_str())
        {
            return Err("部门动作目录不连续".into());
        }
        let names = names(backend, attempt)?;
        let final_attempt = index + 1 == attempts.len();
        if names.contains("before.json")
            && model::classify(
                action,
                &read(backend, &attempt.join("before.json"))?,
                target,
            )? != "before"
        {
            return Err("部门动作链的 POST 前像不再为空".into());
        }
        if !names.contains("intent.json") {
            if names.iter().any(|name| name != "before.json") || final_attempt {
                return Err("部门动作链有未完成的本地尝试".into());
            }
            continue;
        }
        let current = intent(backend, run, action, attempt)?;
        if names.contains("api-response.json") {
            model::created_response(
                action,
                target,
                &read(backend, &attempt.join("api-response.json"))?,
            )?;
        }
        if key.is_null() {
            key = current["idempotency_key"].clone();
        }
        if current["idempotency_key"] != key {
            return Err("部门重试未复用首个已登记幂等键".into());
        }
        let states = names
            .iter()
            .filter(|name| name.starts_with("reconcile-"))
            .map(|name| reconciliation(backend, run, action, target, attempt, &attempt.join(name)))
            .collect::<Result<Vec<_>>>()?;
        if names.contains("confirmed.json") {
            if !final_attempt {
                return Err("已确认部门后不得增加后续尝试".into());
            }
            let confirmed = confirmation(backend, run, action, target, attempt)?;
            if confirmed["api_response_proven"] != names.contains("api-response.json") {
                return Err("部门成功响应未被唯一确认消费".into());
            }
        } else if names.contains("api-response.json") {
            return Err("部门成功响应尚未与完整后像确认".into());
        } else if final_attempt
            || states.is_empty()
            || states.iter().any(|state| *state != "before")
        {
            return Err("未确认部门意图未经完整空前像核对".into());
        }
    }
    let final_attempt = attempts.last().ok_or("部门动作链为空")?;
    common::binding(&final_attempt.join("confirmed.json"))
}

fn names(backend: &Path, attempt: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let allowed = Regex::new(r"^(?:observed|reconcile)-[0-9]{4,}\.json$")?;
    for entry in fs::read_dir(attempt)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "部门动作证据名编码无效")?;
        if ![
            "before.json",
            "intent.json",
            "api-response.json",
            "confirmed.json",
        ]
        .contains(&name.as_str())
            && !allowed.is_match(&name)
        {
            return Err("部门动作链含未登记证据".into());
        }
        common::common_file(backend, &entry.path(), "部门动作证据")?;
        names.insert(name);
    }
    Ok(names)
}
