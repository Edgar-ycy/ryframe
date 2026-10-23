use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::{files, inputs, model, native, path, protocol, receipts};
use crate::Result;

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        // 只移除本阶段创建的空目录；异常现场或外来文件保持原样。
        let _ = fs::remove_dir(&self.0);
    }
}

pub(super) fn run(raw: &str) -> Result<Option<Value>> {
    let request = protocol::request(&json!({"raw":raw}))?;
    let command = request["command"].as_str().ok_or("参考操作缺失")?;
    if !matches!(command, "plan" | "copy" | "damage") {
        return Ok(None);
    }
    if command == "plan" && request.as_object().is_none_or(|value| value.len() != 4) {
        return Ok(None);
    }
    let backend = path(&request["backend_dir"])?;
    let plan = native::json::read(path(&request["plan"])?)?;
    inputs::domain::validate_plan(&plan, backend)?;
    if command == "plan" {
        return model::summary(&json!({"plan":plan})).map(Some);
    }
    let work = files::work(&json!({"plan":plan}), true)?;
    let lock = path(&work)?.join(".reference-lock");
    fs::create_dir(&lock)?;
    let _guard = Lock(lock);
    let receipt = receipts::start(
        &json!({"plan":plan,"command":command,"copy_id":request["copy_id"],
        "root":request["backup_root"],"receipt":null,"bindings":null}),
    )?;
    let result = (|| {
        let root = path(&request["backup_root"])?;
        super::unlinked(root)?;
        let manifest = native::json::read(&root.join("manifest.json"))?;
        if command == "copy" {
            files::copy(
                &json!({"work":work,"root":root,"manifest":manifest,"copy_id":request["copy_id"]}),
            )
        } else {
            files::damage(
                &json!({"work":work,"root":root,"manifest":manifest,"relative":request["artifact"],"missing":request["missing"]}),
            )
        }
    })();
    match result {
        Ok(value) => {
            receipts::finish(
                &json!({"receipt":receipt,"result":value,"failure":null,"resource_images":null}),
            )?;
            Ok(Some(value))
        }
        Err(error) => {
            let published = receipts::finish(&json!({"receipt":receipt,"result":null,
                "failure":{"error_type":"ReferenceLocalFailure","message":"参考本地阶段失败；保留现场，禁止覆盖或自动重放"},"resource_images":null}));
            if published.is_err() {
                return Err(
                    "参考本地阶段与失败收据发布均未确认；保留运行收据和现场，禁止自动重放".into(),
                );
            }
            Err(error)
        }
    }
}
