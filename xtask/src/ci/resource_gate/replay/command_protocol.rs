use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

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
        let request: ReplayCommandRequest = serde_json::from_reader(input).unwrap();
        assert_eq!(request.target_root, target);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        for (target, reason) in [
            (PathBuf::from("relative"), "target_root 须为绝对路径"),
            (
                directory
                    .path()
                    .join("x".repeat(MAX_REQUEST_BYTES as usize + 1)),
                "16 KiB",
            ),
        ] {
            assert!(
                ReplayCommandRequest::new(target)
                    .input_file(directory.path())
                    .unwrap_err()
                    .contains(reason)
            );
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }
}
