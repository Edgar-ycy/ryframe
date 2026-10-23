//! prepare 续作必须在初始化之前闭合，且逐项保留历史前像。
use super::{super::json as json_io, evidence_model::*};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) const FAILURE: &[&str] = &[
    "status",
    "stage",
    "at",
    "error_type",
    "reason",
    "automatic_retry",
    "automatic_resource_cleanup",
    "fresh_target_initialized",
    "clone_verified",
    "restore_qualified",
];

pub(crate) fn artifact(name: &str) -> Result<bool> {
    Ok(
        ["request.json", "prepare.json", "failure.json"].contains(&name)
            || Regex::new(r"^[a-z0-9-]+-[a-f0-9]{32}\.command\.json$")?.is_match(name)
            || Regex::new(r"^storage-layout-[a-f0-9]{32}\.json$")?.is_match(name)
            || Regex::new(r"^resume-prepare-[a-f0-9]{32}\.(intent|confirmed|failure)\.json$")?
                .is_match(name),
    )
}

#[derive(Default)]
struct Records {
    intents: BTreeMap<String, Value>,
    values: BTreeMap<String, Value>,
    terminals: BTreeMap<String, Value>,
}

pub(crate) fn records(backend: &Path, target: &Path, request: &Value) -> Result<Value> {
    let records = scan(backend, target, request)?;
    let Records {
        intents,
        values,
        mut terminals,
    } = records;
    check(
        terminals
            .keys()
            .all(|attempt| intents.contains_key(attempt)),
        "fresh 目标 prepare 续作结果缺少原 intent",
    )?;
    causality(backend, target, &intents, &values, &terminals)?;
    // 私有原始失败诊断只留在绑定文件中；跨进程响应不复制 reason/error_type。
    for terminal in terminals.values_mut() {
        if terminal[0] == "failure" {
            let fields = terminal[1].as_object_mut().ok_or("prepare 失败结果无效")?;
            fields.remove("reason");
            fields.remove("error_type");
        } else {
            check(
                terminal[1]["prepare"] == binding(backend, &target.join("prepare.json"))?
                    && terminal[1]["request"] == binding(backend, &target.join("request.json"))?,
                "prepare 续作确认不属于当前固定结果",
            )?;
        }
    }
    let pending = intents
        .keys()
        .filter(|attempt| !terminals.contains_key(*attempt))
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({"intents":intents,"terminals":terminals,"pending":pending}))
}

fn scan(backend: &Path, target: &Path, request: &Value) -> Result<Records> {
    let mut records = Records::default();
    let Records {
        intents,
        values,
        terminals,
    } = &mut records;
    let pattern = Regex::new(r"^resume-prepare-([a-f0-9]{32})\.(intent|confirmed|failure)\.json$")?;
    for name in names(target, r"^resume-prepare-.*\.json$")? {
        let capture = pattern
            .captures(&name)
            .ok_or("fresh 目标包含无法归属的 prepare 续作记录")?;
        let (attempt, kind) = (&capture[1], &capture[2]);
        let path = target.join(&name);
        let value = read(backend, &path)?;
        timestamp(&value["at"]).map_err(|_| {
            if kind == "intent" {
                "fresh 目标 prepare 续作 intent 无效"
            } else {
                "fresh 目标 prepare 续作结果无效"
            }
        })?;
        if kind == "intent" {
            json_io::exact(
                &value,
                &[
                    "format_version",
                    "kind",
                    "attempt",
                    "at",
                    "request",
                    "files_before",
                ],
                "prepare 续作 intent",
            )?;
            check(
                value["format_version"] == 1
                    && value["kind"] == "devex-clone-prepare-resume-intent"
                    && value["attempt"] == attempt
                    && value["request"] == *request,
                "fresh 目标 prepare 续作 intent 无效",
            )?;
            for (name, descriptor) in json_io::object(&value["files_before"], "prepare 前像")? {
                check(
                    Path::new(name).file_name().and_then(|name| name.to_str())
                        == Some(name.as_str())
                        && binding(backend, &target.join(name))? == *descriptor,
                    "fresh 目标 prepare 续作前像变化",
                )?;
            }
            intents.insert(attempt.to_owned(), binding(backend, &path)?);
            values.insert(attempt.to_owned(), value);
        } else {
            let mut fields = vec![
                "format_version",
                "kind",
                "attempt",
                "at",
                "intent",
                "remote_writes",
            ];
            fields.extend(if kind == "confirmed" {
                &["prepare", "request"][..]
            } else {
                &["error_type", "reason"][..]
            });
            json_io::exact(&value, &fields, "prepare 续作结果")?;
            check(
                value["format_version"] == 1
                    && value["kind"] == format!("devex-clone-prepare-resume-{kind}")
                    && value["attempt"] == attempt
                    && value["remote_writes"] == 0,
                "fresh 目标 prepare 续作结果无效",
            )?;
            check(
                !terminals.contains_key(attempt),
                "fresh 目标同一 prepare 续作同时存在多个结果",
            )?;
            terminals.insert(
                attempt.to_owned(),
                json!([kind, value, binding(backend, &path)?]),
            );
        }
    }
    Ok(records)
}

