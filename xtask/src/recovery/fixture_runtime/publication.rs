use super::model::*;
use crate::Result;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};
use uuid::Uuid;

fn content(value: &Value, limit: usize) -> Result<Vec<u8>> {
    let mut raw = serde_json::to_vec_pretty(value)?;
    raw.push(b'\n');
    check(raw.len() <= limit, "浏览器发布文件超过上限")?;
    Ok(raw)
}
fn pending(path: &Path, raw: &[u8]) -> Result<Value> {
    safe(path)?;
    let mut stream = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    stream.write_all(raw)?;
    stream.sync_all()?;
    drop(stream);
    let metadata = fs::symlink_metadata(path)?;
    check(
        crate::recovery::clone::native::resources::common::link_count_for(path, &metadata)? == 1,
        "浏览器待发布文件硬链接数无效",
    )?;
    let guard = snapshot(path, 16 * 1024 * 1024)?;
    check(fs::read(path)? == raw, "浏览器待发布文件内容漂移")?;
    unchanged(&guard)?;
    Ok(guard)
}
pub(super) fn terminal(directory_path: &Path, target: &Path, value: &Value) -> Result<()> {
    terminal_with(
        directory_path,
        target,
        value,
        || Ok(()),
        |path| fs::remove_file(path),
    )
}
fn terminal_with(
    directory_path: &Path,
    target: &Path,
    value: &Value,
    before_link: impl FnOnce() -> Result<()>,
    unlink: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    directory(directory_path)?;
    safe(target)?;
    check(
        target.parent() == Some(directory_path)
            && regex::Regex::new(r"^browser-[a-z0-9][a-z0-9-]{0,63}-result\.json$")?
                .is_match(target.file_name().and_then(|n| n.to_str()).unwrap_or("")),
        "成功收据路径不属于当前运行目录",
    )?;
    let staged = target.with_file_name(format!(
        "{}.pending-{}",
        target
            .file_name()
            .ok_or("成功收据缺少文件名")?
            .to_string_lossy(),
        Uuid::new_v4().simple()
    ));
    let guard = pending(&staged, &content(value, 16 * 1024 * 1024)?)?;
    before_link()?;
    check(!target.try_exists()?, "成功收据已存在，拒绝覆盖或重放")?;
    unchanged(&guard)?;
    fs::hard_link(&staged, target)?;
    // 删除 pending 是最终提交点；此后不能再增加会失败的校验或写入。
    unlink(&staged)?;
    Ok(())
}
pub(super) fn tool_capture(frontend: &Path) -> Result<Value> {
    let path = frontend.join(".local-tests/frontend-tools.json");
    safe(&path)?;
    let (value, guard) = if path.try_exists()? {
        let guard = snapshot(&path, 1024 * 1024)?;
        let (value, _) = document(&path)?;
        unchanged(&guard)?;
        (value, guard)
    } else {
        (json!({}), Value::Null)
    };
    Ok(json!({"path":path,"guard":guard,"value":value}))
}
pub(super) fn tool_assert(capture: &Value) -> Result<()> {
    let path = path(&capture["path"])?;
    if capture["guard"].is_null() {
        check(!path.try_exists()?, "浏览器工具配置被并发创建；禁止覆盖")
    } else {
        unchanged(&capture["guard"])
    }
}
pub(super) fn tool_publish(capture: &Value, configuration: &Value) -> Result<Value> {
    tool_publish_with(capture, configuration, || Ok(()), || Ok(()))
}
fn tool_publish_with(
    capture: &Value,
    configuration: &Value,
    after_flush: impl FnOnce() -> Result<()>,
    after_publish: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    tool_assert(capture)?;
    let path = path(&capture["path"])?;
    let parent = path.parent().ok_or("浏览器工具配置缺少父目录")?;
    fs::create_dir_all(parent)?;
    tool_assert(capture)?;
    let lock = path.with_file_name("frontend-tools.json.lock");
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .map_err(|_| "浏览器工具配置正在写入或有未确认写入；保留现场")?;
    let pending_path = parent.join(format!(
        ".frontend-tools.json.{}.pending",
        Uuid::new_v4().simple()
    ));
    let staging = pending_path.with_extension("publish");
    let mut attempted = false;
    let result: Result<Value> = (|| {
        tool_assert(capture)?;
        let mut value = capture["value"]
            .as_object()
            .ok_or("浏览器工具配置必须是对象")?
            .clone();
        value.insert("real_browser".into(), configuration.clone());
        let value = Value::Object(value);
        let raw = content(&value, 1024 * 1024)?;
        let guard = pending(&pending_path, &raw)?;
        after_flush()?;
        tool_assert(capture)?;
        unchanged(&guard)?;
        attempted = true;
        if capture["guard"].is_null() {
            fs::hard_link(&pending_path, &path)?;
        } else {
            fs::hard_link(&pending_path, &staging)?;
            tool_assert(capture)?;
            replace(&staging, &path)?;
        }
        after_publish()?;
        let published = tool_capture(parent.parent().ok_or("工具配置缺少前端目录")?)?;
        check(
            published["value"] == value && fs::read(&path)? == raw,
            "浏览器工具配置发布结果未知",
        )?;
        tool_assert(&published)?;
        fs::remove_file(&pending_path)?;
        Ok(published)
    })();
    drop(lock_file);
    match result {
        Ok(value) => {
            fs::remove_file(lock)?;
            Ok(value)
        }
        Err(_) => {
            if !attempted {
                let _ = fs::remove_file(&pending_path);
                let _ = fs::remove_file(lock);
            }
            Err("浏览器工具配置写入失败；核对漂移与待发布证据，禁止自动重放".into())
        }
    }
}
#[cfg(windows)]
fn replace(source: &Path, target: &Path) -> Result<()> {
    // tempfile 的安全 persist 使用平台原子替换，保留独立 pending 证据。
    let temp = tempfile::NamedTempFile::new_in(target.parent().ok_or("替换缺少父目录")?)?;
    let staged = temp.into_temp_path();
    fs::copy(source, &staged)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&staged)?
        .sync_all()?;
    staged
        .persist(target)
        .map_err(|_| "浏览器工具配置原子替换结果未知")?;
    fs::remove_file(source)?;
    Ok(())
}
#[cfg(not(windows))]
fn replace(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_commits_last_and_preserves_concurrent_target_and_pending() {
        let temp = super::super::tests::temporary();
        let root = temp.path();
        let target = root.join("browser-a-result.json");
        terminal(root, &target, &json!({"ok":true})).unwrap();
        assert!(terminal(root, &target, &json!({})).is_err());
        assert_eq!(document(&target).unwrap().0, json!({"ok":true}));
        let target = root.join("browser-b-result.json");
        assert!(
            terminal_with(
                root,
                &target,
                &json!({}),
                || {
                    fs::write(&target, b"racer")?;
                    Ok(())
                },
                |path| fs::remove_file(path)
            )
            .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"racer");
    }
    #[test]
    fn terminal_unlink_interruption_preserves_unknown_result() {
        let temp = super::super::tests::temporary();
        let root = temp.path();
        let target = root.join("browser-a-result.json");
        assert!(
            terminal_with(
                root,
                &target,
                &json!({}),
                || Ok(()),
                |_| Err(std::io::Error::other("injected"))
            )
            .is_err()
        );
        assert!(target.exists());
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);
    }
    #[test]
    fn tool_merge_preserves_fields_and_rejects_drift_and_concurrent_creation() {
        let temp = super::super::tests::temporary();
        let root = temp.path();
        fs::create_dir(root.join(".local-tests")).unwrap();
        write_new(
            &root.join(".local-tests/frontend-tools.json"),
            &json!({"other":{"a":1},"real_browser":{"old":true}}),
        )
        .unwrap();
        let before = tool_capture(root).unwrap();
        let after = tool_publish(&before, &json!({"scopeId":"new"})).unwrap();
        assert_eq!(
            after["value"],
            json!({"other":{"a":1},"real_browser":{"scopeId":"new"}})
        );
        assert!(tool_publish(&before, &json!({})).is_err());
        let absent_root = root.join("absent");
        fs::create_dir(&absent_root).unwrap();
        let absent = tool_capture(&absent_root).unwrap();
        fs::create_dir(absent_root.join(".local-tests")).unwrap();
        fs::write(absent_root.join(".local-tests/frontend-tools.json"), b"{}").unwrap();
        assert!(tool_publish(&absent, &json!({})).is_err());
    }
    #[test]
    fn unknown_tool_publish_keeps_lock_and_pending_and_blocks_replay() {
        let temp = super::super::tests::temporary();
        let root = temp.path();
        let before = tool_capture(root).unwrap();
        assert!(
            tool_publish_with(
                &before,
                &json!({"credentialEndpoint":"http://127.0.0.1:1/v1/credentials"}),
                || Ok(()),
                || Err("injected secret".into())
            )
            .is_err()
        );
        let parent = root.join(".local-tests");
        assert!(parent.join("frontend-tools.json.lock").exists());
        assert_eq!(fs::read_dir(parent).unwrap().count(), 3);
        assert!(tool_publish(&tool_capture(root).unwrap(), &json!({})).is_err());
    }
    #[test]
    fn tool_rechecks_after_pending_flush() {
        let temp = super::super::tests::temporary();
        let root = temp.path();
        let before = tool_capture(root).unwrap();
        assert!(
            tool_publish_with(
                &before,
                &json!({}),
                || {
                    fs::write(root.join(".local-tests/frontend-tools.json"), b"racer")?;
                    Ok(())
                },
                || Ok(())
            )
            .is_err()
        );
        assert_eq!(
            fs::read(root.join(".local-tests/frontend-tools.json")).unwrap(),
            b"racer"
        );
        assert!(!root.join(".local-tests/frontend-tools.json.lock").exists());
    }
}
