use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Result;

pub(crate) const ENVIRONMENT_KEY: &str = "RYFRAME_DEVEX_NATIVE_TOOL";

/// 绑定当前控制器产物，子进程不通过 Cargo 临时编译或搜索 PATH 中的同名程序。
pub(crate) fn current_binding() -> Result<String> {
    let executable = std::env::current_exe()?;
    Ok(serde_json::to_string(&binding(&executable)?)?)
}

pub(crate) fn binding(path: &Path) -> Result<Value> {
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
    Ok(
        json!({"format_version": 1, "kind": "ryframe-devex-native-tool", "path": path.to_str().ok_or("原生工具路径必须是UTF-8")?, "bytes": bytes, "sha256": hex::encode(hash.finalize())}),
    )
}
