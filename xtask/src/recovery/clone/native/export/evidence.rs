use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use super::super::{digest, json as json_io, lifecycle::manifest};
use super::{input::Context, inventory};
use crate::Result;

// 集合与既有完整导出复核契约一致；不把 SQL 或对象 payload 加入小型证明摘要。
pub(super) fn files(context: &Context, export: &Value, root: &Path) -> Result<Value> {
    frozen_files(&context.backend, context.source(), export, root)
}

pub(super) fn frozen_files(
    backend: &Path,
    source: &Value,
    export: &Value,
    root: &Path,
) -> Result<Value> {
    let mut paths = BTreeSet::new();
    paths.insert(manifest::bound_file(
        backend,
        &export["request"],
        "原导出请求",
        true,
    )?);
    for name in [
        "request",
        "generation-before",
        "generation-after",
        "inventory-before",
        "inventory-after",
        "inventory-before-targets",
        "inventory-after-targets",
        "schema-before",
        "schema-after",
    ] {
        paths.insert(digest::safe_file(root, &format!("{name}.json"))?);
    }
    for phase in ["before", "after"] {
        paths.extend(inventory_files(source, root, phase)?);
    }
    for bucket in json_io::array(&export["objects"], "完整对象桶")? {
        for entry in json_io::array(&bucket["entries"], "完整对象条目")? {
            let capture = digest::safe_file(
                root,
                json_io::text(&entry["capture"]["file"], "对象采集路径")?,
            )?;
            let directory = capture.parent().ok_or("对象采集缺少目录")?;
            for name in capture_names() {
                paths.insert(digest::safe_file(directory, &name)?);
            }
        }
    }
    let mut files = Map::new();
    for path in paths {
        let actual = digest::file(&path)?;
        files.insert(
            manifest::path_text(&path, "原始证明路径")?,
            json!({"bytes":actual.bytes,"sha256":actual.sha256}),
        );
    }
    Ok(Value::Object(files))
}

fn inventory_files(source: &Value, root: &Path, phase: &str) -> Result<Vec<PathBuf>> {
    let saved = json_io::read(&root.join(format!("inventory-{phase}-targets.json")))?;
    json_io::exact(&saved, &["runtime", "targets"], "逐目标库存证明")?;
    let runtime_name = format!("inventory-{phase}-runtime.json");
    let runtime_path = bound_relative(root, &saved["runtime"], &runtime_name)?;
    let mut paths = vec![runtime_path.clone()];
    let mut targets = Vec::new();
    let mut expected = json_io::array(&source["databases"], "来源数据库")?
        .iter()
        .map(|database| {
            Ok(format!(
                "inventory-{phase}-target-{}.json",
                json_io::text(&database["key"], "来源数据库 key")?
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    for descriptor in json_io::array(&saved["targets"], "逐目标库存证明")? {
        let name = json_io::text(&descriptor["file"], "逐目标库存文件")?;
        if !expected.remove(name) {
            return Err("逐目标原始文件未精确覆盖登记目标".into());
        }
        let path = bound_relative(root, descriptor, name)?;
        let target = json_io::read(&path)?;
        if name
            != format!(
                "inventory-{phase}-target-{}.json",
                json_io::text(&target["target"]["database"]["key"], "实际目标 key")?
            )
        {
            return Err("逐目标原始文件名与实际目标不同".into());
        }
        paths.push(path);
        targets.push(target);
    }
    if !expected.is_empty()
        || inventory::complete_selected(&json_io::read(&runtime_path)?, targets, source)?
            != json_io::read(&root.join(format!("inventory-{phase}.json")))?
    {
        return Err("完整库存与已绑定的原始逐目标库存不同".into());
    }
    Ok(paths)
}

pub(crate) fn bound_relative(root: &Path, descriptor: &Value, expected: &str) -> Result<PathBuf> {
    json_io::exact(descriptor, &["file", "bytes", "sha256"], "原始库存绑定")?;
    if descriptor["file"] != expected {
        return Err("原始库存文件名不符".into());
    }
    let path = digest::safe_file(root, expected)?;
    let actual = digest::file(&path)?;
    if descriptor["bytes"].as_u64() != Some(actual.bytes)
        || descriptor["sha256"].as_str() != Some(actual.sha256.as_str())
    {
        return Err("原始库存文件摘要变化".into());
    }
    Ok(path)
}

pub(super) fn capture_names() -> BTreeSet<String> {
    let mut names = BTreeSet::from(["intent.json".into(), "capture.json".into()]);
    for phase in ["head-before", "get", "head-after"] {
        names.insert(format!("{phase}.json"));
        names.insert(format!("{phase}.diagnostic.json"));
    }
    for phase in ["owner-before", "owner-after"] {
        for bucket in super::super::input::BUCKETS {
            for suffix in ["bin", "json", "diagnostic.json"] {
                names.insert(format!("{phase}-{bucket}.{suffix}"));
            }
        }
    }
    names
}
