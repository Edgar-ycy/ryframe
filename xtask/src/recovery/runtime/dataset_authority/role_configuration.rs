//! 从 fresh 配置捕获两角色的私有文件字节，并核验新发布的受管目录。

use super::reads;
use crate::{
    Result,
    ci::databases::files,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{
            digest,
            resources::common::device_inode_for,
            target_verify::input::request_configuration::{
                configuration_files, configuration_identity,
            },
        },
        monitoring::execution::ProtectedInputs,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::Path,
};

const COMMON: &str = "app.toml";
const TEST: &str = "app.test.toml";
const API: &str = "config";
const WORKER: &str = "worker-config";
const INSTANCE: &str = "snowflake_worker_id";

pub(in crate::recovery::runtime) struct RoleConfiguration {
    common: Option<Vec<u8>>,
    api: Vec<u8>,
    worker: Vec<u8>,
}

impl RoleConfiguration {
    pub(in crate::recovery::runtime) fn capture(
        source: &Path,
        expected: &str,
        worker_id: u16,
    ) -> Result<Self> {
        if worker_id > 1023 {
            return Err("恢复 Worker 实例编号必须在 0 到 1023 之间".into());
        }
        let root = source.parent().ok_or("fresh 配置缺少父目录")?;
        let before = directory_snapshot(source)?;
        let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let declared = json!(source);
        let snapshot = || configuration_files(root, &environment, Some(&declared));
        let selected = [COMMON, TEST]
            .into_iter()
            .filter(|name| before.members.contains(OsStr::new(name)));
        let guard = ProtectedInputs::open(selected.map(|name| source.join(name)))?;
        guard.finish((|| {
            let (directory, bindings) = snapshot()?;
            if configuration_identity(&directory, &bindings)? != expected {
                return Err("fresh 角色配置与已登记的应用配置身份不同".into());
            }
            let common = bindings
                .get(COMMON)
                .map(|binding| capture_file(&directory.join(COMMON), binding))
                .transpose()?;
            let api = capture_file(&directory.join(TEST), &bindings[TEST])?;
            let table = ryframe_config::AppConfig::read_document(
                &directory,
                ryframe_config::Environment::Test,
            )
            .map_err(|_| "fresh 角色 TOML 无法安全解析")?;
            let api_id = table
                .get(INSTANCE)
                .and_then(toml::Value::as_integer)
                .filter(|id| (0..=1023).contains(id))
                .ok_or("fresh API 实例编号必须在 0 到 1023 之间")?;
            if i64::from(worker_id) == api_id {
                return Err("恢复 API 与 Worker 实例编号必须不同".into());
            }
            let worker = render_worker(&api, worker_id)?;
            if snapshot()? != (directory, bindings) || directory_snapshot(source)? != before {
                return Err("fresh 配置在捕获期间变化".into());
            }
            Ok(Self {
                common,
                api,
                worker,
            })
        })())
    }

    pub(in crate::recovery::runtime) fn publish(&self, output: &Path) -> Result<()> {
        create_directory(output)?;
        let output_identity = directory_identity(output)?;
        for (name, bytes) in self.roles() {
            require_identity(output, output_identity)?;
            let directory = output.join(name);
            create_directory(&directory)?;
            let identity = directory_identity(&directory)?;
            for (filename, contents) in self.files(bytes) {
                require_identity(output, output_identity)?;
                require_identity(&directory, identity)?;
                write_new(&directory.join(filename), contents)?;
            }
            require_identity(&directory, identity)?;
        }
        require_identity(output, output_identity)?;
        self.verify(output)
    }

    pub(in crate::recovery::runtime) fn verify(&self, output: &Path) -> Result<()> {
        let before = directory_snapshot(output)?;
        before.require_members([API, WORKER])?;
        let mut roles = Vec::new();
        for (name, bytes) in self.roles() {
            let directory = output.join(name);
            let snapshot = directory_snapshot(&directory)?;
            let files = self
                .files(bytes)
                .map(|(filename, contents)| (directory.join(filename), contents))
                .collect::<Vec<_>>();
            snapshot.require_members(self.files(bytes).map(|(filename, _)| filename))?;
            roles.push((directory, snapshot, files));
        }
        let guard = ProtectedInputs::open(
            roles
                .iter()
                .flat_map(|(_, _, files)| files.iter().map(|(path, _)| path.clone())),
        )?;
        guard.finish((|| {
            for (_, _, files) in &roles {
                for (path, expected) in files {
                    if fs::read(path)? != *expected {
                        return Err("受管恢复角色配置的文件字节已变化".into());
                    }
                }
            }
            for (directory, snapshot, _) in roles {
                if directory_snapshot(&directory)? != snapshot {
                    return Err("受管恢复角色目录在核验期间变化".into());
                }
            }
            if directory_snapshot(output)? != before {
                return Err("受管恢复配置目录在核验期间变化".into());
            }
            Ok(())
        })())
    }

