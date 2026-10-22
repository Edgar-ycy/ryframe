use ryframe_config::{AppConfig, Environment, ResourceScopeId};
use sha2::{Digest, Sha256};
use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

// 工具和测试共同使用同一文件捕获/发布实现；不进入产品运行时。
#[path = "../../../../xtask/src/ci/integration_configuration/capture.rs"]
mod capture;
#[path = "../../../../xtask/src/ci/integration_configuration/files.rs"]
mod files;
#[path = "../../../../xtask/src/ci/integration_configuration/owned.rs"]
mod owned;

use capture::Captured;
use files::diagnostic;
use owned::OwnedConfiguration;

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub fn backend_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("architecture/crate-boundaries.toml").is_file())
        .expect("必须从后端 workspace 运行集成测试")
        .to_owned()
}

pub fn enabled(role: &str) -> bool {
    let path = backend_root()
        .join(".local-tests/service-tests")
        .join(role)
        .join("config");
    let exists = files::directory_available(&path).unwrap_or_else(|error| panic!("{error}"));
    if !exists {
        eprintln!(
            "跳过 {role} 真实协议测试：显式配置目录 {} 不存在，真实链路未验收",
            path.display()
        );
    }
    exists
}

pub struct Configuration {
    pub app: AppConfig,
    captured: Captured,
    owned: Option<OwnedConfiguration>,
    container: Option<tempfile::TempDir>,
}

impl Configuration {
    pub fn load(role: &str, case: &str) -> Self {
        let root = backend_root();
        let source = root
            .join(".local-tests/service-tests")
            .join(role)
            .join("config");
        Self::from_directory(&root, &source, case, role.ends_with("https"))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn from_directory(
        root: &Path,
        source: &Path,
        case: &str,
        metadata: bool,
    ) -> Result<Self, String> {
        let captured = Captured::load(source, metadata)?;
        if captured.app.redis.is_some() && !captured.redis_endpoint_explicit {
            return Err(diagnostic(
                source,
                "检查redis.host/redis.port",
                "Redis测试配置必须显式给出host和port，不采用默认端点",
            ));
        }
        let mut selected = captured.selected.clone();
        selected.insert(
            "scope_id".into(),
            toml::Value::String(unique_scope(captured.app.scope_id.as_str(), case)?),
        );
        let content = captured.derived_files(&selected)?;
        let parent = root.join(".local-tests/service-tests/runtime");
        files::create_parents(&parent)?;
        let container = tempfile::Builder::new()
            .prefix("owned-")
            .tempdir_in(&parent)
            .map_err(|error| files::io_error(&parent, "创建独占临时目录", error))?;
        let directory = container.path().join("config");
        let owned = match OwnedConfiguration::create(&directory, &content) {
            Ok(owned) => owned,
            Err(error) => {
                let _retained = container.keep();
                return Err(error);
            }
        };
        let loaded = AppConfig::load(&directory, Environment::Test)
            .map_err(|error| error.message().to_owned());
        match loaded.and_then(|app| {
            captured.verify()?;
            owned.verify()?;
            Ok(app)
        }) {
            Ok(app) => {
                let result = Self {
                    app,
                    captured,
                    owned: Some(owned),
                    container: Some(container),
                };
                if metadata {
                    result.ca()?;
                }
                Ok(result)
            }
            Err(error) => {
                let _retained = container.keep();
                Err(error)
            }
        }
    }

    pub fn ca(&self) -> Result<Vec<u8>, String> {
        self.verify()?;
        let directory = &self.owned.as_ref().expect("存活快照").directory;
        let path = directory.join("test.toml");
        let metadata: HttpsMetadata = files::parse(&files::read_required(&path)?.bytes, &path)?;
        if !metadata.ca_certificate.is_absolute() {
            return Err(diagnostic(
                &path,
                "检查test.ca_certificate",
                "CA必须是明确绝对文件路径",
            ));
        }
        self.captured
            .materials
            .get(&metadata.ca_certificate)
            .map(|image| image.bytes.clone())
            .ok_or_else(|| diagnostic(&path, "检查test.ca_certificate", "CA不属于本次捕获材料"))
    }

    pub fn verify(&self) -> Result<(), String> {
        self.captured.verify()?;
        let parent = self.container.as_ref().expect("存活容器").path();
        files::require_directory(parent)?;
        let entries =
            fs::read_dir(parent).map_err(|error| files::io_error(parent, "检查快照容器", error))?;
        let mut count = 0;
        for entry in entries {
            let entry = entry.map_err(|error| files::io_error(parent, "检查快照容器项", error))?;
            if entry.file_name() != "config" {
                return Err(diagnostic(
                    &entry.path(),
                    "检查快照容器",
                    "发现外来内容，保留整个容器",
                ));
            }
            count += 1;
        }
        if count != 1 {
            return Err(diagnostic(
                parent,
                "检查快照容器",
                "预期唯一config目录，保留现场",
            ));
        }
        self.owned.as_ref().expect("存活快照").verify()
    }

    fn clean(&mut self) -> Result<(), String> {
        self.verify()?;
        self.owned.take().expect("存活快照").remove()?;
        if let Some(container) = self.container.take() {
            // 不递归删除；容器出现任何外来内容时保留。
            let path = container.keep();
            fs::remove_dir(&path)
                .map_err(|error| files::io_error(&path, "删除已知空容器", error))?;
        }
        Ok(())
    }
}

impl Drop for Configuration {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Some(container) = self.container.take() {
                eprintln!(
                    "测试失败，保留配置快照 {}，不自动重放",
                    container.path().display()
                );
                let _retained = container.keep();
            }
            return;
        }
        if let Err(error) = self.clean() {
            if let Some(container) = self.container.take() {
                let _retained = container.keep();
            }
            panic!("配置快照清理失败：{error}");
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpsMetadata {
    ca_certificate: PathBuf,
}

fn unique_scope(scope: &str, case: &str) -> Result<String, String> {
    let digest = hex::encode(Sha256::digest(format!("{scope}/{case}").as_bytes()));
    let name: String = case
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(6)
        .collect();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let derived = format!(
        "{}-{}-{}-{:x}-{sequence:x}",
        &scope[..scope.len().min(4)],
        &digest[..8],
        name,
        std::process::id()
    );
    ResourceScopeId::parse(derived).map(|scope| scope.as_str().to_owned())
}

// 客户端字段先销毁；快照始终保留至该客户端及测试清理完成。
pub struct ScopedClient<T> {
    pub client: T,
    pub configuration: Configuration,
}

impl<T> Deref for ScopedClient<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.client
    }
}

impl<T> Drop for ScopedClient<T> {
    fn drop(&mut self) {
        if let Err(error) = self.configuration.verify() {
            if std::thread::panicking() {
                eprintln!("客户端结束复核失败：{error}");
            } else {
                panic!("客户端结束复核失败：{error}");
            }
        }
    }
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;
