//! 续作阶段的精确文件增量；既有文件不可变，新增文件必须由当前阶段唯一消费。
use super::super::super::super::{
    export::transport::Transport, json as document, resources::common,
};
use super::super::super::{
    fresh::selection::Selected, prepare_observations, resume_observations as observations, tree,
};
use super::window::Window;
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) fn unchanged<T: Transport>(w: &Window<'_, T>, before: &Value) -> Result<()> {
    w.checkpoint()?;
    if tree::snapshot(w.inputs.backend, w.inputs.observation_dir, true)? != *before {
        return Err("续作锁内已声明的完整树变化".into());
    }
    w.checkpoint()
}
fn delta<T: Transport>(w: &Window<'_, T>, before: &Value) -> Result<(Value, BTreeSet<String>)> {
    w.checkpoint()?;
    let current = tree::snapshot(w.inputs.backend, w.inputs.observation_dir, true)?;
    let old = observations::file_map(before)?;
    let new = observations::file_map(&current)?;
    if old.iter().any(|(k, v)| new.get(k) != Some(v)) {
        return Err("续作既有证据缺失或漂移".into());
    }
    let names = new
        .keys()
        .filter(|k| !old.contains_key(*k))
        .cloned()
        .collect();
    Ok((current, names))
}
pub(super) fn added<T: Transport>(
    w: &Window<'_, T>,
    before: &Value,
    expected: &BTreeSet<String>,
) -> Result<Value> {
    let (current, names) = delta(w, before)?;
    if names != *expected {
        return Err("续作阶段含有未声明或缺失文件，禁止吸收未知增量".into());
    }
    Ok(current)
}
pub(super) fn one<T: Transport>(
    w: &Window<'_, T>,
    before: &Value,
    path: &Path,
    value: Option<&Value>,
) -> Result<Value> {
    if path.parent() != Some(w.inputs.observation_dir) {
        return Err("续作收据不属于固定目标目录".into());
    }
    if let Some(value) = value
        && document::read(path)? != *value
    {
        return Err("续作收据发布后变化".into());
    }
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("续作收据文件名无效")?;
    added(w, before, &BTreeSet::from([name.into()]))
}
pub(super) fn runtime<T: Transport>(
    w: &Window<'_, T>,
    before: &Value,
    candidate: &Selected,
) -> Result<Value> {
    let (current, names) = delta(w, before)?;
    let mut progress = 0;
    prepare_observations::validate(
        w.inputs.backend,
        w.inputs.observation_dir,
        &names,
        w.inputs.request,
        candidate,
        false,
        &mut progress,
    )?;
    let redis = &candidate.value["redis"];
    if redis["platform"] == "windows" {
        if progress != 2 {
            return Err("续作实际存储观察缺少完整 layout 与 Windows Redis 投影".into());
        }
        let baseline = observations::windows_redis_baseline(
            w.inputs.backend,
            w.inputs.observation_dir,
            &active_prepare(w)?,
        )?;
        prepare_observations::windows_redis_transition(
            &w.inputs.request["storage"]["redis"],
            &baseline,
        )?;
        prepare_observations::windows_redis_transition(&baseline, redis)?;
    } else {
        if progress != 6 {
            return Err("续作实际存储观察不是layout及五项内核完整轮次".into());
        }
        same_redis_name(w, &names)?;
    }
    Ok(current)
}
pub(super) fn objects<T: Transport>(
    w: &Window<'_, T>,
    before: &Value,
    objects: &Value,
) -> Result<Value> {
    let (current, mut names) = delta(w, before)?;
    consume_objects(w, &mut names, objects)?;
    if !names.is_empty() {
        return Err("续作对象观察含有未知文件".into());
    }
    Ok(current)
}
fn consume_objects<T: Transport>(
    w: &Window<'_, T>,
    names: &mut BTreeSet<String>,
    objects: &Value,
) -> Result<()> {
    let baseline = active_prepare(w)?;
    for stage in ["bucket-readiness", "bucket-versioning", "scoped-list"] {
        let values = observations::objects(
            w.inputs.backend,
            w.inputs.observation_dir,
            names,
            w.inputs.request,
            stage,
        )?;
        let buckets = values
            .iter()
            .filter_map(|v| v["bucket"].as_str())
            .collect::<BTreeSet<_>>();
        if values.len() != 5 || buckets.len() != 5 {
            return Err("续作对象观察未唯一完整覆盖五桶".into());
        }
        let original = observations::objects(
            w.inputs.backend,
            w.inputs.observation_dir,
            &baseline,
            w.inputs.request,
            stage,
        )?;
        for value in &values {
            let before = original
                .iter()
                .find(|v| v["bucket"] == value["bucket"])
                .ok_or("对象桶缺少准备前像")?;
            if stage != "scoped-list" && value["response"] != before["response"] {
                return Err("对象端点或版本观察不同于准备前像".into());
            }
            if stage == "scoped-list" {
                document::exact(
                    &value["response"],
                    &["IsTruncated", "Contents"],
                    "对象完整分页",
                )?;
            }
            match stage {
                "bucket-versioning" if !value["response"]["Status"].is_null() => {
                    return Err("对象版本状态变化".into());
                }
                "scoped-list" => {
                    let bucket = document::text(&value["bucket"], "对象桶")?;
                    let keys = value["response"]
                        .get("Contents")
                        .and_then(Value::as_array)
                        .ok_or("对象清单不完整")?
                        .iter()
                        .map(|v| v["Key"].clone())
                        .collect::<Vec<_>>();
                    if value["response"]["IsTruncated"] != false
                        || json!(keys) != objects[bucket]["keys"]
                    {
                        return Err("对象完整清单不同于实际owner集合".into());
                    }
                }
                _ => {}
            }
        }
        for name in observations::stage(names, stage)? {
            names.remove(&name);
        }
    }
    observations::owner_reads(
        w.inputs.backend,
        w.inputs.observation_dir,
        names,
        w.inputs.request,
        objects,
    )
}
pub(super) fn preimage<T: Transport>(
    w: &Window<'_, T>,
    before: &Value,
    objects: &Value,
    started: &Path,
    value: &Value,
) -> Result<Value> {
    let (current, mut names) = delta(w, before)?;
    let name = started
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("started文件名无效")?;
    if started.parent() != Some(w.inputs.observation_dir)
        || !names.remove(name)
        || document::read(started)? != *value
    {
        return Err("迁移started前像不属于当前唯一增量".into());
    }
    consume_objects(w, &mut names, objects)?;
    for db in document::array(&w.inputs.request["target"]["databases"], "四目标")? {
        let stage = format!("mysql-{}", document::text(&db["key"], "目标key")?);
        let entries = observations::stage(&names, &stage)?;
        if entries.len() != 1 {
            return Err("续作资源前像没有精确四库观察".into());
        }
        let value = observations::receipt(w.inputs.backend, w.inputs.observation_dir, &entries[0])?;
        let candidate = Selected {
            value: w.inputs.request["storage"].clone(),
            runtime: None,
        };
        let mut expected =
            prepare_observations::slots(w.inputs.backend, w.inputs.request, &candidate)?
                .into_iter()
                .find(|slot| slot.name == stage)
                .ok_or("固定DB观察槽缺失")?
                .expected;
        expected["stdout"] = json!(format!(
            "{}\n{}\n",
            document::text(&db["server_uuid"], "UUID")?,
            document::text(&db["database"], "物理库")?
        ));
        prepare_observations::matches_slot(&value, &expected, true)?;
        names.remove(&entries[0]);
    }
    if !names.is_empty() {
        return Err("续作资源前像含有未知文件".into());
    }
    Ok(current)
}
pub(super) fn inventory<T: Transport>(
    w: &Window<'_, T>,
    name: &str,
    result: &Value,
    evidence: &[Value],
) -> Result<BTreeSet<String>> {
    let directory = w.inputs.observation_dir.join(name);
    if result["receipt"] != common::binding(&directory.join("inventory.json"))? {
        return Err("初始化库存未返回精确发布绑定".into());
    }
    let mut files = BTreeSet::from([name.to_owned()]);
    let mut names = BTreeSet::new();
    for value in evidence {
        document::exact(value, &["path", "type", "bytes", "sha256"], "库存文件描述")?;
        let file = document::text(&value["path"], "库存文件名")?;
        if Path::new(file).file_name().and_then(|v| v.to_str()) != Some(file)
            || value["type"] != "file"
            || !names.insert(file)
        {
            return Err("初始化库存证据路径重复或越界".into());
        }
        let actual = common::binding(&directory.join(file))?;
        if value["bytes"] != actual["bytes"] || value["sha256"] != actual["sha256"] {
            return Err("初始化库存证据变化".into());
        }
        files.insert(format!("{name}/{file}"));
    }
    if !names.contains("inventory.json") {
        return Err("初始化库存缺少成功收据".into());
    }
    Ok(files)
}

