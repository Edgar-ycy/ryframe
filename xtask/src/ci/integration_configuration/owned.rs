use super::files::{self, Image, diagnostic};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

// 仅删除已成功发布且完整前像仍一致的已知文件；未知或漂移现场始终保留。
pub(crate) struct OwnedConfiguration {
    pub(crate) directory: PathBuf,
    images: BTreeMap<String, Image>,
}

impl OwnedConfiguration {
    pub(crate) fn create(
        directory: &Path,
        content: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, String> {
        files::require_directory(
            directory
                .parent()
                .ok_or_else(|| diagnostic(directory, "创建快照", "缺少父目录"))?,
        )?;
        fs::create_dir(directory).map_err(|error| {
            files::io_error(directory, "独占创建快照目录；已有目录不覆盖", error)
        })?;
        let mut owned = Self {
            directory: directory.to_owned(),
            images: BTreeMap::new(),
        };
        for (name, bytes) in content {
            if !matches!(name.as_str(), "app.toml" | "app.test.toml" | "test.toml") {
                return Err(diagnostic(
                    directory,
                    "发布快照",
                    "存在未知文件名，目录已创建，保留现场",
                ));
            }
            let path = directory.join(name);
            files::publish(&path, bytes)?;
            let image = files::read_required(&path)?;
            if image.bytes != *bytes {
                return Err(diagnostic(&path, "复核发布", "发布后字节变化，保留现场"));
            }
            owned.images.insert(name.clone(), image);
        }
        owned.verify()?;
        Ok(owned)
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        files::require_directory(&self.directory)?;
        let entries = fs::read_dir(&self.directory)
            .map_err(|error| files::io_error(&self.directory, "枚举独占目录", error))?;
        let mut count = 0;
        for entry in entries {
            let entry =
                entry.map_err(|error| files::io_error(&self.directory, "枚举独占目录项", error))?;
            let name = entry.file_name().into_string().map_err(|_| {
                diagnostic(
                    &self.directory,
                    "复核快照",
                    "存在非UTF8未知目录项，保留现场",
                )
            })?;
            if !self.images.contains_key(&name) {
                return Err(diagnostic(
                    &entry.path(),
                    "复核快照",
                    "发现外来文件，保留整个目录",
                ));
            }
            count += 1;
        }
        if count != self.images.len() {
            return Err(diagnostic(
                &self.directory,
                "复核快照",
                "已发布文件缺失，保留现场",
            ));
        }
        for (name, expected) in &self.images {
            let path = self.directory.join(name);
            if files::read_required(&path)? != *expected {
                return Err(diagnostic(
                    &path,
                    "复核快照",
                    "字节或元数据漂移，保留现场，不删除或覆盖",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn remove(self) -> Result<(), String> {
        self.verify()?;
        for name in self.images.keys() {
            let path = self.directory.join(name);
            if files::read_required(&path)? != self.images[name] {
                return Err(diagnostic(&path, "清理前复核", "文件漂移，停止清理"));
            }
            fs::remove_file(&path)
                .map_err(|error| files::io_error(&path, "清理明确拥有文件", error))?;
        }
        fs::remove_dir(&self.directory).map_err(|error| {
            files::io_error(&self.directory, "删除空快照目录；外来内容保留", error)
        })
    }
}
