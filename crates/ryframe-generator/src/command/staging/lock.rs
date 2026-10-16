use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct GenerationLock {
    path: PathBuf,
    identity: Vec<u8>,
}

impl GenerationLock {
    pub(super) fn acquire(path: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(path.parent().ok_or("生成锁缺少父目录")?)
            .map_err(|error| format!("无法创建生成锁目录：{error}"))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let identity = format!(
            "pid={}\nnonce={nonce}\nkind=business-generator\n",
            process::id()
        )
        .into_bytes();
        let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|error| format!("无法取得生成锁 {}：{error}；本批未安装。确认该锁所属进程已结束后再处理这个精确锁文件，不会自动删除", path.display()))?;
        if let Err(error) = file.write_all(&identity).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(format!("无法保存生成锁：{error}；本批未安装"));
        }
        Ok(Self { path, identity })
    }

    pub(super) fn release(&self) -> Result<(), String> {
        match fs::read(&self.path) {
            Ok(bytes) if bytes == self.identity => fs::remove_file(&self.path)
                .map_err(|error| format!("无法清理本次生成锁 {}：{error}", self.path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(format!(
                "生成锁归属已变化，拒绝删除 {}；核对生成结果和并发控制器",
                self.path.display()
            )),
        }
    }
}

impl Drop for GenerationLock {
    fn drop(&mut self) {
        if fs::read(&self.path).is_ok_and(|bytes| bytes == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_requires_exclusive_ownership_and_preserves_a_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("generation.lock");
        let first = GenerationLock::acquire(path.clone()).unwrap();
        assert!(GenerationLock::acquire(path.clone()).is_err());
        fs::write(&path, "另一个控制器").unwrap();
        assert!(first.release().unwrap_err().contains("归属已变化"));
        drop(first);
        assert_eq!(fs::read_to_string(&path).unwrap(), "另一个控制器");
        fs::remove_file(&path).unwrap();
        let second = GenerationLock::acquire(path.clone()).unwrap();
        second.release().unwrap();
        assert!(!path.exists());
    }
}
