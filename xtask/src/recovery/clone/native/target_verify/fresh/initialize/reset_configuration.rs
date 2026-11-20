//! reset 专用私有配置：原 E 不变，只有维护来源、受控停机声明和状态目录可派生。
use super::super::super::super::{digest, json as document, resources::common};
use super::super::super::{creation_plan, guard::Inputs, input::request_configuration};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) struct Snapshot {
    pub(crate) directory: PathBuf,
    receipt: PathBuf,
    identity: (u64, u64),
    guard: Inputs,
}

impl Snapshot {
    pub(crate) fn prepare(
        backend: &Path,
        target: &Path,
        request: &Value,
        generation: &Value,
    ) -> Result<Self> {
        if generation["operator_declared_controlled_generation_only"] != true
            || generation["external_writers_discovered"] != false
        {
            return Err("reset 配置派生缺少 fresh 已核验的受控新目标与停机前置".into());
        }
        Self::load(backend, target, request, &generation["maintenance"], true)
    }

    pub(crate) fn open(
        backend: &Path,
        target: &Path,
        request: &Value,
        maintenance: &Value,
    ) -> Result<Self> {
        Self::load(backend, target, request, maintenance, false)
    }

    fn load(
        backend: &Path,
        target: &Path,
        request: &Value,
        maintenance: &Value,
        create: bool,
    ) -> Result<Self> {
        let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let root = execution_root(backend, request)?;
        let (base, files) = request_configuration::fresh_files(&root, &environment, request)?;
        if request_configuration::configuration_identity(&base, &files)?.as_str()
            != request["configuration_sha256"]
                .as_str()
                .ok_or("原 E 配置绑定缺失")?
        {
            return Err("reset 配置派生的原 E 与固定请求绑定不同".into());
        }
        let mut paths = files.keys().map(|name| base.join(name)).collect::<Vec<_>>();
        let source_guard = Inputs::open(paths.clone())?;
        let mut table = crate::rate_limit::load_app_table(&root, Some(&base), "test")
            .map_err(|_| "reset 原 E 无法安全解析；保留现场，禁止写入")?;
        derive(target, &mut table, maintenance)?;
        let bytes = toml::to_string(&table)
            .map_err(|_| "reset 快照无法安全序列化")?
            .into_bytes();
        let directory = directory(&root, target, request)?;
        let new = create && !directory.try_exists()?;
        if new {
            publish(&directory, &bytes)?;
        }
        validate_absolute_path(&directory, LocalTestPathKind::ExistingDirectory)?;
        let path = directory.join("app.test.toml");
        paths.push(path.clone());
        if fs::read(&path)? != bytes {
            return Err("reset 私有快照内容漂移或不属于唯一合法派生；禁止覆盖与重放".into());
        }
        let current = crate::rate_limit::load_app_table(&root, Some(&directory), "test")
            .map_err(|_| "reset 私有快照无法安全解析")?;
        let after = request_configuration::fresh_files(&root, &environment, request)?;
        if current != table || after.0 != base || after.1 != files {
            return Err("reset 快照或原 E 在核验期间变化".into());
        }
        creation_plan::reset_configuration(target, &serde_json::to_value(current)?, maintenance)?;
        let receipt = target.join("reset-configuration.json");
        let expected = json!({"format_version":1,"kind":"devex-clone-fresh-reset-configuration",
            "request_sha256":digest::plan_hash(request)?,"source_configuration_sha256":request["configuration_sha256"],
            "source_configuration":{"directory":base,"files":files},
            "snapshot":{"directory":directory,"file":common::binding(&path)?},
            "source_sha":maintenance["source"]["snapshot"]["head"],"services_stopped":true,
            "state_directory":target.join("reset-state")});
        if new {
            document::write_new(&receipt, &expected)?;
        }
        if document::read(&receipt)? != expected {
            return Err("reset 快照完成绑定缺失、漂移或不属于原 E；禁止修补部分发布".into());
        }
        paths.push(receipt.clone());
        let guard = Inputs::open(paths)?;
        let snapshot = Self {
            identity: common::device_inode_for(&directory, &fs::symlink_metadata(&directory)?)?,
            directory,
            receipt,
            guard,
        };
        if fs::read(&path)? != bytes || document::read(&snapshot.receipt)? != expected {
            return Err("reset 快照在取得完整保护句柄期间变化".into());
        }
        source_guard.check()?;
        snapshot.check()?;
        Ok(snapshot)
    }

