use super::model::*;
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn scan(
    root: &Path,
    current: &Path,
    files: &mut Vec<Value>,
    guards: &mut Vec<Value>,
    limits: (u64, u64),
    total: &mut u64,
) -> Result<()> {
    directory(current)?;
    let mut entries = fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        safe(&path)?;
        if entry.file_type()?.is_dir() {
            scan(root, &path, files, guards, limits, total)?;
        } else {
            check(files.len() < limits.0 as usize, "浏览器产物超过文件数上限")?;
            let guard = snapshot(&path, limits.1.saturating_sub(*total))?;
            *total = total
                .checked_add(
                    guard["descriptor"]["bytes"]
                        .as_u64()
                        .ok_or("产物字节数无效")?,
                )
                .ok_or("产物字节溢出")?;
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("产物路径不是UTF-8")?
                .replace('\\', "/");
            files.push(json!({"path":relative,"bytes":guard["descriptor"]["bytes"],"sha256":guard["descriptor"]["sha256"]}));
            guards.push(guard);
        }
    }
    Ok(())
}
pub(super) fn capture(root: &Path, parent: &Path, limits: (u64, u64)) -> Result<Value> {
    check(
        limits.0 > 0 && limits.0 <= MAX_FILES && limits.1 > 0 && limits.1 <= MAX_BYTES,
        "浏览器产物上限无效",
    )?;
    directory(parent)?;
    directory(root)?;
    check(
        root != parent && root.starts_with(parent),
        "浏览器产物不在已登记父目录内",
    )?;
    let (mut files, mut guards, mut total) = (Vec::new(), Vec::new(), 0);
    scan(root, root, &mut files, &mut guards, limits, &mut total)?;
    check(!files.is_empty(), "浏览器产物不能为空")?;
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    for guard in &guards {
        unchanged(guard)?;
    }
    let (mut repeated, mut repeated_guards, mut repeated_total) = (Vec::new(), Vec::new(), 0);
    scan(
        root,
        root,
        &mut repeated,
        &mut repeated_guards,
        limits,
        &mut repeated_total,
    )?;
    repeated.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    check(files == repeated, "浏览器完整清单采集期间发生变化")?;
    Ok(
        json!({"manifest":{"format_version":1,"kind":"bounded-artifact-manifest","root":root,
        "limits":{"files":limits.0,"bytes":limits.1},"total_files":files.len(),"total_bytes":total,"files":files},
        "guards":guards,"root":root,"parent":parent}),
    )
}
pub(super) fn verify(value: &Value, root: &Path, parent: &Path) -> Result<Value> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "root",
            "limits",
            "total_files",
            "total_bytes",
            "files",
        ],
        "浏览器完整清单",
    )?;
    strict::exact(&value["limits"], &["files", "bytes"], "浏览器完整清单上限")?;
    let limits = (
        value["limits"]["files"].as_u64().ok_or("文件数上限无效")?,
        value["limits"]["bytes"].as_u64().ok_or("字节数上限无效")?,
    );
    let current = capture(root, parent, limits)?;
    check(
        current["manifest"] == *value,
        "浏览器产物与已发布完整清单不一致",
    )?;
    Ok(value.clone())
}
pub(super) fn assert_unchanged(value: &Value) -> Result<()> {
    for guard in value["guards"].as_array().ok_or("完整清单guard无效")? {
        unchanged(guard)?;
    }
    verify(
        &value["manifest"],
        &path(&value["root"])?,
        &path(&value["parent"])?,
    )?;
    Ok(())
}
