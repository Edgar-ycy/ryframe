//! 完整库存历史的纯只读核验；续作资格仍由外层控制器决定。
use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Value, json};

use super::super::{guard, tree};
use crate::{
    Result,
    recovery::clone::native::{digest, json as json_io, resources::common},
};

const KEYS: [&str; 4] = ["shared-control", "shared", "dedicated-a", "dedicated-b"];
const DIAGNOSTICS: &[&str] = &[
    "库存历史目录不属于本初始化代次的精确目录",
    "库存历史仍被初始化锁占用",
    "库存历史存在未解析失败或非普通失败证据",
    "库存历史未完整采集本侧四个目标",
    "库存历史必须保留完整八份原始输出",
    "库存历史原始文件或摘要变化",
    "库存历史来源前后绑定变化",
    "库存历史请求必须包含四个不同目标数据库",
    "库存历史目标物理身份不同",
    "库存历史与原实际采集收据不同",
    "库存历史核验期间文件树或目录身份变化",
];

pub(super) fn diagnostic(reason: &str) -> Option<&'static str> {
    DIAGNOSTICS
        .iter()
        .copied()
        .find(|message| *message == reason)
}

pub(super) fn verify(backend: &Path, target: &Path, value: &Value) -> Result<Value> {
    verify_with(backend, target, value, || {})
}

fn verify_with(
    backend: &Path,
    target: &Path,
    value: &Value,
    observe: impl FnOnce(),
) -> Result<Value> {
    let resumed = json_io::boolean(&value["resumed"], "库存续作标志")?;
    let selected = if value["inventory_directory"].is_null() {
        target.join("inventory-initial")
    } else {
        json_io::text(&value["inventory_directory"], "库存显式目录")?.into()
    };
    // 显式 reference/images 库存目录允许位于后端 .local-tests 的其他已登记位置。
    if !value["inventory_directory"].is_null() {
        common::local_directory(backend, &selected, "库存显式目录")?;
    }
    let initial = &value["initial"];
    let binding = &initial["receipt"];
    let filename = common::bound_file(backend, binding, "库存历史收据")?;
    let parent = filename.parent().ok_or("库存历史收据缺少父目录")?;
    let resume_name = parent
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|name| name.starts_with("inventory-resume-"));
    let resume_directory = parent.parent() == Some(target) && resume_name;
    if filename.file_name().and_then(|v| v.to_str()) != Some("inventory.json")
        || (resume_name && !(resumed && resume_directory))
        || (parent != selected && !(resumed && resume_directory))
    {
        return Err(DIAGNOSTICS[0].into());
    }
    unfailed(parent, resumed)?;
    let identity = directory_identity(parent)?;
    let target_identity = directory_identity(target)?;
    let before = tree::snapshot(backend, parent, false)?;
    let paths = json_io::array(&before, "库存文件树")?
        .iter()
        .filter(|entry| entry["type"] == "file")
        .map(|entry| Ok(parent.join(json_io::text(&entry["path"], "库存文件路径")?)))
        .collect::<Result<Vec<_>>>()?;
    let protected = guard::Inputs::open(paths)?;
    let receipt = json_io::read(&filename)?;
    check_receipt(backend, parent, &receipt, &value["request"], initial)?;
    observe();
    // stdin 的 initial/request 为不可变借用；所有实际读取文件和绑定在返回前重新核验。
    common::bound_file(backend, binding, "库存历史收据")?;
    protected.check()?;
    unfailed(parent, resumed)?;
    if directory_identity(parent)? != identity
        || directory_identity(target)? != target_identity
        || tree::snapshot(backend, parent, false)? != before
    {
        return Err(DIAGNOSTICS[10].into());
    }
    Ok(json!({"status":"inventory_history_verified","remote_writes":0}))
}

fn unfailed(directory: &Path, resumed: bool) -> Result<()> {
    match fs::symlink_metadata(directory.join("initialize.lock")) {
        Ok(_) => return Err(DIAGNOSTICS[1].into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match fs::symlink_metadata(directory.join("failure.json")) {
        Ok(metadata) if digest::linked(&metadata) || !metadata.is_file() || !resumed => {
            Err(DIAGNOSTICS[2].into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || digest::linked(&metadata) {
        return Err(DIAGNOSTICS[0].into());
    }
    common::device_inode_for(path, &metadata)
}

fn check_receipt(
    backend: &Path,
    directory: &Path,
    receipt: &Value,
    request: &Value,
    initial: &Value,
) -> Result<()> {
    let target = &request["target"];
    let scope = json_io::text(&target["scope_id"], "库存目标 scope")?;
    let observations = json_io::exact(&receipt["observations"], &KEYS, "库存四目标观察")?;
    if receipt["format_version"].as_u64() != Some(1)
        || receipt["status"] != "side_inventory_captured"
        || receipt["side"] != "target"
        || receipt["scope_id"] != scope
        || receipt["keys"] != json!(KEYS)
    {
        return Err(DIAGNOSTICS[3].into());
    }
    raw_outputs(backend, directory, receipt)?;
    for name in ["binding-before.json", "binding-after.json"] {
        let path = common::common_file(backend, &directory.join(name), "库存来源绑定")?;
        if receipt["binding_sha256"] != digest::plan_hash(&json_io::read(&path)?)? {
            return Err(DIAGNOSTICS[6].into());
        }
    }
    let databases = json_io::array(&target["databases"], "库存请求数据库")?;
    let mut images = BTreeMap::new();
    let mut keys = std::collections::BTreeSet::new();
    for database in databases {
        let key = json_io::text(&database["key"], "库存数据库 key")?;
        let name = json_io::text(&database["database"], "库存物理数据库")?;
        if !KEYS.contains(&key) || !keys.insert(key) || images.contains_key(name) {
            return Err(DIAGNOSTICS[7].into());
        }
        let image = &observations[key];
        let server = json_io::text(&database["server_uuid"], "库存服务器身份")?;
        if image["resource"]
            != json!({"kind":"database","scope_id":scope,
            "server_uuid":server,"database":name})
        {
            return Err(DIAGNOSTICS[8].into());
        }
        images.insert(name, image);
    }
    if keys.len() != KEYS.len() {
        return Err(DIAGNOSTICS[7].into());
    }
    if initial["observations"] != serde_json::to_value(images)? {
        return Err(DIAGNOSTICS[9].into());
    }
    Ok(())
}

fn raw_outputs(backend: &Path, directory: &Path, receipt: &Value) -> Result<()> {
    let names = ["before", "after"]
        .into_iter()
        .flat_map(|phase| KEYS.map(|key| format!("{phase}-target-{key}.json")))
        .collect::<Vec<_>>();
    let expected = names.iter().map(String::as_str).collect::<Vec<_>>();
    let files = json_io::exact(&receipt["inventories"], &expected, "库存八份原始输出")
        .map_err(|_| DIAGNOSTICS[4])?;
    for name in names {
        let path = common::common_file(backend, &directory.join(&name), "库存原始输出")?;
        let actual = digest::file(&path)?;
        if files[&name] != json!({"bytes":actual.bytes,"sha256":actual.sha256}) {
            return Err(DIAGNOSTICS[5].into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/target_verify/inventory_history/tests.rs"]
mod tests;
