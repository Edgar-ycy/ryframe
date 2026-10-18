use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as json_io},
        monitoring::contract::{STAGING_DIRECTORY, STAGING_MANIFEST, TOKEN_NAME, exact},
    },
};

const MAX_TOKEN_BYTES: u64 = 8192;
const TOOL_NAMES: [&str; 4] = ["prometheus", "promtool", "alertmanager", "amtool"];
const RULES: [(&str, &str); 2] = [
    ("alerts", "deploy/prometheus/ryframe-alerts.yml"),
    ("tests", "deploy/prometheus/ryframe-alerts.test.yml"),
];

#[derive(Clone)]
pub(crate) struct Credential {
    pub(crate) secret: String,
    pub(crate) absolute: Value,
    pub(crate) relative: Value,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("secret", &"[已隐藏]")
            .field("absolute", &self.absolute)
            .field("relative", &self.relative)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Staged {
    pub(crate) manifest: Value,
    pub(crate) files: BTreeMap<String, PathBuf>,
}

pub(crate) fn read_credential(path: &Path) -> Result<Credential> {
    read_credential_with_security(path, super::permissions::security_receipt)
}

pub(crate) fn read_credential_with_security(
    path: &Path,
    security_reader: impl Fn(&Path) -> Result<Value>,
) -> Result<Credential> {
    if path.file_name().and_then(|value| value.to_str()) != Some(TOKEN_NAME) {
        return Err("监控凭据必须命名为 metrics-token.txt".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // 凭据读取期间允许读取，阻止写入、删除及同字节替换。
        options.share_mode(1);
    }
    let mut stream = options.open(path)?;
    let before = fs::symlink_metadata(path)?;
    if digest::linked(&before) || !before.is_file() {
        return Err("监控凭据必须是普通文件".into());
    }
    if before.len() == 0 || before.len() > MAX_TOKEN_BYTES {
        return Err("监控凭据文件大小无效".into());
    }
    let identity =
        crate::recovery::clone::native::resources::common::device_inode_for(path, &before)?;
    let opened = crate::recovery::clone::native::resources::common::device_inode_for(
        path,
        &stream.metadata()?,
    )?;
    if identity != opened {
        return Err("监控凭据在打开期间被替换".into());
    }
    let security = security_reader(path)?;
    let mut first = Vec::new();
    stream
        .by_ref()
        .take(MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut first)?;
    stream.seek(SeekFrom::Start(0))?;
    let mut second = Vec::new();
    stream
        .by_ref()
        .take(MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut second)?;
    if first != second || first.len() as u64 != before.len() {
        return Err("监控凭据在读取期间发生变化".into());
    }
    if security_reader(path)? != security {
        return Err("监控凭据权限在读取期间发生变化".into());
    }
    let secret = std::str::from_utf8(&first)
        .map_err(|_| "监控凭据不是 UTF-8")?
        .trim()
        .to_owned();
    if secret.is_empty() || secret.contains('\0') {
        return Err("监控凭据内容无效".into());
    }
    let digest = digest::file(path)?;
    if digest.bytes != first.len() as u64 || digest.sha256 != hex::encode(Sha256::digest(&first)) {
        return Err("监控凭据路径与已读取文件的字节不同".into());
    }
    let (device, inode) = crate::recovery::clone::native::resources::common::device_inode_for(
        path,
        &fs::metadata(path)?,
    )?;
    if identity != (device, inode) {
        return Err("监控凭据在读取期间被同字节替换".into());
    }
    let security_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&security)?.as_slice()));
    let common = json!({
        "bytes": digest.bytes,
        "sha256": digest.sha256,
        "device": device,
        "inode": inode,
        "security_sha256": security_sha256,
    });
    let mut absolute = common.clone();
    absolute["path"] = json!(path.to_string_lossy());
    let mut relative = common;
    relative["path"] = json!(TOKEN_NAME);
    Ok(Credential {
        secret,
        absolute,
        relative,
    })
}

