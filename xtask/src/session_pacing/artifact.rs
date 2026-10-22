use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use same_file::Handle;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::error;
use crate::{devex::runtime_configuration, http_session::SessionError};

pub(crate) struct Artifacts {
    path: PathBuf,
    ancestors: Vec<(PathBuf, Handle)>,
}

impl Artifacts {
    pub(crate) fn new(path: &Path) -> Result<Self, SessionError> {
        let path = std::path::absolute(path).map_err(|_| error("pacing-artifact"))?;
        let mut ancestors: Vec<_> = path.ancestors().map(Path::to_path_buf).collect();
        ancestors.reverse();
        let mut guards = Vec::new();
        for entry in ancestors {
            match fs::symlink_metadata(&entry) {
                Ok(metadata) => directory(&metadata)?,
                Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&entry).map_err(|_| error("pacing-artifact"))?;
                    directory(
                        &fs::symlink_metadata(&entry).map_err(|_| error("pacing-artifact"))?,
                    )?;
                }
                Err(_) => return Err(error("pacing-artifact")),
            }
            let handle = Handle::from_path(&entry).map_err(|_| error("pacing-artifact"))?;
            guards.push((entry, handle));
        }
        let result = Self {
            path,
            ancestors: guards,
        };
        result.check()?;
        Ok(result)
    }

    fn check(&self) -> Result<(), SessionError> {
        for (path, handle) in &self.ancestors {
            directory(&fs::symlink_metadata(path).map_err(|_| error("pacing-artifact"))?)?;
            if *handle != Handle::from_path(path).map_err(|_| error("pacing-artifact"))? {
                return Err(error("pacing-artifact"));
            }
        }
        Ok(())
    }

    pub(crate) fn write(&self, name: &str, value: &Value) -> Result<(), SessionError> {
        if !matches!(name, "pacing-authority.json" | "pacing-summary.json") {
            return Err(error("pacing-artifact"));
        }
        let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| error("pacing-artifact"))?;
        bytes.push(b'\n');
        self.check()?;
        let path = self.path.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| error("pacing-artifact"))?;
        let handle = Handle::from_file(file.try_clone().map_err(|_| error("pacing-artifact"))?)
            .map_err(|_| error("pacing-artifact"))?;
        self.check()?;
        file.write_all(&bytes)
            .map_err(|_| error("pacing-artifact"))?;
        file.sync_all().map_err(|_| error("pacing-artifact"))?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| error("pacing-artifact"))?;
        if !metadata.is_file()
            || linked(&metadata)
            || handle != Handle::from_path(&path).map_err(|_| error("pacing-artifact"))?
        {
            return Err(error("pacing-artifact"));
        }
        self.check()
    }
}

pub(crate) fn catalog(root: &Path) -> Result<BTreeMap<String, Value>, SessionError> {
    let path = root.join("openapi/openapi.json");
    for ancestor in path.ancestors().skip(1) {
        directory(&fs::symlink_metadata(ancestor).map_err(|_| error("pacing-catalog"))?)?;
    }
    let metadata = fs::symlink_metadata(&path).map_err(|_| error("pacing-catalog"))?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() > 16 * 1024 * 1024 {
        return Err(error("pacing-catalog"));
    }
    let file = File::open(&path).map_err(|_| error("pacing-catalog"))?;
    let handle = Handle::from_file(file.try_clone().map_err(|_| error("pacing-catalog"))?)
        .map_err(|_| error("pacing-catalog"))?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("pacing-catalog"))?;
    let after = fs::symlink_metadata(&path).map_err(|_| error("pacing-catalog"))?;
    if bytes.len() > 16 * 1024 * 1024
        || !after.is_file()
        || linked(&after)
        || handle != Handle::from_path(&path).map_err(|_| error("pacing-catalog"))?
    {
        return Err(error("pacing-catalog"));
    }
    let document = serde_json::from_slice(&bytes).map_err(|_| error("pacing-catalog"))?;
    runtime_configuration::operation_catalog(&document).map_err(|_| error("pacing-catalog"))
}

pub(crate) fn digest(value: &Value) -> Result<String, SessionError> {
    let bytes = serde_json::to_vec(&canonical(value)).map_err(|_| error("pacing-authority"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        Value::Object(values) => {
            let ordered: BTreeMap<_, _> = values
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect();
            Value::Object(ordered.into_iter().collect())
        }
        value => value.clone(),
    }
}

pub(crate) fn authority_receipt<T>(
    prepared: &Result<T, SessionError>,
    duration: u64,
    authority: Option<&Value>,
    actual: Option<&str>,
    expected: &Value,
) -> Value {
    let mut value = json!({"format_version": 1, "success": prepared.is_ok(), "duration_ms": super::model::milliseconds(duration)});
    if let Err(failure) = prepared {
        value["failure"] = json!(failure.category);
    }
    if let Some(authority) = authority {
        value["authority"] = authority.clone();
    }
    if let Some(actual) = actual {
        value["actual_sha256"] = json!(actual);
    }
    if expected.as_str().is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        value["expected_sha256"] = expected.clone();
    }
    value
}

fn directory(metadata: &Metadata) -> Result<(), SessionError> {
    if !metadata.is_dir() || linked(metadata) {
        return Err(error("pacing-artifact"));
    }
    Ok(())
}

#[cfg(windows)]
fn linked(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn linked(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}
