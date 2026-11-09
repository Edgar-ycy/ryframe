//! 冻结秘密字节和绑定；只创建明确目标，不删除失败现场。
use super::super::protocol::GuardError;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::{digest, json as document},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
const NAMES: [&str; 7] = [
    "mysql-client.cnf",
    "rustfs-access-key.txt",
    "rustfs-secret-key.txt",
    "redis-password.txt",
    "reset-admin-password.txt",
    "jwt-secret.txt",
    "metrics-token.txt",
];
pub(super) struct Secrets {
    pub(super) directory: PathBuf,
    pub(super) bindings: Value,
    contents: BTreeMap<String, Vec<u8>>,
    manifest: Option<Value>,
}
impl Secrets {
    pub(super) fn open(root: &Path, original: &Path, selected: Option<&Path>) -> Result<Self> {
        let boundary = original.join(".local-tests/reference-fixture");
        let directory = selected
            .map(|path| {
                if path.is_absolute() {
                    path.to_owned()
                } else {
                    root.join(path)
                }
            })
            .unwrap_or_else(|| boundary.join("secrets"));
        if selected.is_some() {
            validate_local_test_path(&directory, root, LocalTestPathKind::ExistingDirectory)?;
        }
        validate_absolute_path(&directory, LocalTestPathKind::ExistingDirectory)?;
        if !directory.starts_with(&boundary) {
            return Err(Box::new(GuardError("秘密集必须属于冻结来源工作树")));
        }
        reject_failed(&directory)?;
        let mut contents = BTreeMap::new();
        let mut bindings = serde_json::Map::new();
        for name in NAMES {
            let path = directory.join(name);
            validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
            let before = binding(&path)?;
            let bytes = fs::read(&path)?;
            if binding(&path)? != before {
                return Err(Box::new(GuardError("秘密文件读取期间变化")));
            }
            contents.insert(name.into(), bytes);
            bindings.insert(name.into(), before);
        }
        let bindings = Value::Object(bindings);
        let manifest = if directory != boundary.join("secrets") {
            let path = directory.join("secret-set.json");
            let before = binding(&path)?;
            let value = document::read(&path)?;
            document::exact(
                &value,
                &["format_version", "kind", "status", "source", "files"],
                "秘密集收据",
            )
            .map_err(|_| GuardError("生成秘密集收据无效"))?;
            if value["format_version"] != 1
                || value["kind"] != "reference-fixture-secret-set"
                || value["status"] != "generated"
                || value["files"] != bindings
                || binding(&path)? != before
            {
                return Err(Box::new(GuardError("生成秘密集收据无效")));
            }
            Some(before)
        } else {
            None
        };
        let result = Self {
            directory,
            bindings,
            contents,
            manifest,
        };
        result.check()?;
        Ok(result)
    }
    pub(super) fn values(&self) -> Result<Value> {
        let mut values = serde_json::Map::new();
        for (key, file) in [
            ("access_key", NAMES[1]),
            ("secret_key", NAMES[2]),
            ("redis_password", NAMES[3]),
            ("admin_password", NAMES[4]),
            ("jwt_secret", NAMES[5]),
            ("metrics_token", NAMES[6]),
        ] {
            let value = std::str::from_utf8(&self.contents[file])
                .map_err(|_| GuardError("秘密文件必须为 UTF-8"))?
                .trim();
            if value.is_empty() {
                return Err(Box::new(GuardError("秘密文件不能为空")));
            }
            values.insert(key.into(), json!(value));
        }
        Ok(Value::Object(values))
    }
    pub(super) fn check(&self) -> Result<()> {
        reject_failed(&self.directory)?;
        for name in NAMES {
            let path = self.directory.join(name);
            if binding(&path)? != self.bindings[name] || fs::read(&path)? != self.contents[name] {
                return Err(Box::new(GuardError("秘密集在准备期间变化")));
            }
        }
        if let Some(expected) = &self.manifest
            && binding(&self.directory.join("secret-set.json"))? != *expected
        {
            return Err(Box::new(GuardError("秘密集收据变化")));
        }
        Ok(())
    }
}
fn reject_failed(directory: &Path) -> Result<()> {
    match fs::symlink_metadata(directory.join("failed.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Box::new(GuardError("秘密集存在失败标记或无法核验"))),
    }
}
pub(super) fn binding(path: &Path) -> Result<Value> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let value = digest::file(path)?;
    Ok(json!({"path":path,"bytes":value.bytes,"sha256":value.sha256}))
}
pub(super) fn copy_mysql(root: &Path, source: &Path, destination: &Path) -> Result<()> {
    validate_local_test_path(source, root, LocalTestPathKind::ExistingFile)?;
    validate_local_test_path(destination, root, LocalTestPathKind::OutputFile).or_else(|_| {
        validate_local_test_path(destination, root, LocalTestPathKind::ExistingFile)
    })?;
    let before = binding(source)?;
    let bytes = fs::read(source)?;
    if source != destination {
        if destination.exists() {
            if fs::read(destination)? != bytes {
                return Err(Box::new(GuardError("目标 MySQL 凭据与冻结来源不同")));
            }
        } else {
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or(GuardError("凭据目标父目录缺失"))?,
            )?;
            write_private(destination, &bytes)?;
        }
    }
    if binding(source)? != before || fs::read(source)? != bytes {
        return Err(Box::new(GuardError("MySQL 凭据来源变化")));
    }
    Ok(())
}
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    validate_absolute_path(path, LocalTestPathKind::OutputFile)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    private_permissions(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub(super) fn private_permissions(path: &Path, directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, directory);
    }
    Ok(())
}
