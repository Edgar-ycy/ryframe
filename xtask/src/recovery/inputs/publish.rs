use std::{fs, path::Path};

use serde::Serialize;
use serde_json::Value;

use super::document;
use crate::Result;

#[derive(Serialize)]
struct Summary<'a> {
    status: String,
    sha256: &'a str,
    value: &'a Value,
}

pub(crate) fn finish(
    operation: &str,
    output: Option<&Path>,
    value: &Value,
    rebuild: impl Fn() -> Result<Value>,
) -> Result<()> {
    let mut written = false;
    if let Some(target) = output {
        if rebuild()? != *value {
            return Err("正式恢复输入在发布前发生变化".into());
        }
        let content = document::canonical_bytes(value)?;
        let parent = target.parent().ok_or("正式恢复输入缺少父目录")?;
        let name = target
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("正式恢复输入文件名必须为 UTF-8")?;
        let pending = parent.join(format!(".{name}.{}.pending", uuid::Uuid::new_v4().simple()));
        let publish_result: Result<()> = (|| {
            if !document::write_pending_and_link(target, value, &pending)? {
                return Err("正式恢复输入发布未完成".into());
            }
            document::verify_published(target, value, &content)?;
            if rebuild()? != *value {
                return Err("正式恢复输入在发布后发生变化；保留文件且禁止重放".into());
            }
            Ok(())
        })();
        if publish_result.is_ok() {
            written = true;
        }
        match fs::remove_file(&pending) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if written => {
                eprintln!("正式恢复输入已发布，但临时文件清理失败：{error}");
            }
            Err(error) => {
                return Err(format!("正式恢复输入发布失败，且临时文件清理失败：{error}").into());
            }
        }
        publish_result?;
    }
    let sha256 = document::hash(value)?;
    println!(
        "{}",
        serde_json::to_string(&Summary {
            status: format!(
                "restore_{operation}_{}",
                if written { "published" } else { "planned" }
            ),
            sha256: &sha256,
            value,
        })?
    );
    Ok(())
}
