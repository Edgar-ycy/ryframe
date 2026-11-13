use super::{LAUNCH, ROLES, check, descriptor, text};
use crate::{
    Result,
    recovery::clone::native::{digest, json as strict, resources::common},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{ErrorKind, Read},
    path::{Component, Path},
    time::SystemTime,
};

pub(in crate::recovery::runtime) fn receipt_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !digest::linked(&metadata) => Ok(true),
        Ok(_) => Err("恢复运行状态收据必须是普通文件".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn safe(path: &Path) -> Result<()> {
    check(
        path.is_absolute()
            && !path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir)),
        "恢复运行路径必须为规范绝对路径",
    )?;
    check(
        !digest::linked_path(path)?.contains(&true),
        "恢复运行路径不得经过链接或重解析点",
    )
}
fn state(path: &Path) -> Result<Value> {
    safe(path)?;
    let metadata = fs::symlink_metadata(path)?;
    check(metadata.is_file(), "恢复运行输入必须为普通文件")?;
    Ok(
        json!({"identity":common::device_inode_for(path, &metadata)?,"bytes":metadata.len(),
        "modified":metadata.modified()?.duration_since(SystemTime::UNIX_EPOCH)?.as_nanos().to_string()}),
    )
}
pub(super) fn bound_value(path: &Path) -> Result<Value> {
    let before = state(path)?;
    check(
        before["bytes"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= strict::MAX_JSON_BYTES),
        "恢复运行证据为空或超过16MiB",
    )?;
    let bytes = bounded_bytes(path)?;
    check(
        state(path)? == before && before["bytes"] == json!(bytes.len()),
        "恢复运行证据读取期间被替换",
    )?;
    Ok(json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}))
}

fn bounded_bytes(path: &Path) -> Result<Vec<u8>> {
    crate::recovery::runtime::dataset_authority::reads::file(path)?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(strict::MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    check(
        !bytes.is_empty() && bytes.len() as u64 <= strict::MAX_JSON_BYTES,
        "恢复运行证据为空或超过16MiB",
    )?;
    Ok(bytes)
}
pub(super) fn document(path: &Path) -> Result<Value> {
    let before = state(path)?;
    let binding = bound_value(path)?;
    let bytes = bounded_bytes(path)?;
    check(
        json!(hex::encode(Sha256::digest(&bytes))) == binding["sha256"],
        "恢复运行JSON读取期间发生变化",
    )?;
    let value = strict::parse_fragment(
        std::str::from_utf8(&bytes).map_err(|_| "恢复运行JSON必须为UTF-8")?,
        "恢复运行JSON",
    )
    .map_err(|_| "恢复运行JSON含重复字段、非标准数值或无效语法")?;
    check(
        value.is_object() && state(path)? == before && bound_value(path)? == binding,
        "恢复运行输入类型无效或读取期间被替换",
    )?;
    Ok(value)
}
pub(super) fn bound(path: &Path, expected: &Value) -> Result<()> {
    descriptor(expected)?;
    check(
        bound_value(path)? == *expected,
        "恢复运行文件与冻结描述不一致",
    )
}
pub(super) fn names(directory: &Path) -> Result<BTreeSet<String>> {
    crate::recovery::runtime::dataset_authority::reads::directory(directory)?;
    safe(directory)?;
    fs::read_dir(directory)?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| "恢复运行文件名不是UTF-8".into())
        })
        .collect()
}
pub(in crate::recovery::runtime) fn generation_directory(
    directory: &Path,
    roles: &Value,
    request: &Value,
) -> Result<()> {
    use crate::recovery::runtime::dataset_authority::b0;

    safe(directory)?;
    if !directory.try_exists()? {
        return Ok(());
    }
    check(
        fs::symlink_metadata(directory)?.is_dir(),
        "恢复运行代次目录不是普通目录",
    )?;
    let mut allowed = BTreeSet::from([LAUNCH.to_owned()]);
    let legacy = if crate::recovery::b0::matches(&request["authority"]) {
        [b0::config_directory("api"), b0::config_directory("worker")]
    } else {
        [None, None]
    };
    allowed.extend(legacy.into_iter().flatten().map(str::to_owned));
    for role in ROLES {
        let operation = text(&roles[role]["operation_id"])?;
        allowed.extend([
            format!("{role}.log"),
            format!("{role}.json"),
            format!("{role}-tree.json"),
            format!("{role}-members-{operation}-ready.json"),
            format!("{role}-members-{operation}-stopped.json"),
            format!("{role}-tree-{operation}-control.json"),
            format!("{role}-tree-{operation}-result.json"),
        ]);
    }
    let actual = names(directory)?;
    check(
        actual.is_subset(&allowed),
        "恢复运行进程树目录包含未登记文件",
    )?;
    for name in actual {
        let path = directory.join(name);
        safe(&path)?;
        if legacy
            .into_iter()
            .flatten()
            .any(|item| path.file_name().is_some_and(|name| name == item))
        {
            check(
                fs::symlink_metadata(&path)?.is_dir(),
                "B0 配置快照必须是普通目录",
            )?;
            let entries = names(&path)?;
            check(
                entries
                    .iter()
                    .all(|name| matches!(name.as_str(), "app.toml" | "app.test.toml")),
                "B0 配置快照包含未登记文件",
            )?;
            for entry in entries {
                let file = path.join(entry);
                safe(&file)?;
                check(
                    fs::symlink_metadata(&file)?.is_file(),
                    "B0 配置快照包含非普通文件",
                )?;
            }
            continue;
        }
        check(
            fs::symlink_metadata(path)?.is_file(),
            "恢复运行进程树目录包含非普通文件",
        )?;
    }
    Ok(())
}
pub(super) fn write_scope(root: &Path, request: &Value) -> Result<()> {
    safe(root)?;
    let coordinator = super::path(&request["tools"]["coordinator_root"])?;
    let local = coordinator.join(".local-tests");
    safe(&local)?;
    check(
        fs::symlink_metadata(&local)?.is_dir() && root.starts_with(&local) && root != local,
        "恢复运行写入必须位于已绑定协调器忽略目录的明确子目录",
    )
}
