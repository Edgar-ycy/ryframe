use super::super::super::{exact_object, file_binding, read_json, validate_path};
use super::super::protocol::GuardError;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(super) const NAMES: [&str; 7] = [
    "mysql-client.cnf",
    "rustfs-access-key.txt",
    "rustfs-secret-key.txt",
    "redis-password.txt",
    "reset-admin-password.txt",
    "jwt-secret.txt",
    "metrics-token.txt",
];
pub(super) type Contents = BTreeMap<String, Vec<u8>>;
pub(super) struct Frozen {
    directory: PathBuf,
    pub(super) contents: Contents,
    pub(super) bindings: Value,
    manifest: Option<Value>,
}

impl Frozen {
    pub(super) fn open(root: &Path, execution: &Path, selected: Option<&Path>) -> Result<Self> {
        let boundary = execution.join(".local-tests/reference-fixture");
        let directory = selected
            .map(|p| {
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    root.join(p)
                }
            })
            .unwrap_or_else(|| boundary.join("secrets"));
        validate_path(
            &directory,
            root,
            LocalTestPathKind::ExistingDirectory,
            "秘密源目录",
        )?;
        if !directory.starts_with(&boundary) || directory == boundary {
            return Err(Box::new(GuardError(
                "秘密源必须属于执行工作树 reference-fixture 子目录",
            )));
        }
        reject_failed(&directory)?;
        let mut contents = Contents::new();
        let mut bindings = serde_json::Map::new();
        for name in NAMES {
            let path = directory.join(name);
            validate_path(&path, root, LocalTestPathKind::ExistingFile, "秘密源文件")?;
            let binding = file_binding(&path, "秘密源文件")?;
            let bytes = fs::read(&path)?;
            if binding != file_binding(&path, "秘密源文件")? || !matches_bytes(&binding, &bytes)
            {
                return Err(Box::new(GuardError("秘密源在读取期间变化")));
            }
            bindings.insert(name.into(), binding);
            contents.insert(name.into(), bytes);
        }
        let manifest = if directory != boundary.join("secrets") {
            let path = directory.join("secret-set.json");
            let binding = file_binding(&path, "秘密集收据")?;
            let value = read_json(&path, "秘密集收据", 16 * 1024 * 1024)?;
            exact_object(
                &value,
                &["format_version", "kind", "status", "source", "files"],
                "秘密集收据",
            )?;
            if value["format_version"].as_u64() != Some(1)
                || value["kind"] != "reference-fixture-secret-set"
                || value["status"] != "generated"
                || value["files"] != Value::Object(bindings.clone())
            {
                return Err(Box::new(GuardError("秘密集收据或七文件绑定无效")));
            }
            Some(binding)
        } else {
            None
        };
        let result = Self {
            directory,
            contents,
            bindings: Value::Object(bindings),
            manifest,
        };
        result.check()?;
        Ok(result)
    }

    pub(super) fn check(&self) -> Result<()> {
        reject_failed(&self.directory)?;
        if verify(&self.directory, &self.contents)? != self.bindings {
            return Err(Box::new(GuardError("秘密源文件在复制期间变化")));
        }
        if let Some(expected) = &self.manifest
            && file_binding(&self.directory.join("secret-set.json"), "秘密集收据")? != *expected
        {
            return Err(Box::new(GuardError("秘密集收据在复制期间变化")));
        }
        Ok(())
    }
}

fn reject_failed(directory: &Path) -> Result<()> {
    match fs::symlink_metadata(directory.join("failed.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Box::new(GuardError(
            "秘密目录存在失败标记或无法核验，拒绝消费半成品",
        ))),
    }
}

fn matches_bytes(binding: &Value, bytes: &[u8]) -> bool {
    binding["bytes"] == json!(bytes.len())
        && binding["sha256"] == hex::encode(Sha256::digest(bytes))
}

pub(super) fn verify(directory: &Path, contents: &Contents) -> Result<Value> {
    let mut result = serde_json::Map::new();
    for name in NAMES {
        let path = directory.join(name);
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        let binding = file_binding(&path, "秘密文件")?;
        let bytes = fs::read(&path)?;
        if bytes != contents[name]
            || !matches_bytes(&binding, &bytes)
            || file_binding(&path, "秘密文件")? != binding
        {
            return Err(Box::new(GuardError("秘密文件内容或绑定在核验期间变化")));
        }
        result.insert(name.into(), binding);
    }
    Ok(Value::Object(result))
}

pub(super) fn publish(
    directory: &Path,
    contents: &Contents,
    after_sync: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    for name in NAMES {
        validate_absolute_path(directory, LocalTestPathKind::ExistingDirectory)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(name))?;
        file.write_all(&contents[name])?;
        file.sync_all()?;
        after_sync(&directory.join(name))?;
    }
    Ok(())
}