fn active_prepare<T: Transport>(w: &Window<'_, T>) -> Result<BTreeSet<String>> {
    let snapshot = document::read(&w.registration.workspace.join("prepared-files.json"))?;
    let baseline = observations::file_map(&snapshot["files"])?
        .into_keys()
        .collect();
    let records = super::super::super::evidence_prepare::records(
        w.inputs.backend,
        w.inputs.observation_dir,
        &w.registration.value["request"],
    )?;
    let groups = super::super::super::prepare_groups::partition(
        w.inputs.backend,
        w.inputs.observation_dir,
        &baseline,
        &records,
    )?;
    Ok(groups
        .into_iter()
        .last()
        .filter(|v| v.complete)
        .ok_or("缺少最后完整准备观察组")?
        .files)
}
fn same_redis_name<T: Transport>(w: &Window<'_, T>, current: &BTreeSet<String>) -> Result<()> {
    let head = |names: &BTreeSet<String>| -> Result<String> {
        let values = observations::stage(names, "redis-kernel")?
            .into_iter()
            .map(|name| observations::receipt(w.inputs.backend, w.inputs.observation_dir, &name))
            .collect::<Result<Vec<_>>>()?;
        let value = values
            .iter()
            .find(|v| {
                v["command"]
                    .as_array()
                    .and_then(|v| v.last())
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.ends_with("/stat"))
            })
            .ok_or("Redis观察缺少实际进程stat")?;
        let (head, _) =
            observations::process_identity(document::text(&value["stdout"], "Redis stat")?)?;
        Ok(head
            .split_once(' ')
            .ok_or("Redis stat缺少进程名")?
            .1
            .to_owned())
    };
    if head(&active_prepare(w)?)? != head(current)? {
        return Err("Redis进程名不同于准备前像".into());
    }
    Ok(())
}
