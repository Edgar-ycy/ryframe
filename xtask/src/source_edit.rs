use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime},
};

use crate::Result;

pub(crate) struct SourceEdit {
    path: PathBuf,
    original: Vec<u8>,
    modified: Vec<u8>,
    active: bool,
}

pub(crate) struct EditStart {
    pub(crate) monotonic: Instant,
    pub(crate) wall_clock: SystemTime,
}

impl SourceEdit {
    pub(crate) fn apply(
        path: &Path,
        label: &str,
        comment_prefix: &str,
    ) -> Result<(Self, EditStart)> {
        let original = fs::read(path)
            .map_err(|error| format!("无法读取测量源码 {}：{error}", path.display()))?;
        let mut modified = original.clone();
        if !modified.ends_with(b"\n") {
            modified.push(b'\n');
        }
        modified.extend_from_slice(
            format!("{comment_prefix} ryframe-devex 保存事件：{label}\n").as_bytes(),
        );
        let started = atomic_replace_if_unchanged(path, &original, &modified)?;
        Ok((
            Self {
                path: path.to_path_buf(),
                original,
                modified,
                active: true,
            },
            started,
        ))
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        if self.active {
            atomic_replace_if_unchanged(&self.path, &self.modified, &self.original).map_err(
                |error| -> Box<dyn std::error::Error> {
                    format!("无法安全还原测量源码 {}：{error}", self.path.display()).into()
                },
            )?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for SourceEdit {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

static NEXT_ATOMIC_WRITE: AtomicU64 = AtomicU64::new(0);

fn atomic_replace_if_unchanged(path: &Path, expected: &[u8], contents: &[u8]) -> Result<EditStart> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("原子写入路径缺少 UTF-8 文件名：{}", path.display()))?;
    let nonce = NEXT_ATOMIC_WRITE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{file_name}.devex-{}-{nonce}.tmp",
        std::process::id()
    ));
    let permissions = fs::metadata(path)?.permissions();
    let write = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        output.write_all(contents)?;
        output.sync_all()?;
        drop(output);
        fs::set_permissions(&temporary, permissions)?;
        if fs::read(path)? != expected {
            return Err("目标文件在测量写入期间被其他操作修改，拒绝覆盖".into());
        }
        let started = EditStart {
            monotonic: Instant::now(),
            wall_clock: SystemTime::now(),
        };
        replace_path(&temporary, path)?;
        Ok(started)
    })();
    if write.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write
}

#[cfg(windows)]
fn replace_path(source: &Path, target: &Path) -> Result<()> {
    use winsafe::{ReplaceFile, co};

    let source = source
        .to_str()
        .ok_or_else(|| format!("原子写入临时路径不是有效 Unicode：{}", source.display()))?;
    let target = target
        .to_str()
        .ok_or_else(|| format!("原子写入目标路径不是有效 Unicode：{}", target.display()))?;
    ReplaceFile(
        target,
        source,
        None,
        co::REPLACEFILE::WRITE_THROUGH | co::REPLACEFILE::IGNORE_ACL_ERRORS,
    )?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_path(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)?;
    Ok(())
}
