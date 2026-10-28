//! 受管 Linux 只读协议；仅观察冻结文件、进程身份和互斥端口绑定。
use super::super::json as document;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
};

mod host;
#[cfg(target_os = "linux")]
mod kernel;
pub(super) use host::Observer;
pub(crate) use host::owned::Failure;
pub(super) use host::validate as validate_settings;
pub(super) const LIMIT: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FileProof {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageProof {
    pub path: String,
    pub executable: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub pid: u32,
    pub started: String,
    pub executable: String,
    pub boot_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub format_version: u8,
    pub uid: u32,
    pub program: String,
    pub sha256: String,
    pub source_root: PathBuf,
    pub source_commit: String,
    pub source_sha256: String,
    pub sources: Value,
    pub files: Vec<FileProof>,
    pub images: Vec<ImageProof>,
    pub identity: Option<Identity>,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub format_version: u8,
    pub uid: u32,
    pub program: String,
    pub sha256: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub boot_id: String,
    pub identity: Option<Identity>,
    pub alive: bool,
    pub bound_and_closed: bool,
}

pub(super) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.as_os_str() != "__linux-observer" {
        return None;
    }
    let mut input = Vec::new();
    let result = if args.next().is_some() {
        Err("观察器拒绝命令参数".into())
    } else {
        std::io::stdin()
            .lock()
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut input)
            .map_err(Into::into)
            .and_then(|_| execute(&input))
    };
    let (code, output) = match result {
        Ok(observation) => (0, json!({"ok":true,"result":observation})),
        Err(_) => (2, json!({"ok":false,"error":"LinuxObserverRejected"})),
    };
    Some(if writeln!(std::io::stdout().lock(), "{output}").is_ok() {
        code
    } else {
        2
    })
}

fn execute(input: &[u8]) -> Result<Observation> {
    if input.is_empty() || input.len() > LIMIT {
        return Err("观察器输入为空或超限".into());
    }
    let value = document::parse_fragment(std::str::from_utf8(input)?, "Linux 观察器")?;
    document::exact(
        &value,
        &[
            "format_version",
            "uid",
            "program",
            "sha256",
            "source_root",
            "source_commit",
            "source_sha256",
            "sources",
            "files",
            "images",
            "identity",
            "port",
        ],
        "Linux 观察器请求",
    )?;
    let request: Request = serde_json::from_value(value)?;
    if request.format_version != 1
        || request.port < 1024
        || request.files.len() > 128
        || request.images.len() > 4
    {
        return Err("观察器版本、端口或冻结项数量无效".into());
    }
    #[cfg(target_os = "linux")]
    {
        kernel::observe(&request)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("观察器仅允许 Linux 原生执行".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_duplicate_unknown_and_oversized_protocols_fail_without_echo() {
        for input in [
            b"{}".to_vec(),
            b"{\"uid\":1,\"uid\":1}".to_vec(),
            b"{\"secret\":\"private-value\"}".to_vec(),
            vec![b' '; LIMIT + 1],
            b"{} {}".to_vec(),
            vec![0xff],
        ] {
            assert!(execute(&input).is_err());
        }
    }
}