fn causality(
    backend: &Path,
    target: &Path,
    intents: &BTreeMap<String, Value>,
    values: &BTreeMap<String, Value>,
    terminals: &BTreeMap<String, Value>,
) -> Result<()> {
    let failure = if !intents.is_empty() && target.join("failure.json").exists() {
        let value = read(backend, &target.join("failure.json"))?;
        json_io::exact(&value, FAILURE, "prepare 既有失败")?;
        timestamp(&value["at"])?;
        Some(value)
    } else {
        None
    };
    let started = if !intents.is_empty() && target.join("initialize.started.json").exists() {
        let value = read(backend, &target.join("initialize.started.json"))?;
        json_io::exact(&value, &["at", "generation_sha256"], "初始化开始")?;
        timestamp(&value["at"])?;
        Some(value)
    } else {
        None
    };
    for (attempt, terminal) in terminals {
        check(
            terminal[1]["intent"] == intents[attempt]
                && ordered(&[&values[attempt]["at"], &terminal[1]["at"]]),
            "fresh 目标 prepare 续作结果不属于原 intent",
        )?;
        if let Some(started) = &started {
            check(
                ordered(&[&terminal[1]["at"], &started["at"]]),
                "fresh 目标 prepare 续作结果晚于初始化开始",
            )?;
        }
    }
    for value in values.values() {
        check(
            failure
                .as_ref()
                .is_none_or(|failure| ordered(&[&failure["at"], &value["at"]]))
                && started
                    .as_ref()
                    .is_none_or(|started| ordered(&[&value["at"], &started["at"]])),
            "fresh 目标 prepare 续作时间不属于初始化前因果链",
        )?;
    }
    Ok(())
}

pub(crate) fn stages(backend: &Path, target: &Path, request: &Value) -> Result<BTreeSet<String>> {
    let records = records(backend, target, request)?;
    check(
        json_io::array(&records["pending"], "prepare 待结束续作")?.is_empty(),
        "fresh 初始化前存在未收尾的 prepare 续作",
    )?;
    for descriptor in json_io::object(&records["intents"], "prepare intents")?.values() {
        let value = read(backend, &bound(backend, descriptor)?)?;
        for name in json_io::object(&value["files_before"], "prepare 前像")?.keys() {
            check(artifact(name)?, "prepare 续作前像包含初始化写入证据")?;
        }
    }
    let prepared = binding(backend, &target.join("prepare.json"))?;
    let requested = binding(backend, &target.join("request.json"))?;
    for terminal in json_io::object(&records["terminals"], "prepare 结果")?.values() {
        check(
            terminal[0] != "confirmed"
                || terminal[1]["prepare"] == prepared && terminal[1]["request"] == requested,
            "prepare 续作确认不属于当前固定结果",
        )?;
    }
    names(target, r"^resume-prepare-.*\.(intent|confirmed)\.json$")
}

pub(crate) fn failure_path(backend: &Path, target: &Path, request: &Value) -> Result<Value> {
    let root = target.join("failure.json");
    let secondary = names(target, r"^failure-.*\.json$")?;
    check(
        root.is_file()
            && secondary.len() <= 1
            && secondary.iter().all(|name| {
                Regex::new(r"^failure-[a-f0-9]{32}\.json$")
                    .is_ok_and(|pattern| pattern.is_match(name))
            }),
        "fresh 目标没有唯一可归属的初始化失败",
    )?;
    if let Some(name) = secondary.first() {
        let failed = read(backend, &root)?;
        json_io::exact(&failed, FAILURE, "prepare 失败")?;
        timestamp(&failed["at"])?;
        check(
            failed["status"] == "needs_reconciliation"
                && failed["stage"].as_str().is_some_and(|stage| {
                    ["request", "prepare_binding", "prepare_absence"].contains(&stage)
                })
                && ["error_type", "reason"].iter().all(|field| {
                    failed[*field]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                })
                && [
                    "automatic_retry",
                    "automatic_resource_cleanup",
                    "fresh_target_initialized",
                    "clone_verified",
                    "restore_qualified",
                ]
                .iter()
                .all(|field| failed[*field] == false),
            "fresh 目标 failure 不是可续作的只读 prepare 失败",
        )?;
        let records = records(backend, target, request)?;
        let prepared = binding(backend, &target.join("prepare.json"))?;
        let requested = binding(backend, &target.join("request.json"))?;
        check(
            json_io::array(&records["pending"], "prepare 待结束续作")?.is_empty()
                && json_io::object(&records["terminals"], "prepare 结果")?
                    .values()
                    .any(|item| {
                        item[0] == "confirmed"
                            && item[1]["prepare"] == prepared
                            && item[1]["request"] == requested
                    }),
            "初始化失败之前的 prepare 失败尚未显式收尾",
        )?;
        return binding(backend, &target.join(name));
    }
    binding(backend, &root)
}
