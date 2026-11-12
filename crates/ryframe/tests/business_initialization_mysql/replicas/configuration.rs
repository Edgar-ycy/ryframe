use ryframe_config::{AppConfig, DatabaseReplicaConfig, DbConnection, DbTlsMode, Environment};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(super) const REPLICA: &str = "fixture-read-replica";

type FileImage = (PathBuf, Option<Vec<u8>>);

/// 冻结本用例实际读取的配置及 TLS 文件；缺失的公共配置也参与前后像。
pub(super) struct Configuration {
    pub(super) app: AppConfig,
    images: Vec<FileImage>,
}

impl Configuration {
    pub(super) fn load() -> Result<Self, String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|p| p.join("architecture/crate-boundaries.toml").is_file())
            .ok_or("无法定位副本测试的 backend")?;
        let directory = root.join(".local-tests/mysql/config");
        let images = ["app.toml", "app.test.toml"]
            .into_iter()
            .map(|name| {
                let path = directory.join(name);
                Ok((path.clone(), image(&path, name == "app.toml")?))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let app = AppConfig::load(&directory, Environment::Test)
            .map_err(|e| format!("副本测试文件配置无效：{}", e.message()))?;
        if !matches!(
            app.database.primary.host.as_str(),
            "localhost" | "127.0.0.1" | "::1"
        ) {
            return Err("副本测试只接受原配置中的回环 MySQL；未创建代理或修改主机名".into());
        }
        if app.database.primary.tls_mode == DbTlsMode::Disabled {
            return Err("副本真实场景必须保留启用的 MySQL TLS".into());
        }
        let mut result = Self { app, images };
        result
            .images
            .extend(tls_images(&result.app.database.primary)?);
        if let Some(replica) = result.physical() {
            let images = tls_images(replica)?;
            result.images.extend(images);
        }
        result.verify()?;
        Ok(result)
    }

    pub(super) fn derive(
        &self,
        primary: &str,
        replica: &str,
        port: u16,
    ) -> Result<AppConfig, String> {
        self.verify()?;
        if primary == replica || port == self.app.database.primary.port {
            return Err("副本测试的派生 schema 或代理端口不独立".into());
        }
        let mut read = self.app.database.primary.clone();
        read.database = replica.into();
        self.with_proxy(primary, read, port)
    }

    pub(super) fn physical(&self) -> Option<&DbConnection> {
        self.app
            .database
            .replicas
            .iter()
            .find(|replica| replica.name == "physical-replica")
            .map(|replica| &replica.connection)
    }

    pub(super) fn with_proxy(
        &self,
        primary: &str,
        mut read: DbConnection,
        port: u16,
    ) -> Result<AppConfig, String> {
        self.verify()?;
        let mut app = self.app.clone();
        app.database.primary.database = primary.into();
        read.port = port;
        // 本用例仅接入两个 owned schema，不访问原配置中的其他拓扑资源。
        app.database.sources.clear();
        app.database.replicas = vec![DatabaseReplicaConfig {
            name: REPLICA.into(),
            connection: read,
        }];
        app.validate()
            .map_err(|e| format!("派生副本配置无效：{}", e.message()))?;
        Ok(app)
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        for (path, expected) in &self.images {
            if image(path, expected.is_none())? != *expected {
                return Err(
                    "副本测试配置或 TLS 文件前后 SHA 不一致；不继续连接、修改或重放".into(),
                );
            }
        }
        Ok(())
    }
}

fn tls_images(connection: &DbConnection) -> Result<Vec<FileImage>, String> {
    [
        &connection.tls_ca,
        &connection.tls_client_cert,
        &connection.tls_client_key,
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.trim().is_empty())
    .map(|configured| {
        // TLS 相对路径仍以实际进程 cwd 解析，与驱动读取规则一致。
        let path = PathBuf::from(configured);
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|_| "无法读取 TLS 文件的当前目录")?
                .join(path)
        };
        let digest = image(&path, false)?;
        Ok((path, digest))
    })
    .collect()
}

fn image(path: &Path, missing_allowed: bool) -> Result<Option<Vec<u8>>, String> {
    for parent in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(parent).map_err(|_| "无法核对副本配置/TLS 的父目录")?;
        if !ordinary(&metadata) || !metadata.is_dir() {
            return Err("副本配置/TLS 路径不得经过链接或重解析点".into());
        }
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if missing_allowed && error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(_) => return Err("副本配置/TLS 文件缺失或不可读取".into()),
    };
    if !ordinary(&metadata) || !metadata.is_file() {
        return Err("副本配置/TLS 必须是普通无链接文件".into());
    }
    let bytes = fs::read(path).map_err(|_| "无法读取副本配置/TLS 文件")?;
    let after = fs::symlink_metadata(path).map_err(|_| "副本配置/TLS 文件读取期间消失")?;
    if !ordinary(&after) || !after.is_file() {
        return Err("副本配置/TLS 文件读取期间被替换为链接".into());
    }
    Ok(Some(Sha256::digest(bytes).to_vec()))
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}