pub(crate) fn create(
    backend: &Path,
    run: &Path,
    coordinator: &Value,
    credential: &Credential,
    tools: &BTreeMap<String, PathBuf>,
    versions: &BTreeMap<String, String>,
) -> Result<Value> {
    let staging = run.join(STAGING_DIRECTORY);
    fs::create_dir(&staging).map_err(|error| format!("无法创建监控 staging：{error}"))?;
    let mut sources = BTreeMap::new();
    for (_, relative) in RULES {
        sources.insert(relative.to_owned(), backend.join(relative));
    }
    for (name, path) in tools {
        sources.insert(tool_relative(name), path.clone());
    }
    let mut files = Vec::new();
    for (relative, source) in sources {
        let target = staging.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut reader = fs::File::open(&source)?;
        let mut writer = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        std::io::copy(&mut reader, &mut writer)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            writer.set_permissions(fs::Permissions::from_mode(
                if relative.starts_with("tools/") {
                    0o700
                } else {
                    0o600
                },
            ))?;
        }
        writer.sync_all()?;
        files.push(file_receipt(&target, &relative)?);
    }
    files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = json!({
        "format_version": 1,
        "kind": "restore-monitoring-input-staging",
        "coordinator": coordinator,
        "files": files,
        "credential": credential.relative,
        "tool_versions": versions,
    });
    json_io::write_new(&staging.join(STAGING_MANIFEST), &manifest)?;
    descriptor(&staging.join(STAGING_MANIFEST))
}

pub(crate) fn verify(
    run: &Path,
    manifest_receipt: &Value,
    coordinator: &Value,
    credential: &Credential,
    tools: &Map<String, Value>,
) -> Result<Staged> {
    verify_impl(run, manifest_receipt, coordinator, credential, Some(tools))
}

pub(crate) fn verify_without_tools(
    run: &Path,
    manifest_receipt: &Value,
    coordinator: &Value,
    credential: &Credential,
) -> Result<Staged> {
    verify_impl(run, manifest_receipt, coordinator, credential, None)
}

fn verify_impl(
    run: &Path,
    manifest_receipt: &Value,
    coordinator: &Value,
    credential: &Credential,
    tools: Option<&Map<String, Value>>,
) -> Result<Staged> {
    let root = run.join(STAGING_DIRECTORY);
    let manifest_path = root.join(STAGING_MANIFEST);
    if descriptor(&manifest_path)? != *manifest_receipt {
        return Err("监控 staging 清单摘要已经变化".into());
    }
    let manifest = json_io::read(&manifest_path)?;
    let object = exact(
        &manifest,
        &[
            "format_version",
            "kind",
            "coordinator",
            "files",
            "credential",
            "tool_versions",
        ],
        "监控 staging 清单",
    )?;
    if object.get("format_version").and_then(Value::as_u64) != Some(1)
        || object.get("kind").and_then(Value::as_str) != Some("restore-monitoring-input-staging")
        || object.get("coordinator") != Some(coordinator)
        || object.get("credential") != Some(&credential.relative)
    {
        return Err("监控 staging 版本、协调来源或凭据不同".into());
    }
    let files = object
        .get("files")
        .and_then(Value::as_array)
        .ok_or("监控 staging 文件清单无效")?;
    let mut paths = BTreeSet::new();
    let mut result = BTreeMap::new();
    let mut previous = None::<String>;
    for file in files {
        let file = exact(
            file,
            &["path", "bytes", "sha256", "device", "inode"],
            "监控 staging 文件",
        )?;
        let relative = file
            .get("path")
            .and_then(Value::as_str)
            .ok_or("监控 staging 文件路径无效")?;
        validate_relative(relative)?;
        if previous.as_deref().is_some_and(|value| relative <= value)
            || !paths.insert(relative.to_owned())
        {
            return Err("监控 staging 文件清单必须有序且唯一".into());
        }
        previous = Some(relative.to_owned());
        let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let file = Value::Object(file.clone());
        if file_receipt(&path, relative)? != file {
            return Err("监控 staging 文件摘要或身份不同".into());
        }
        result.insert(relative.to_owned(), path);
    }
    let expected = RULES
        .iter()
        .map(|(_, path)| (*path).to_owned())
        .chain(TOOL_NAMES.iter().map(|name| tool_relative(name)))
        .collect::<BTreeSet<_>>();
    if paths != expected {
        return Err("监控 staging 缺少固定规则或工具".into());
    }
    if let Some(tools) = tools {
        for name in TOOL_NAMES {
            let relative = tool_relative(name);
            let expected_path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            let item = tools.get(name).ok_or("监控绑定缺少固定工具")?;
            if item.get("path").and_then(Value::as_str)
                != Some(expected_path.to_string_lossy().as_ref())
            {
                return Err(format!("监控工具 {name} 不属于当前 staging").into());
            }
        }
    }
    exact_tree(&root, &paths)?;
    Ok(Staged {
        manifest,
        files: result,
    })
}

