use super::Checked;
use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

pub(super) fn dispatch(operation: &str, value: serde_json::Value) -> Checked<serde_json::Value> {
    use serde::Deserialize;
    use serde_json::json;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Directory {
        path: std::path::PathBuf,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Instance {
        text: String,
        worker_id: u64,
    }
    match operation {
        "private-directory" => {
            let request: Directory =
                serde_json::from_value(value).map_err(|_| "protocol_request")?;
            validate_absolute_path(&request.path, LocalTestPathKind::NewDirectory)
                .map_err(|_| "config_path")?;
            private_directory(&request.path)?;
            Ok(json!({"state":"created"}))
        }
        "instance-text" => {
            let request: Instance =
                serde_json::from_value(value).map_err(|_| "protocol_request")?;
            if request.worker_id > 1023 {
                return Err("instance_id");
            }
            let _: toml::Table = toml::from_str(&request.text).map_err(|_| "config_invalid")?;
            Ok(json!({"text":super::prepare::instance_text(&request.text, request.worker_id)?}))
        }
        _ => Err("protocol_operation"),
    }
}

pub(super) fn read(path: &Path, limit: usize) -> Checked<Vec<u8>> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile).map_err(|_| "config_path")?;
    let file = fs::File::open(path).map_err(|_| "config_read")?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "config_read")?;
    if bytes.len() > limit {
        return Err("config_limit");
    }
    Ok(bytes)
}

pub(crate) fn private_directory(path: &Path) -> Checked<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder
            .mode(0o700)
            .create(path)
            .map_err(|_| "config_directory")?;
    }
    #[cfg(not(unix))]
    builder.create(path).map_err(|_| "config_directory")?;
    #[cfg(windows)]
    restrict_acl(path)?;
    Ok(())
}

#[cfg(windows)]
fn restrict_acl(path: &Path) -> Checked<()> {
    use std::process::{Command, Stdio};
    let mut identity = Command::new("whoami");
    identity.args(["/user", "/fo", "csv", "/nh"]);
    super::client::environment(&mut identity);
    let identity = identity.output().map_err(|_| "config_acl")?;
    if !identity.status.success() {
        return Err("config_acl");
    }
    let text = String::from_utf8(identity.stdout).map_err(|_| "config_acl")?;
    let pattern = regex::Regex::new(r"S-1-[0-9-]+").map_err(|_| "config_acl")?;
    let sid = pattern.find(&text).ok_or("config_acl")?.as_str();
    let mut command = Command::new("icacls");
    command
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:(OI)(CI)F")])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    super::client::environment(&mut command);
    let group = crate::process::ChildGroup::new().map_err(|_| "config_acl")?;
    if !group
        .spawn(command)
        .map_err(|_| "config_acl")?
        .wait()
        .map_err(|_| "config_acl")?
        .success()
    {
        return Err("config_acl");
    }
    Ok(())
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Checked<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| "config_write")?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "config_write")
}

pub(super) fn replace_receipt(path: &Path, value: &serde_json::Value) -> Checked<()> {
    let parent = path.parent().ok_or("receipt_write")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| "receipt_write")?;
    let bytes = serde_json::to_vec(value).map_err(|_| "receipt_write")?;
    file.write_all(&bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "receipt_write")?;
    file.persist(path).map_err(|_| "receipt_write")?;
    Ok(())
}