    fn roles(&self) -> [(&'static str, &[u8]); 2] {
        [(API, &self.api), (WORKER, &self.worker)]
    }

    fn files<'a>(&'a self, role: &'a [u8]) -> impl Iterator<Item = (&'static str, &'a [u8])> + 'a {
        self.common
            .as_deref()
            .map(|bytes| (COMMON, bytes))
            .into_iter()
            .chain([(TEST, role)])
    }
}

fn capture_file(path: &Path, binding: &Value) -> Result<Vec<u8>> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let bytes = fs::read(path)?;
    if binding["bytes"].as_u64() != Some(u64::try_from(bytes.len())?)
        || binding["sha256"].as_str() != Some(hex::encode(Sha256::digest(&bytes)).as_str())
    {
        return Err("fresh 配置文件字节与已核验的绑定不同".into());
    }
    Ok(bytes)
}

fn render_worker(api: &[u8], worker_id: u16) -> Result<Vec<u8>> {
    let source = std::str::from_utf8(api).map_err(|_| "fresh app.test.toml 必须为 UTF-8")?;
    let mut original: toml::Table =
        toml::from_str(source).map_err(|_| "fresh app.test.toml 无法安全解析")?;
    let text = source.replace("\r\n", "\n").replace('\r', "\n");
    let value = files::dispatch("instance-text", json!({"text":text,"worker_id":worker_id}))
        .map_err(|_| "无法安全改写恢复 Worker 实例编号")?;
    let text = value["text"]
        .as_str()
        .ok_or("恢复 Worker 配置改写缺少文本")?;
    let mut rewritten: toml::Table =
        toml::from_str(text).map_err(|_| "恢复 Worker app.test.toml 无法安全解析")?;
    original.remove(INSTANCE);
    let actual = rewritten
        .remove(INSTANCE)
        .and_then(|value| value.as_integer());
    if actual != Some(i64::from(worker_id)) || rewritten != original {
        return Err("恢复 Worker 配置改写改变了实例编号以外的语义".into());
    }
    if cfg!(windows) {
        Ok(text.replace('\n', "\r\n").into_bytes())
    } else {
        Ok(text.as_bytes().to_vec())
    }
}

#[derive(PartialEq, Eq)]
struct DirectorySnapshot {
    identity: (u64, u64),
    members: BTreeSet<OsString>,
}

impl DirectorySnapshot {
    fn require_members<'a>(&self, expected: impl IntoIterator<Item = &'a str>) -> Result<()> {
        if self.members != expected.into_iter().map(OsString::from).collect() {
            return Err("受管恢复角色目录包含缺失或多余成员".into());
        }
        Ok(())
    }
}

fn directory_snapshot(path: &Path) -> Result<DirectorySnapshot> {
    let identity = directory_identity(path)?;
    let mut members = BTreeSet::new();
    for entry in reads::entries(path)? {
        let entry = entry?;
        if digest::linked(&fs::symlink_metadata(entry.path())?) {
            return Err("恢复角色配置目录成员不得经过链接".into());
        }
        members.insert(entry.file_name());
    }
    require_identity(path, identity)?;
    Ok(DirectorySnapshot { identity, members })
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn require_identity(path: &Path, expected: (u64, u64)) -> Result<()> {
    if directory_identity(path)? != expected {
        return Err("恢复角色配置目录身份发生变化".into());
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<()> {
    validate_absolute_path(path, LocalTestPathKind::NewDirectory)?;
    files::private_directory(path).map_err(|_| "无法创建私有恢复角色配置目录；保留原现场".into())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    validate_absolute_path(path, LocalTestPathKind::OutputFile)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut file = options.open(path)?;
    let identity = device_inode_for(path, &file.metadata()?)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    if device_inode_for(path, &fs::symlink_metadata(path)?)? != identity {
        return Err("新建恢复角色配置文件身份发生变化；保留原现场".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/dataset_authority/role_configuration.rs"]
mod tests;