    pub(crate) fn check(&self) -> Result<()> {
        validate_absolute_path(&self.directory, LocalTestPathKind::ExistingDirectory)?;
        if common::device_inode_for(&self.directory, &fs::symlink_metadata(&self.directory)?)?
            != self.identity
        {
            return Err("reset 私有配置目录身份变化；禁止继续写入".into());
        }
        let names = fs::read_dir(&self.directory)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        if names.len() != 1 || names[0] != "app.test.toml" {
            return Err("reset 私有快照不完整或含有未知文件；保留部分现场，禁止覆盖".into());
        }
        self.guard.check()
    }

    pub(crate) fn request(&self, original: &Value) -> Result<Value> {
        self.check()?;
        let mut request = original.clone();
        request["config_dir"] = json!(self.directory.to_str().ok_or("reset 配置路径不是 UTF-8")?);
        Ok(request)
    }

    pub(crate) fn binding(&self) -> Result<Value> {
        self.check()?;
        common::binding(&self.receipt)
    }
}

fn execution_root(backend: &Path, request: &Value) -> Result<PathBuf> {
    match request
        .get("execution_backend")
        .filter(|value| !value.is_null())
    {
        Some(value) => Ok(document::text(&value["path"], "目标执行树")?.into()),
        None => Ok(backend.to_owned()),
    }
}

fn directory(root: &Path, target: &Path, request: &Value) -> Result<PathBuf> {
    let local = root.join(".local-tests");
    validate_absolute_path(&local, LocalTestPathKind::ExistingDirectory)?;
    let key = digest::plan_hash(&json!({"target":target,"request":request}))?;
    let directory = local.join(format!("fresh-reset-config-{key}"));
    if directory.starts_with(target) || target.starts_with(&directory) {
        return Err("reset 明文快照必须位于公开目标观察树之外".into());
    }
    Ok(directory)
}

fn derive(target: &Path, table: &mut toml::Table, maintenance: &Value) -> Result<()> {
    let source = document::text(&maintenance["source"]["snapshot"]["head"], "维护来源")?;
    if !crate::frontend_origin::manifest::hex_digest(source, 40) {
        return Err("reset 快照必须绑定完整维护来源 SHA".into());
    }
    let reset = table
        .get_mut("reset")
        .and_then(toml::Value::as_table_mut)
        .ok_or("reset 原 E 缺少有效配置表")?;
    for (key, expected) in [
        ("source_sha", toml::Value::String(source.to_owned())),
        ("services_stopped", toml::Value::Boolean(true)),
        (
            "state_directory",
            toml::Value::String(
                target
                    .join("reset-state")
                    .to_str()
                    .ok_or("reset 状态路径不是 UTF-8")?
                    .into(),
            ),
        ),
    ] {
        if reset.get(key).is_some_and(|actual| actual != &expected) {
            return Err(format!("reset.{key} 的显式配置与已授权 fresh 派生冲突；禁止覆盖").into());
        }
        reset.insert(key.into(), expected);
    }
    Ok(())
}

fn publish(directory: &Path, bytes: &[u8]) -> Result<()> {
    crate::ci::databases::files::private_directory(directory)
        .map_err(|_| "reset 私有目录权限建立失败；不写明文，不继续写资源")?;
    // Windows 原子发布直接使用 Win32 路径，暂存文件与目标须使用同一个扩展长度目录。
    #[cfg(windows)]
    let native_directory = PathBuf::from(common::windows_path(directory)?);
    #[cfg(windows)]
    let directory = native_directory.as_path();
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    staged.write_all(bytes)?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    staged
        .persist_noclobber(directory.join("app.test.toml"))
        .map_err(|_| "reset 配置原子发布失败；保留部分目录，禁止覆盖或继续写资源")?;
    Ok(())
}
