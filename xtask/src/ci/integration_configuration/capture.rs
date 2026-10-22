use super::files::{self, Image, diagnostic};
use ryframe_config::{AppConfig, Environment};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

// 捕获含凭据，不实现 Debug，不重新捕获发生漂移的来源。
pub(crate) struct Captured {
    pub(crate) directory: PathBuf,
    pub(crate) app: AppConfig,
    pub(crate) selected: toml::Table,
    images: BTreeMap<String, Option<Image>>,
    pub(crate) materials: BTreeMap<PathBuf, Image>,
    pub(crate) redis_endpoint_explicit: bool,
}

impl Captured {
    pub(crate) fn load(directory: &Path, metadata: bool) -> Result<Self, String> {
        files::require_directory(directory)?;
        let mut images = BTreeMap::new();
        images.insert(
            "app.toml".into(),
            files::read_optional(&directory.join("app.toml"))?,
        );
        let selected = files::read_required(&directory.join("app.test.toml"))?;
        let table: toml::Table = files::parse(&selected.bytes, &directory.join("app.test.toml"))?;
        let base: toml::Table = match &images["app.toml"] {
            Some(image) => files::parse(&image.bytes, &directory.join("app.toml"))?,
            None => toml::Table::new(),
        };
        let redis_endpoint_explicit = ["host", "port"].into_iter().all(|key| {
            [&table, &base].into_iter().any(|table| {
                table
                    .get("redis")
                    .and_then(toml::Value::as_table)
                    .is_some_and(|redis| redis.contains_key(key))
            })
        });
        images.insert("app.test.toml".into(), Some(selected));
        if metadata {
            images.insert(
                "test.toml".into(),
                Some(files::read_required(&directory.join("test.toml"))?),
            );
        }
        let app = AppConfig::load(directory, Environment::Test)
            .map_err(|error| error.message().to_owned())?;
        let mut materials = BTreeMap::new();
        if let Some(redis) = &app.redis {
            for value in [&redis.tls_ca, &redis.tls_client_cert, &redis.tls_client_key]
                .into_iter()
                .flatten()
            {
                let path = PathBuf::from(value);
                capture_material(&mut materials, &path, directory)?;
            }
        }
        if metadata {
            let path = directory.join("test.toml");
            let metadata: HttpsMetadata = files::parse(
                &images["test.toml"].as_ref().expect("已捕获元数据").bytes,
                &path,
            )
            .map_err(|error| {
                format!("{error}；test.toml 必须且仅允许 ca_certificate 绝对文件路径字段")
            })?;
            capture_material(&mut materials, &metadata.ca_certificate, &path)?;
        }
        let captured = Self {
            directory: directory.to_owned(),
            app,
            selected: table,
            images,
            materials,
            redis_endpoint_explicit,
        };
        captured.verify()?;
        Ok(captured)
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        files::require_directory(&self.directory)?;
        for (name, expected) in &self.images {
            let path = self.directory.join(name);
            if files::read_optional(&path)? != *expected {
                return Err(diagnostic(
                    &path,
                    "复核捕获来源",
                    "文件存在性、字节或元数据已漂移；禁止重新捕获",
                ));
            }
        }
        for (path, expected) in &self.materials {
            if files::read_required(path)? != *expected {
                return Err(diagnostic(
                    path,
                    "复核TLS材料",
                    "证书或私钥身份/字节漂移；保留现场",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn derived_files(
        &self,
        selected: &toml::Table,
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        self.verify()?;
        let mut files = BTreeMap::new();
        for (name, image) in &self.images {
            if let Some(image) = image {
                files.insert(name.clone(), image.bytes.clone());
            }
        }
        let selected = toml::to_string(selected).map_err(|_| {
            diagnostic(
                &self.directory.join("app.test.toml"),
                "序列化快照",
                "TOML表无法序列化",
            )
        })?;
        files.insert("app.test.toml".into(), selected.into_bytes());
        Ok(files)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpsMetadata {
    ca_certificate: PathBuf,
}

fn capture_material(
    materials: &mut BTreeMap<PathBuf, Image>,
    path: &Path,
    context: &Path,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(diagnostic(
            context,
            "检查TLS文件字段",
            "CA/客户端证书/私钥必须是明确绝对文件路径",
        ));
    }
    materials.insert(path.to_owned(), files::read_required(path)?);
    Ok(())
}
