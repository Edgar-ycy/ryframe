use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const SOURCE_NAME: &str = "source-commit";
const LOCK_NAME: &str = "source-commit.lock";
const RESTORE_VERIFIER_NAME: &str = "restore-proof-verifier.json";

/// 在单次 Cargo 构建期间发布精确源码提交，并在内容未漂移时清理。
pub(crate) fn with_source_commit<T>(
    root: &Path,
    commit: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_binding(root, commit, |_| operation())
}

/// 在同一源码绑定中只向正式恢复证明操作发放当前核验器。
pub(crate) fn with_restore_verifier<T>(
    root: &Path,
    commit: &str,
    coordinator: &serde_json::Value,
    verifier: &serde_json::Value,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_binding(root, commit, |binding| {
        binding.with_restore_verifier(root, commit, coordinator, verifier, operation)
    })
}

fn with_binding<T>(
    root: &Path,
    commit: &str,
    operation: impl FnOnce(&BuildSourceBinding) -> Result<T>,
) -> Result<T> {
    validate_commit(commit)?;
    let binding = BuildSourceBinding::create(root, commit)?;
    let result = operation(&binding);
    let cleanup = binding.finish();
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(operation), Err(cleanup)) => Err(format!(
            "构建失败：{operation}；构建来源清理也失败：{cleanup}；保留现场，不自动重放"
        )
        .into()),
    }
}

fn validate_commit(commit: &str) -> Result<()> {
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("构建来源必须是 40 位小写十六进制提交 SHA".into());
    }
    Ok(())
}

struct BuildSourceBinding {
    source: PathBuf,
    source_bytes: Vec<u8>,
    lock: PathBuf,
    lock_bytes: Vec<u8>,
    active: bool,
}

impl BuildSourceBinding {
    fn create(root: &Path, commit: &str) -> Result<Self> {
        let local = root.join(".local-tests");
        if !local.exists() {
            fs::create_dir(&local)
                .map_err(|error| format!("无法创建构建来源根目录 {}：{error}", local.display()))?;
        }
        let directory = local.join("build");
        validate_local_test_path(&directory, root, LocalTestPathKind::StateDirectory)
            .map_err(|error| format!("构建来源目录无效：{error}"))?;
        if !directory.exists() {
            fs::create_dir(&directory).map_err(|error| {
                format!("无法创建构建来源目录 {}：{error}", directory.display())
            })?;
        }
        validate_local_test_path(&directory, root, LocalTestPathKind::ExistingDirectory)
            .map_err(|error| format!("构建来源目录无效：{error}"))?;
        let source = directory.join(SOURCE_NAME);
        let lock = directory.join(LOCK_NAME);
        for path in [&source, &lock] {
            validate_local_test_path(path, root, LocalTestPathKind::OutputFile)
                .map_err(|error| format!("构建来源文件 {} 无效：{error}", path.display()))?;
        }
        let lock_bytes = format!("{}:{}\n", std::process::id(), uuid::Uuid::new_v4()).into_bytes();
        write_new(&lock, &lock_bytes, "构建来源锁")?;
        let source_bytes = format!("{commit}\n").into_bytes();
        if let Err(error) = write_new(&source, &source_bytes, "构建来源") {
            let _ = remove_exact(&lock, &lock_bytes, "构建来源锁");
            return Err(error);
        }
        Ok(Self {
            source,
            source_bytes,
            lock,
            lock_bytes,
            active: true,
        })
    }

    fn with_restore_verifier<T>(
        &self,
        root: &Path,
        commit: &str,
        coordinator: &serde_json::Value,
        verifier: &serde_json::Value,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let path = self.lock.with_file_name(RESTORE_VERIFIER_NAME);
        validate_local_test_path(&path, root, LocalTestPathKind::OutputFile)?;
        validate_local_test_path(&self.lock, root, LocalTestPathKind::ExistingFile)?;
        let lock = OpenOptions::new().read(true).write(true).open(&self.lock)?;
        let identity = same_file::Handle::from_file(lock.try_clone()?)?;
        lock.try_lock()
            .map_err(|_| "恢复核验器发放要求独占同一构建来源锁")?;
        self.check_lock(&lock, &identity)?;
        let grant = serde_json::json!({
            "format_version":1, "kind":"ryframe-restore-proof-verifier",
            "source_sha":commit, "coordinator":coordinator, "verifier":verifier,
        });
        let mut bytes = serde_json::to_vec(&grant)?;
        bytes.push(b'\n');
        if bytes.len() > 16 * 1024 {
            return Err("恢复核验器发放文件超过 16 KiB".into());
        }
        write_new(&path, &bytes, "恢复核验器发放文件")?;
        let grant_identity = same_file::Handle::from_path(&path)?;
        let result = operation();
        let cleanup = self.check_lock(&lock, &identity).and_then(|()| {
            if same_file::Handle::from_path(&path)? != grant_identity {
                return Err("恢复核验器发放文件身份已漂移，保留现场".into());
            }
            drop(grant_identity);
            remove_exact(&path, &bytes, "恢复核验器发放文件")
        });
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(format!("{error}；恢复核验器发放收尾失败：{cleanup}").into())
            }
        }
    }

    fn check_lock(&self, mut lock: &File, identity: &same_file::Handle) -> Result<()> {
        lock.rewind()?;
        let mut bytes = Vec::new();
        lock.take(4097).read_to_end(&mut bytes)?;
        if bytes != self.lock_bytes || &same_file::Handle::from_path(&self.lock)? != identity {
            return Err("构建来源锁在恢复核验期间变化".into());
        }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.active = false;
        remove_exact(&self.source, &self.source_bytes, "构建来源")?;
        remove_exact(&self.lock, &self.lock_bytes, "构建来源锁")
    }
}

