use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub(crate) format_version: u8,
    pub(crate) kind: String,
    pub(crate) path: std::path::PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

impl Binding {
    pub(crate) fn valid(&self) -> bool {
        self.format_version == 1
            && self.kind == "ryframe-devex-native-tool"
            && self.path.is_absolute()
            && self.bytes > 0
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// 绑定当前控制器产物，子进程不通过 Cargo 临时编译或搜索 PATH 中的同名程序。
pub(crate) fn current_binding() -> Result<Binding> {
    let executable = std::env::current_exe()?;
    typed_binding(&executable)
}

pub(crate) fn typed_binding(path: &Path) -> Result<Binding> {
    let metadata = fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
    {
        return Err("原生任务采集器必须绑定当前控制器的绝对普通产物路径".into());
    }
    let mut source = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut block = [0_u8; 65_536];
    loop {
        let length = source.read(&mut block)?;
        if length == 0 {
            break;
        }
        bytes = bytes.checked_add(length as u64).ok_or("原生工具大小溢出")?;
        hash.update(&block[..length]);
    }
    if bytes != metadata.len() || fs::metadata(path)?.len() != bytes {
        return Err("绑定期间原生任务采集器产物变化".into());
    }
    Ok(Binding {
        format_version: 1,
        kind: "ryframe-devex-native-tool".into(),
        path: path.to_path_buf(),
        bytes,
        sha256: hex::encode(hash.finalize()),
    })
}
