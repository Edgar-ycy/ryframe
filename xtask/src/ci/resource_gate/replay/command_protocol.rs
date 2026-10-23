use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::io::Read;

const MAX_REQUEST_BYTES: u64 = 16 * 1024;

/// replay 驱动通过独立 stdin 接收上下文，不向外部命令追加门禁参数。
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayCommandRequest {
    format_version: u32,
    pub(crate) target_root: PathBuf,
}

impl ReplayCommandRequest {
    pub(crate) fn new(target_root: PathBuf) -> Self {
        Self {
            format_version: 1,
            target_root,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != 1 || !self.target_root.is_absolute() {
            return Err("replay 请求版本须为 1 且 target_root 须为绝对路径".to_owned());
        }
        Ok(())
    }

    pub(crate) fn input_file(&self, directory: &Path) -> Result<File, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("无法编码 replay JSON 请求：{error}"))?;
        if bytes.len() as u64 > MAX_REQUEST_BYTES {
            return Err("replay JSON 请求超过 16 KiB；请缩短 target_root 路径".to_owned());
        }
        // 匿名文件在命令返回或取消后自动释放，不留下可被下一案例误读的请求。
        let mut file = tempfile::tempfile_in(directory)
            .map_err(|error| format!("无法创建 replay stdin 请求文件：{error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|error| format!("无法写入或复位 replay stdin 请求：{error}"))?;
        Ok(file)
    }

    #[cfg(test)]
    pub(crate) fn read(reader: impl std::io::Read) -> Result<Self, String> {
        let mut bytes = Vec::new();
        reader
            .take(MAX_REQUEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("无法读取 replay stdin 请求：{error}"))?;
        if bytes.len() as u64 > MAX_REQUEST_BYTES {
            return Err("replay JSON 请求超过 16 KiB".to_owned());
        }
        let request: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("replay stdin 不是合法 JSON 请求：{error}"))?;
        request.validate()?;
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_unicode_space_paths_and_cleans_anonymous_input() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target 中文 with spaces");
        let input = ReplayCommandRequest::new(target.clone())
            .input_file(directory.path())
            .unwrap();
        assert_eq!(
            ReplayCommandRequest::read(input).unwrap().target_root,
            target
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn request_rejects_missing_unknown_invalid_and_oversized_input() {
        for input in [
            "",
            "null",
            "{}",
            "{",
            r#"{"format_version":2,"target_root":"/tmp"}"#,
            r#"{"format_version":1,"target_root":"relative"}"#,
            r#"{"format_version":1,"target_root":"/tmp","extra":true}"#,
            r#"{"format_version":1,"target_root":"/tmp"} {}"#,
        ] {
            assert!(
                ReplayCommandRequest::read(input.as_bytes()).is_err(),
                "{input}"
            );
        }
        assert!(
            ReplayCommandRequest::read(&vec![b' '; MAX_REQUEST_BYTES as usize + 1][..])
                .unwrap_err()
                .contains("16 KiB")
        );
    }
}
