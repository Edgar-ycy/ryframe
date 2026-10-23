use super::super::{configuration, documents};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) struct Snapshot {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl Snapshot {
    pub fn read(root: &Path, path: &Path, maximum: usize, label: &str) -> Result<(Self, Value)> {
        validate_local_test_path(path, root, LocalTestPathKind::ExistingFile)?;
        let before = fs::metadata(path)?;
        if before.len() > maximum as u64 {
            return Err(format!("{label}超过固定上限").into());
        }
        let bytes = fs::read(path)?;
        let after = fs::metadata(path)?;
        if bytes.len() as u64 != before.len()
            || after.len() != before.len()
            || after.modified()? != before.modified()?
        {
            return Err(format!("{label}在读取期间发生变化").into());
        }
        let value = documents::parse_fragment(std::str::from_utf8(&bytes)?, label)?;
        Ok((
            Self {
                path: path.into(),
                bytes,
            },
            value,
        ))
    }
    pub fn unchanged(&self, root: &Path) -> Result<()> {
        validate_local_test_path(&self.path, root, LocalTestPathKind::ExistingFile)?;
        if fs::read(&self.path)? != self.bytes {
            return Err("已有数据输入在业务验证期间发生变化".into());
        }
        Ok(())
    }
    pub fn sha256(&self) -> String {
        hex::encode(Sha256::digest(&self.bytes))
    }
}

pub(super) fn arguments(
    args: &[OsString],
    required: &[&str],
    flag: &str,
) -> Result<BTreeMap<String, String>> {
    let allowed = required.iter().copied().collect::<BTreeSet<_>>();
    let mut result = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let name = args[index]
            .to_str()
            .filter(|value| allowed.contains(*value) || *value == flag)
            .ok_or("已有数据参数未知或不是UTF-8")?;
        if result.contains_key(name) {
            return Err("已有数据参数重复".into());
        }
        if name == flag {
            result.insert(name.into(), "true".into());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or("已有数据参数缺少值或不是UTF-8")?;
        result.insert(name.into(), value.into());
        index += 2;
    }
    if required.iter().any(|name| !result.contains_key(*name))
        || result.get(flag).map(String::as_str) != Some("true")
        || result.len() != required.len() + 1
    {
        return Err("已有数据私有入口缺少完整显式参数与 --write".into());
    }
    Ok(result)
}

pub(super) fn path(arguments: &BTreeMap<String, String>, name: &str) -> Result<PathBuf> {
    let value = arguments.get(name).ok_or("已有数据路径参数缺失")?;
    if value.chars().any(char::is_control) {
        return Err("已有数据路径包含控制字符".into());
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("已有数据路径必须为绝对路径".into());
    }
    Ok(path)
}

pub(super) fn password(
    protocol_root: &Path,
    configuration_root: &Path,
    binding_path: &Path,
    dataset: &Value,
    snapshots: &mut Vec<Snapshot>,
    configuration_snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<String> {
    let (snapshot, binding) =
        Snapshot::read(protocol_root, binding_path, 1024 * 1024, "应用配置绑定")?;
    crate::recovery::clone::native::s3::app_configuration::load(
        configuration_root,
        &binding,
        configuration_snapshots,
    )?;
    let password = configuration::current(configuration_root, dataset, configuration_snapshots)?;
    snapshots.push(snapshot);
    Ok(password)
}

pub(super) fn catalog(value: &Value) -> Result<BTreeMap<String, Value>> {
    crate::devex::runtime_configuration::operation_catalog(value).map_err(Into::into)
}

pub(super) fn wait_start(expected: &Value) -> Result<()> {
    read_start(std::io::stdin(), expected)
}

pub(super) fn read_start(input: impl Read, expected: &Value) -> Result<()> {
    let mut bytes = Vec::new();
    input.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err("已有数据启动授权超过4KiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "已有数据启动授权必须为UTF-8")?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.is_empty() || line.contains(['\r', '\n']) {
        return Err("已有数据只接受一条启动授权".into());
    }
    let observed = documents::parse_fragment(line, "已有数据启动授权")?;
    if observed != *expected {
        return Err("已有数据启动授权不属于当前运行身份".into());
    }
    Ok(())
}

pub(super) fn unchanged(
    root: &Path,
    snapshots: &[Snapshot],
    configuration_snapshots: &[(PathBuf, Vec<u8>)],
) -> Result<()> {
    for snapshot in snapshots {
        snapshot.unchanged(root)?;
    }
    crate::recovery::clone::native::s3::app_configuration::unchanged(configuration_snapshots)
}

pub(super) fn write_result(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

pub(super) fn reference_dataset(
    root: &Path,
    descriptor: &Value,
    snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<Value> {
    let path = crate::recovery::clone::native::lifecycle::manifest::bound_file(
        root,
        descriptor,
        "原参考计划",
        true,
    )?;
    let bytes = fs::read(&path)?;
    if super::super::model::sha(&bytes) != descriptor["sha256"]
        || bytes.len() as u64 != descriptor["bytes"].as_u64().ok_or("原参考计划长度缺失")?
    {
        return Err("原参考计划读取期间变化".into());
    }
    let reference = documents::parse_fragment(std::str::from_utf8(&bytes)?, "原参考计划")?;
    snapshots.push((path, bytes));
    crate::recovery::clone::native::s3::app_configuration::unchanged(snapshots)?;
    Ok(reference["dataset"].clone())
}
