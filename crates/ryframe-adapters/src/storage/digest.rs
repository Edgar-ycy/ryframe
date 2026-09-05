use super::{ObjectDigest, StorageError, StorageResult};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncReadExt;

pub(crate) async fn file_digest(path: &Path) -> StorageResult<ObjectDigest> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|source| StorageError::Io {
            operation: "读取校验文件",
            source,
        })?;
    let metadata = file.metadata().await.map_err(|source| StorageError::Io {
        operation: "检查校验文件",
        source,
    })?;
    if !metadata.is_file() {
        return Err(StorageError::InvalidLocation(
            "校验对象必须是普通文件".into(),
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| StorageError::Io {
                operation: "流式校验文件",
                source,
            })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes += read as u64;
    }
    if bytes != metadata.len() {
        return Err(StorageError::InvalidResponse(
            "文件大小在校验期间发生变化".into(),
        ));
    }
    Ok(ObjectDigest {
        bytes,
        sha256: hex::encode(digest.finalize()),
    })
}