impl Drop for BuildSourceBinding {
    fn drop(&mut self) {
        if self.active {
            let _ = remove_exact(&self.source, &self.source_bytes, "构建来源");
            let _ = remove_exact(&self.lock, &self.lock_bytes, "构建来源锁");
        }
    }
}

fn write_new(path: &Path, bytes: &[u8], label: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法独占创建{label} {}：{error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("无法持久化{label} {}：{error}", path.display()).into())
}

fn remove_exact(path: &Path, expected: &[u8], label: &str) -> Result<()> {
    let observed =
        fs::read(path).map_err(|error| format!("无法复核{label} {}：{error}", path.display()))?;
    if observed != expected {
        return Err(format!("{label} {} 已漂移；保留现场，不清理", path.display()).into());
    }
    fs::remove_file(path)
        .map_err(|error| format!("无法清理{label} {}：{error}", path.display()).into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{RESTORE_VERIFIER_NAME, with_restore_verifier, with_source_commit};
    use crate::workspace::root_dir;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn fixture() -> tempfile::TempDir {
        let parent = root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let root = tempfile::tempdir_in(parent).unwrap();
        fs::create_dir(root.path().join(".local-tests")).unwrap();
        root
    }

    #[test]
    fn source_binding_is_exact_and_removed_after_operation() {
        let root = fixture();
        let source = root.path().join(".local-tests/build/source-commit");
        let value = with_source_commit(root.path(), SHA, || {
            assert_eq!(fs::read_to_string(&source).unwrap(), format!("{SHA}\n"));
            Ok(42)
        })
        .unwrap();
        assert_eq!(value, 42);
        assert!(!source.exists());
        assert!(!source.with_extension("lock").exists());
    }

    #[test]
    fn source_drift_is_preserved_and_reported() {
        let root = fixture();
        let source = root.path().join(".local-tests/build/source-commit");
        let error = with_source_commit(root.path(), SHA, || {
            fs::write(&source, b"changed\n")?;
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("漂移"));
        assert_eq!(fs::read(&source).unwrap(), b"changed\n");
        assert!(
            root.path()
                .join(".local-tests/build/source-commit.lock")
                .is_file()
        );
    }

    #[test]
    fn invalid_or_concurrent_binding_never_runs_operation() {
        let root = fixture();
        let mut called = false;
        assert!(
            with_source_commit(root.path(), "HEAD", || {
                called = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!called);
        fs::create_dir_all(root.path().join(".local-tests/build")).unwrap();
        fs::write(
            root.path().join(".local-tests/build/source-commit"),
            format!("{SHA}\n"),
        )
        .unwrap();
        assert!(
            with_source_commit(root.path(), SHA, || {
                called = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!called);
    }

    #[test]
    fn restore_verifier_is_published_under_the_same_live_source_lock() {
        let root = fixture();
        let directory = root.path().join(".local-tests/build");
        let grant = directory.join(RESTORE_VERIFIER_NAME);
        let lock = directory.join("source-commit.lock");
        let tool = serde_json::json!({"path":root.path().join("xtask.exe"),"bytes":1,"sha256":"a".repeat(64)});
        let coordinator = serde_json::json!({"root":root.path().join("separate-coordinator"),"sha":"b".repeat(40)});
        with_restore_verifier(root.path(), SHA, &coordinator, &tool, || {
            let contender = fs::OpenOptions::new().read(true).write(true).open(&lock)?;
            assert!(matches!(contender.try_lock(), Err(fs::TryLockError::WouldBlock)));
            let value: serde_json::Value = serde_json::from_slice(&fs::read(&grant)?)?;
            assert_eq!(value, serde_json::json!({"format_version":1,"kind":"ryframe-restore-proof-verifier","source_sha":SHA,"coordinator":coordinator,"verifier":tool}));
            Ok(())
        }).unwrap();
        assert!(!grant.exists());
        assert!(!lock.exists());

        let changed = with_restore_verifier(root.path(), SHA, &coordinator, &tool, || {
            fs::write(&grant, b"changed\n")?;
            Ok(())
        })
        .unwrap_err();
        assert!(changed.to_string().contains("已漂移"));
        assert_eq!(fs::read(&grant).unwrap(), b"changed\n");
    }
}
