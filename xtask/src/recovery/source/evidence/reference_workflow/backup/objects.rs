use std::{fs, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Result, Transport, artifact, inputs, native, text};
use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};
use native::s3::{Configuration, Operation, Request};

pub(super) struct Storage<'a, T> {
    pub(super) backend: &'a Path,
    pub(super) source: &'a Value,
    pub(super) transport: &'a T,
}

impl<T: Transport> Storage<'_, T> {
    pub(super) fn verify_owners(&self, work: &Path) -> Result<()> {
        let scope = text(&self.source["scope_id"])?;
        let mut buckets = inputs::domain::BUCKETS;
        buckets.sort_unstable();
        for bucket in buckets {
            let file = work.join(format!(
                "owner-source-{bucket}-{}.txt",
                uuid::Uuid::new_v4().simple()
            ));
            self.get(bucket, &format!("{scope}/.ryframe-owner"), &file, 4096)?;
            super::super::unlinked(&file)?;
            let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
            if fs::metadata(&file)?.len() != expected.len() as u64
                || fs::read(&file)? != expected.as_bytes()
            {
                return Err("对象 ownership 不属于本次明确隔离 scope".into());
            }
        }
        Ok(())
    }

    pub(super) fn store_buckets(
        &self,
        root: &Path,
        inventory: &Value,
        artifacts: &mut Vec<Value>,
    ) -> Result<()> {
        fs::create_dir(root.join("objects"))?;
        for bucket in native::json::array(&inventory["objects"], "备份对象库存")? {
            self.store_bucket(root, bucket, artifacts)?;
        }
        Ok(())
    }

    fn store_bucket(&self, root: &Path, objects: &Value, artifacts: &mut Vec<Value>) -> Result<()> {
        let bucket = text(&objects["bucket"])?;
        if !inputs::domain::BUCKETS.contains(&bucket) {
            return Err("备份对象桶未登记".into());
        }
        let resource = format!("objects:{bucket}");
        fs::create_dir(root.join(format!("objects/{bucket}")))?;
        let mut entries = Vec::new();
        for entry in native::json::array(&objects["entries"], "备份对象条目")? {
            let (file, index) = self.store_object(root, bucket, &resource, entry)?;
            artifacts.push(file);
            entries.push(index);
        }
        let relative = format!("objects/{bucket}/index.json");
        native::json::write_new(&root.join(&relative), &json!({"entries":entries}))?;
        artifacts.push(artifact(root, &relative, &resource)?);
        Ok(())
    }

    fn store_object(
        &self,
        root: &Path,
        bucket: &str,
        resource: &str,
        entry: &Value,
    ) -> Result<(Value, Value)> {
        let key = text(&entry["key"])?;
        let bytes = entry["bytes"].as_u64().ok_or("备份对象字节数无效")?;
        let filename = hex::encode(Sha256::digest(key.as_bytes()));
        let relative = format!("objects/{bucket}/{filename}.bin");
        let response = self.get(bucket, key, &root.join(&relative), bytes.max(1))?;
        let file = artifact(root, &relative, resource)?;
        if file["bytes"] != entry["bytes"] || file["sha256"] != entry["sha256"] {
            return Err("外部对象复制与静止采集清单不一致".into());
        }
        let mut index = entry.clone();
        index["file"] = json!(relative);
        index["content_type"] = json!(content_type(&response)?);
        Ok((file, index))
    }

    fn get(&self, bucket: &str, key: &str, file: &Path, max_bytes: u64) -> Result<Value> {
        let scope = text(&self.source["scope_id"])?;
        if !inputs::domain::BUCKETS.contains(&bucket) || max_bytes == 0 {
            return Err("备份对象读取必须绑定已登记桶和正整数字节上限".into());
        }
        native::input::name(scope)?;
        native::s3::protocol::validate_key(scope, key, true)?;
        let destination =
            validate_local_test_path(file, self.backend, LocalTestPathKind::OutputFile)?;
        let result = self.transport.s3(Request {
            configuration: Configuration::read_app_binding(self.backend, &self.source["s3"])?,
            bucket: bucket.to_owned(),
            operation: Operation::Get {
                key: key.to_owned(),
                if_match: None,
                version: None,
                destination,
                max_bytes,
            },
        });
        // 复核真实读请求绑定的 TOML，不能用变化后的凭据继续导出。
        Configuration::read_app_binding(self.backend, &self.source["s3"])?;
        result
    }
}

fn content_type(response: &Value) -> Result<&str> {
    let Some(value) = response.get("ContentType") else {
        return Ok("application/octet-stream");
    };
    value
        .as_str()
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or_else(|| "备份对象 ContentType 无效".into())
}
