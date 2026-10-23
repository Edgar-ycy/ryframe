use super::EvidenceObjects;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::s3::{self, Configuration, Operation, Request},
};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 64 * 1024 * 1024;
pub(super) struct Objects {
    configuration: Value,
    scope: String,
    directory: PathBuf,
    local: Option<PathBuf>,
}

impl Objects {
    pub(super) fn new(
        root: &Path,
        runtime: &Path,
        table: &toml::Table,
        scope: &str,
    ) -> Result<Self> {
        let value = serde_json::to_value(table)?;
        let configuration = value["object_storage"].clone();
        let local = match configuration["backend"].as_str() {
            Some("local") => {
                let base = PathBuf::from(
                    configuration["local_base_dir"]
                        .as_str()
                        .ok_or("缺少 object_storage.local_base_dir")?,
                );
                let path = if base.is_absolute() {
                    base
                } else {
                    root.join(base)
                };
                validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)?;
                Some(path.join("exports").join(scope))
            }
            Some("rustfs" | "minio" | "s3") => {
                let endpoint = url::Url::parse(
                    configuration["endpoint"]
                        .as_str()
                        .ok_or("缺少对象 endpoint")?,
                )?;
                if !matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1")) {
                    return Err("对象验收只接受明确的本机 S3 测试端点".into());
                }
                None
            }
            _ => return Err("object_storage.backend 无效".into()),
        };
        Ok(Self {
            configuration,
            scope: scope.to_owned(),
            directory: runtime.into(),
            local,
        })
    }

    fn request(&self, operation: Operation) -> Result<Value> {
        let configuration = serde_json::json!({
            "endpoint":self.configuration["endpoint"], "region":self.configuration["region"],
            "access_key":self.configuration["access_key"], "secret_key":self.configuration["secret_key"]
        });
        s3::execute(Request {
            configuration: Configuration::read(&configuration)?,
            bucket: "exports".into(),
            operation,
        })
    }
}

impl EvidenceObjects for Objects {
    fn bytes(&self, name: &str) -> Result<Option<Vec<u8>>> {
        key(name)?;
        if let Some(local) = &self.local {
            let path = local.join(name);
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // 缺失也要检查现存父路径，不能把断链或越界链接当作已删除。
                    for ancestor in path.ancestors().skip(1) {
                        if fs::symlink_metadata(ancestor).is_ok() {
                            validate_absolute_path(ancestor, LocalTestPathKind::ExistingDirectory)?;
                            break;
                        }
                    }
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
            let mut content = Vec::new();
            fs::File::open(path)?
                .take(LIMIT + 1)
                .read_to_end(&mut content)?;
            if content.len() as u64 > LIMIT {
                return Err("浏览器验收对象超过 64 MiB 上限".into());
            }
            return Ok(Some(content));
        }
        // 精确 HEAD/GET，不依赖列表第一页，也不会把权限或传输错误当作不存在。
        let key = format!("{}/{name}", self.scope);
        let metadata = self.request(Operation::Head {
            key: key.clone(),
            if_match: None,
            version: None,
        })?;
        if metadata["status"] == "not_found" {
            return Ok(None);
        }
        let temporary = tempfile::Builder::new()
            .prefix("artifact-object-")
            .tempdir_in(&self.directory)?;
        let destination = temporary.path().join("object");
        let result = self.request(Operation::Get {
            key,
            if_match: metadata["ETag"].as_str().map(ToOwned::to_owned),
            version: metadata["VersionId"].as_str().map(ToOwned::to_owned),
            destination: destination.clone(),
            max_bytes: LIMIT,
        });
        if let Err(error) = result {
            let retained = temporary.keep();
            return Err(format!(
                "{error}；读取失败证据保留于 {}，没有远端写入",
                retained.display()
            )
            .into());
        }
        Ok(Some(fs::read(destination)?))
    }
}

pub(super) fn key(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', ':', '\0'])
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        Err("对象键必须是安全的相对路径".into())
    } else {
        Ok(())
    }
}