pub(crate) fn file_receipt(path: &Path, relative: &str) -> Result<Value> {
    let metadata = fs::symlink_metadata(path)?;
    if digest::linked(&metadata) || !metadata.is_file() {
        return Err("监控 staging 成员必须是普通文件".into());
    }
    let digest = digest::file(path)?;
    let (device, inode) =
        crate::recovery::clone::native::resources::common::device_inode_for(path, &metadata)?;
    Ok(json!({
        "path": relative,
        "bytes": digest.bytes,
        "sha256": digest.sha256,
        "device": device,
        "inode": inode,
    }))
}

pub(crate) fn descriptor(path: &Path) -> Result<Value> {
    let digest = digest::file(path)?;
    Ok(crate::recovery::monitoring::contract::descriptor(
        path,
        digest.bytes,
        digest.sha256,
    ))
}

pub(crate) fn tool_relative(name: &str) -> String {
    format!("tools/{name}{}", std::env::consts::EXE_SUFFIX)
}

pub(crate) fn rules() -> [(&'static str, &'static str); 2] {
    RULES
}

fn exact_tree(root: &Path, files: &BTreeSet<String>) -> Result<()> {
    let mut expected_directories = BTreeSet::from([String::new()]);
    for path in files {
        let parent = Path::new(path).parent().unwrap_or(Path::new(""));
        let mut current = parent.to_path_buf();
        loop {
            expected_directories.insert(current.to_string_lossy().replace('\\', "/"));
            let Some(parent) = current.parent() else {
                break;
            };
            current = parent.to_path_buf();
        }
    }
    let mut observed_files = BTreeSet::new();
    let mut observed_directories = BTreeSet::new();
    walk(root, root, &mut observed_files, &mut observed_directories)?;
    expected_directories.insert(String::new());
    if observed_files != *files || observed_directories != expected_directories {
        return Err(format!(
            "监控 staging 包含缺失或未知成员：files={observed_files:?}/{files:?}，directories={observed_directories:?}/{expected_directories:?}"
        )
        .into());
    }
    Ok(())
}

fn walk(
    root: &Path,
    current: &Path,
    files: &mut BTreeSet<String>,
    directories: &mut BTreeSet<String>,
) -> Result<()> {
    let relative = current
        .strip_prefix(root)
        .map_err(|_| "监控 staging 路径越界")?
        .to_string_lossy()
        .replace('\\', "/");
    directories.insert(relative.clone());
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if digest::linked(&metadata) {
            return Err("监控 staging 不得包含链接或重解析点".into());
        }
        if metadata.is_dir() {
            walk(root, &entry.path(), files, directories)?;
        } else if metadata.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| "监控 staging 路径越界")?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == STAGING_MANIFEST {
                continue;
            }
            files.insert(relative);
        } else {
            return Err("监控 staging 包含非普通成员".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_relative(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || value.contains(['\\', ':'])
        || value.chars().any(char::is_control)
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::CurDir | Component::RootDir
            )
        })
    {
        return Err("监控 staging 相对路径无效".into());
    }
    Ok(())
}
