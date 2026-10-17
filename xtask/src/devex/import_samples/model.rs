use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

pub(crate) const HEADERS: [&str; 5] = ["用户名", "昵称", "邮箱", "手机号", "部门完整路径"];
pub(crate) const MAX_TEMPLATE_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const LARGE_PAYLOAD_BYTES: usize = 3 * 1024 * 1024;
pub(crate) const LARGE_PAYLOAD_NAME: &str = "ryframe-upload-fixture.bin";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImportSampleModel {
    pub(crate) version: u8,
    pub(crate) rows_per_file: u8,
    pub(crate) username: String,
    pub(crate) nickname: String,
    pub(crate) email: String,
    pub(crate) phone: String,
    pub(crate) department: String,
}

pub(crate) fn model() -> ImportSampleModel {
    ImportSampleModel {
        version: 1,
        rows_per_file: 1,
        username: "dv{namespace}{worker:02x}{cycle:04x}".into(),
        nickname: "性能导入".into(),
        email: "{username}@example.test".into(),
        phone: String::new(),
        department: "current_template_sheet2_A2".into(),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PerformanceRequest {
    pub(crate) template: PathBuf,
    pub(crate) template_sha256: String,
    pub(crate) model_sha256: String,
    pub(crate) directory: PathBuf,
    pub(crate) namespace: String,
    pub(crate) concurrency: u16,
    pub(crate) cycles: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ManifestFile {
    pub(crate) sample: String,
    pub(crate) filename: String,
    pub(crate) username: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ImportSampleManifest {
    pub(crate) format_version: u8,
    pub(crate) namespace: String,
    pub(crate) template_sha256: String,
    pub(crate) model: ImportSampleModel,
    pub(crate) model_sha256: String,
    pub(crate) concurrency: u16,
    pub(crate) cycles: u16,
    pub(crate) files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PublishedFile {
    pub(crate) path: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrowserReceipt {
    #[serde(flatten)]
    pub(crate) published: PublishedFile,
    pub(crate) template_sha256: String,
    pub(crate) username: String,
    pub(crate) rows: u8,
    pub(crate) large: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TemplateReceipt {
    pub(crate) template_sha256: String,
    pub(crate) department_path: String,
    pub(crate) department_sha256: String,
}

#[derive(Debug, Clone)]
pub(crate) struct TemplateArchive {
    pub(crate) member_names: Vec<String>,
    pub(crate) sheet1: Vec<u8>,
    pub(crate) department: String,
}

pub(crate) fn model_sha256() -> Result<String> {
    let encoded = serde_json::to_vec(&serde_json::json!({
        "department": "current_template_sheet2_A2",
        "email": "{username}@example.test",
        "nickname": "性能导入",
        "phone": "",
        "rows_per_file": 1,
        "username": "dv{namespace}{worker:02x}{cycle:04x}",
        "version": 1,
    }))?;
    Ok(sha256_hex(&encoded))
}

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn valid_namespace(value: &str) -> bool {
    value.len() == 20
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn performance_username(namespace: &str, worker: u16, cycle: u16) -> String {
    format!("dv{namespace}{worker:02x}{cycle:04x}")
}

pub(crate) fn department_sha256(department: &str) -> String {
    sha256_hex(department.as_bytes())
}
