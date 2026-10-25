//! 用续作 intent 的精确前像分割完整历史；文件只归属一个尝试，不能用时间或文件名猜边界。
use super::{
    super::{json as document, resources::common},
    evidence_model::{check, ordered},
};
use crate::Result;
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

pub(super) struct Group {
    pub(super) files: BTreeSet<String>,
    pub(super) complete: bool,
    pub(super) published: bool,
    pub(super) confirmed: bool,
}

pub(super) fn partition(
    backend: &Path,
    target: &Path,
    names: &BTreeSet<String>,
    records: &Value,
) -> Result<Vec<Group>> {
    let mut intents = read_intents(backend, target, names, records)?;
    intents.sort_by_key(|(_, v)| {
        v["files_before"]
            .as_object()
            .map_or(0, serde_json::Map::len)
    });
    let mut groups = Vec::new();
    let mut known = BTreeSet::new();
    let mut start = BTreeSet::new();
    let mut prior = None;
    for (name, value) in &intents {
        let before = document::object(&value["files_before"], "prepare 前像")?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        check(
            !before.contains(name) && known.is_subset(&before),
            "prepare intents 前像没有唯一追加顺序",
        )?;
        let delta = before.difference(&start).cloned().collect();
        groups.push(group(delta, prior, records)?);
        known = before;
        check(known.insert(name.clone()), "prepare intent 重复归属")?;
        start = known.clone();
        prior = value["attempt"].as_str();
    }
    let delta = names.difference(&start).cloned().collect();
    groups.push(group(delta, prior, records)?);
    let union = groups
        .iter()
        .flat_map(|v| v.files.iter().cloned())
        .collect::<BTreeSet<_>>();
    let metadata = names
        .iter()
        .filter(|name| metadata(name))
        .cloned()
        .collect::<BTreeSet<_>>();
    check(
        union.is_disjoint(&metadata)
            && union.union(&metadata).cloned().collect::<BTreeSet<_>>() == *names,
        "prepare 文件未被唯一消耗或包含未绑定增量",
    )?;
    publication_order(target, &groups, &intents, records)?;
    Ok(groups)
}

fn read_intents(
    backend: &Path,
    target: &Path,
    names: &BTreeSet<String>,
    records: &Value,
) -> Result<Vec<(String, Value)>> {
    let mut intents = Vec::new();
    for descriptor in document::object(&records["intents"], "prepare intents")?.values() {
        let path = common::bound_file(backend, descriptor, "prepare intent")?;
        let value = document::read(&path)?;
        let before = document::object(&value["files_before"], "prepare 精确前像")?;
        for (name, binding) in before {
            check(
                names.contains(name) && common::binding(&target.join(name))? == *binding,
                "prepare 历史前像缺失或漂移",
            )?;
        }
        intents.push((
            path.file_name()
                .and_then(|v| v.to_str())
                .ok_or("prepare intent 名无效")?
                .to_owned(),
            value,
        ));
    }
    Ok(intents)
}

fn group(mut files: BTreeSet<String>, attempt: Option<&str>, records: &Value) -> Result<Group> {
    let terminal = attempt.and_then(|id| records["terminals"].get(id));
    let published = files.contains("prepare.json");
    let confirmed = terminal.is_some_and(|v| v[0] == "confirmed");
    let complete = published || confirmed;
    if let Some(id) = attempt {
        let intent = format!("resume-prepare-{id}.intent.json");
        check(
            !files.contains(&intent),
            "prepare 自身 intent 不属于观察增量",
        )?;
        if let Some(terminal) = terminal {
            let name = Path::new(document::text(&terminal[2]["path"], "prepare 终态")?)
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or("prepare 终态名无效")?;
            check(files.remove(name), "prepare 终态没有归入唯一尝试")?;
        }
    }
    files.retain(|name| !metadata(name));
    Ok(Group {
        files,
        complete,
        published,
        confirmed,
    })
}

fn metadata(name: &str) -> bool {
    if matches!(name, "request.json" | "prepare.json" | "failure.json") {
        return true;
    }
    let Some(rest) = name.strip_prefix("resume-prepare-") else {
        return false;
    };
    let Some((id, kind)) = rest.split_once('.') else {
        return false;
    };
    id.len() == 32
        && id
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
        && matches!(kind, "intent.json" | "confirmed.json" | "failure.json")
}

fn publication_order(
    target: &Path,
    groups: &[Group],
    intents: &[(String, Value)],
    records: &Value,
) -> Result<()> {
    let prepared = target.join("prepare.json");
    if !prepared.try_exists()? {
        return Ok(());
    }
    let value = document::read(&prepared)?;
    for (index, group) in groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.published)
    {
        let start = index
            .checked_sub(1)
            .and_then(|i| intents.get(i))
            .map(|(_, v)| v);
        if let Some(start) = start {
            check(
                ordered(&[&start["at"], &value["prepared_at"]]),
                "prepare 首次发布早于所属续作 intent",
            )?;
        }
        let terminal = start
            .and_then(|v| v["attempt"].as_str())
            .and_then(|id| records["terminals"].get(id));
        let end = terminal
            .map(|v| &v[1]["at"])
            .or_else(|| intents.get(index).map(|(_, v)| &v["at"]));
        if let Some(end) = end {
            check(
                ordered(&[&value["prepared_at"], end]),
                "prepare 首次发布晚于所属尝试边界",
            )?;
        }
        check(!group.files.is_empty(), "prepare 首次发布缺少完整观察")?;
    }
    Ok(())
}
