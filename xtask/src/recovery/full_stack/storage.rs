use super::context::journal;
pub(super) use crate::recovery::clone::native::input::BUCKETS;
use crate::{Result, recovery::clone::native::json as documents};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn prepare_buckets(
    output: &Path,
    scope: &str,
    binding: &Value,
    mut send: impl FnMut(&str, &Value) -> Result<Value>,
) -> Result<()> {
    let path = output.join("ci-storage.json");
    let mut receipt = json!({"format_version":1,"buckets":[],"automatic_retry_permitted":false});
    documents::write_new(&path, &receipt)?;
    for (index, bucket) in BUCKETS.iter().enumerate() {
        receipt["buckets"]
            .as_array_mut()
            .ok_or("对象收据无效")?
            .push(json!({"bucket":bucket,"state":"creation-unknown"}));
        journal(&path, &receipt)?;
        let mut request = json!({"format_version":1,"kind":"ryframe-native-s3-create-bucket",
            "scope_id":scope,"configuration_binding":binding,"bucket":bucket});
        let result = send("__s3-create-bucket", &request)?;
        if result["status"] != "created_unverified" {
            return Err("对象桶创建未确认，保留 ci-storage.json，禁止自动重放".into());
        }
        request["kind"] = json!("ryframe-native-s3-inspect");
        request["operation"] = json!("head-bucket");
        request["prefix"] = Value::Null;
        request["continuation"] = Value::Null;
        send("__s3-inspect", &request)?;
        receipt["buckets"][index]["state"] = json!("created");
        journal(&path, &receipt)?;
    }
    Ok(())
}
