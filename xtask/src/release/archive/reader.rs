use std::{collections::BTreeSet, fs::File, path::Path};

use crate::Result;

use super::super::support::read_limited;
use super::ARCHIVE_RECEIPT_LIMITS;

pub(crate) fn read_archive(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let file = File::open(path)
        .map_err(|error| format!("无法打开全栈证据 ZIP {}：{error}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("全栈证据 ZIP 格式无效：{error}"))?;
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取全栈证据 ZIP 条目：{error}"))?;
        let path = entry
            .name()
            .map_err(|error| format!("无法读取全栈证据 ZIP 条目路径：{error}"))?
            .into_owned();
        validate_member_path(&path)?;
        let name = path.rsplit('/').next().unwrap_or_default();
        let Some(limit) = ARCHIVE_RECEIPT_LIMITS
            .iter()
            .find_map(|(candidate, limit)| (*candidate == name).then_some(*limit))
        else {
            continue;
        };
        if entry.is_dir() {
            return Err(format!("全栈产物 {name} 不能是目录").into());
        }
        if entry.size() == 0 || entry.size() > limit as u64 {
            return Err(format!("全栈产物 {name} 大小无效").into());
        }
        if !names.insert(name.to_owned()) {
            return Err(format!("全栈归档包含重复 {name}").into());
        }
        let raw = read_limited(&mut entry, limit, &format!("全栈产物 {name}"))?;
        if raw.len() as u64 != entry.size() {
            return Err(format!("全栈产物 {name} 实际大小与归档元数据不一致").into());
        }
        entries.push((path, raw));
    }
    Ok(entries)
}

fn validate_member_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', '\0'])
        || value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        || value
            .split('/')
            .next()
            .is_some_and(|part| part.len() == 2 && part.ends_with(':'))
    {
        return Err("全栈归档成员路径无效".into());
    }
    Ok(())
}
