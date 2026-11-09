//! 旧单进程运行保留原 v1 生产者事件及既有外部运行目录。
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{cache::handoff::history, digest, json as document},
};
use serde_json::{Value, json};
use std::{fs, io::ErrorKind, path::Path};

pub(super) const FILE: &str = "producer-history.json";

pub(super) fn stopped(directory: &Path, scope: &str, role: &str, identity: &Value) -> Result<()> {
    let path = directory.join(FILE);
    let (before, mut value) = match fs::symlink_metadata(&path) {
        Ok(_) => {
            validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
            let before = digest::file(&path)?;
            let value = document::read(&path)?;
            (Some(before), value)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (
            None,
            json!({"format_version":1,"kind":"devex-clone-producer-history",
                "scope_id":scope,"runtime_directory":directory,"events":[]}),
        ),
        Err(error) => return Err(error.into()),
    };
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-producer-history"
        || value["scope_id"] != scope
        || value["runtime_directory"] != json!(directory)
    {
        return Err("原生产者历史与当前 scope 或运行目录不符，拒绝覆盖".into());
    }
    history::append(
        &mut value,
        json!({"event":"stopped","role":role,"identity":identity}),
    )?;
    if let Some(before) = before {
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        if digest::file(&path)? != before {
            return Err("原生产者历史在停止事件发布前变化，保留现场".into());
        }
        document::write_replace(&path, &value)
    } else {
        document::write_new(&path, &value)
    }
}
