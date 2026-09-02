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
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = wide_path(source);
    let target = wide_path(target);
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(windows))]
fn replace_path(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)?;
    Ok(())
}
