//! 文件位置、字节和独占新建沿用原受管边界；不覆盖失败现场。
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common, target_verify},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub(super) fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("未登记生产者或控制锁仍存在；保留现场，不自动接管".into()),
    }
}

pub(in crate::recovery::clone::native) fn bindings(
    backend: &Path,
    value: &Value,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    match value {
        Value::Object(map)
            if map.len() == 3
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|k| map.contains_key(*k)) =>
        {
            paths.insert(common::bound_file(backend, value, "固定请求文件")?);
        }
        Value::Object(map)
            if map.len() == 2 && ["path", "sha256"].iter().all(|k| map.contains_key(*k)) =>
        {
            paths.insert(target_verify::input::request::external_file(value)?);
        }
        Value::Object(map) => {
            for (key, value) in map {
                if key == "execution_backend" && value.get("execution_source").is_some() {
                    paths.extend(
                        crate::recovery::fixture_control::source_pair::execution::target_protected_inputs(
                            backend, value,
                        )?,
                    );
                } else {
                    bindings(backend, value, paths)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                bindings(backend, value, paths)?;
            }
        }
        _ => (),
    }
    Ok(())
}

pub(super) fn publish(backend: &Path, path: &Path, value: &Value) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let before = common::binding(&common::common_file(backend, path, "已有固定收据")?)?;
            if common::read_bound(backend, &before, "已有固定收据")? != *value {
                return Err("已有固定收据内容不同，拒绝覆盖或自动重放".into());
            }
            common::bound_file(backend, &before, "已有固定收据后像")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            document::write_new(path, value)?
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn combine<T>(primary: Result<T>, checked: Result<()>) -> Result<T> {
    match (primary, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(super::diagnostic::combined(first, second)),
    }
}

pub(super) fn same_directory(backend: &Path, recorded: &Value, selected: &Path) -> Result<bool> {
    let recorded = Path::new(document::text(recorded, "登记运行位置")?);
    common::local_directory(backend, recorded, "登记运行目录")?;
    common::local_directory(backend, selected, "当前运行目录")?;
    let position = fs::canonicalize(selected)?;
    let (device, inode) = common::device_inode_for(selected, &fs::symlink_metadata(selected)?)?;
    let expected = serde_json::json!({"path":selected,"device":device,"inode":inode});
    let same = fs::canonicalize(recorded)? == position;
    if same {
        common::directory_identity(
            backend,
            &serde_json::json!({"path":recorded,"device":device,"inode":inode}),
        )?;
    }
    common::directory_identity(backend, &expected)?;
    common::local_directory(backend, recorded, "登记运行目录后像")?;
    if fs::canonicalize(selected)? != position || (same && fs::canonicalize(recorded)? != position)
    {
        return Err("登记运行位置在核对期间漂移".into());
    }
    Ok(same)
}

pub(super) fn same_file(backend: &Path, recorded: &Value, selected: &Path) -> Result<bool> {
    let recorded = Path::new(document::text(recorded, "登记历史位置")?);
    common::common_file(backend, recorded, "登记历史文件")?;
    common::common_file(backend, selected, "当前历史文件")?;
    let frozen = target_verify::guard::Inputs::open([recorded.to_owned(), selected.to_owned()])?;
    let selected_identity = common::device_inode_for(selected, &fs::symlink_metadata(selected)?)?;
    let same = crate::local_test_path::same_local_test_file(backend, selected, recorded)?;
    if same
        && common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)?
            != selected_identity
    {
        return Err("登记历史位置相同但实际文件身份不同".into());
    }
    frozen.check()?;
    Ok(same)
}
