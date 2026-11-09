//! 夹具工具预检使用已登记的 Windows Redis 和本机工具。
use super::super::{digest, json as doc};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn local_tools(review: &Value) -> Result<Value> {
    let mut result = json!({});
    for role in ["mysql", "mysqldump", "rustfs"] {
        let value = &review["tools"][role];
        let path = super::model::plain(Path::new(doc::text(&value["path"], "夹具工具")?))?;
        if digest::file(&path)?.sha256 != doc::text(&value["sha256"], "夹具工具摘要")? {
            return Err("夹具本机工具字节变化".into());
        }
        result[role] = json!({"path":path,"sha256":value["sha256"]});
    }
    Ok(result)
}

pub(in crate::recovery) fn observe(backend: &Path, review: &Value) -> Result<Value> {
    let ready = review["ready_for_execution"]
        .as_bool()
        .ok_or("夹具预检状态无效")?;
    if ready && review["tools"]["redis_server"]["platform"] != "windows" {
        return Err("历史 WSL Redis 审阅仅供核对；请新建 Windows Redis 隔离场景".into());
    }
    let redis = super::windows_tool::observe(backend)?
        .ok_or("夹具工具预检需要显式配置 verification.windows_redis")?;
    let mut result = local_tools(review)?;
    result["redis_server"] = redis;
    Ok(result)
}

#[cfg(test)]
#[test]
fn live_preflight_requires_windows_configuration_and_refuses_ready_legacy_review() {
    let local = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let legacy = json!({
        "ready_for_execution": false,
        "tools": {
            "redis_server": {"distribution": "Ubuntu-24.04"},
            "wsl": {"path": "D:/missing-wsl-sentinel.exe", "sha256": "a".repeat(64)},
        },
    });
    let error = observe(temporary.path(), &legacy).unwrap_err();
    assert_eq!(
        error.to_string(),
        "夹具工具预检需要显式配置 verification.windows_redis"
    );

    let mut ready_windows = legacy.clone();
    ready_windows["ready_for_execution"] = json!(true);
    ready_windows["tools"]["redis_server"] = json!({"platform": "windows"});
    let error = observe(temporary.path(), &ready_windows).unwrap_err();
    assert_eq!(
        error.to_string(),
        "夹具工具预检需要显式配置 verification.windows_redis"
    );

    let mut ready_legacy = legacy;
    ready_legacy["ready_for_execution"] = json!(true);
    let config = temporary.path().join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(config.join("tools.toml"), b"invalid [").unwrap();
    let error = observe(temporary.path(), &ready_legacy).unwrap_err();
    assert_eq!(
        error.to_string(),
        "历史 WSL Redis 审阅仅供核对；请新建 Windows Redis 隔离场景"
    );
}
