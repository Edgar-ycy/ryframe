//! 在统一 data 命令仍持有构建来源锁时核对业务证明工具。

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Read,
    path::{Path, PathBuf},
};

use ryframe_kernel::{AppError, AppResult};
use same_file::Handle;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::proof_file::{regular_path, same_metadata};

const SOURCE: &str = "构建来源文件";
const LOCK: &str = "构建来源锁";
const GRANT: &str = "恢复证明核验器绑定";
const TOOL: &str = "恢复证明核验器";
const MAX_GRANT_BYTES: u64 = 16 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactSource {
    pub(super) path: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    format_version: u8,
    kind: String,
    source_sha: String,
    verifier: ArtifactSource,
}

pub(super) struct VerifierBinding {
    lock: ProtectedFile,
    source: ProtectedFile,
    source_bytes: Vec<u8>,
    grant: ProtectedFile,
    grant_bytes: Vec<u8>,
    tool: ProtectedFile,
    verifier: ArtifactSource,
}

impl VerifierBinding {
    pub(super) fn read(backend: &Path, expected_source_sha: &str) -> AppResult<Self> {
        if !lower_hex(expected_source_sha, 40) {
            return Err(validation("恢复证明缺少精确构建来源 SHA"));
        }
        let directory = backend.join(".local-tests").join("build");
        let lock = ProtectedFile::open(directory.join("source-commit.lock"), LOCK)?;
        lock.require_held()?;
        let source = ProtectedFile::open(directory.join("source-commit"), SOURCE)?;
        let source_bytes = format!("{expected_source_sha}\n").into_bytes();
        source.require_bytes(&source_bytes, source_bytes.len() as u64, SOURCE)?;
        let grant = ProtectedFile::open(directory.join("restore-proof-verifier.json"), GRANT)?;
        let grant_bytes = grant.read_bytes(MAX_GRANT_BYTES, GRANT)?;
        let value: Grant = serde_json::from_slice(&grant_bytes)
            .map_err(|_| validation("恢复证明核验器绑定不是当前严格 JSON 格式"))?;
        if value.format_version != 1
            || value.kind != "ryframe-restore-proof-verifier"
            || value.source_sha != expected_source_sha
            || value.verifier.bytes == 0
            || !lower_hex(&value.verifier.sha256, 64)
        {
            return Err(validation(
                "恢复证明核验器绑定与当前构建来源或工具格式不一致",
            ));
        }
        let tool = ProtectedFile::open(canonical_tool(&value.verifier.path)?, TOOL)?;
        let binding = Self {
            lock,
            source,
            source_bytes,
            grant,
            grant_bytes,
            tool,
            verifier: value.verifier,
        };
        binding.check()?;
        Ok(binding)
    }

    pub(super) fn verifier(&self) -> &ArtifactSource {
        &self.verifier
    }

    pub(super) fn check(&self) -> AppResult<()> {
        self.lock.require_held()?;
        canonical_tool(&self.verifier.path)?;
        self.tool.require_artifact(&self.verifier)?;
        self.source
            .require_bytes(&self.source_bytes, self.source_bytes.len() as u64, SOURCE)?;
        self.grant
            .require_bytes(&self.grant_bytes, MAX_GRANT_BYTES, GRANT)?;
        drop(self.tool.same_handle(false, TOOL)?);
        self.lock.require_held()
    }
}

struct ProtectedFile {
    path: PathBuf,
    identity: Handle,
}

impl ProtectedFile {
    fn open(path: PathBuf, label: &str) -> AppResult<Self> {
        let identity = open_file(&path, false, label)?;
        Ok(Self { path, identity })
    }

    fn same_handle(&self, writable: bool, label: &str) -> AppResult<Handle> {
        let observed = open_file(&self.path, writable, label)?;
        if observed != self.identity {
            return Err(validation(format!("{label}文件身份发生变化")));
        }
        Ok(observed)
    }

    fn require_held(&self) -> AppResult<()> {
        // Windows 会拒绝其他句柄读取已锁定的字节，只以独立打开的句柄探测内核锁。
        let probe = self.same_handle(true, LOCK)?;
        match probe.as_file().try_lock() {
            Err(TryLockError::WouldBlock) => (),
            Ok(()) => {
                probe
                    .as_file()
                    .unlock()
                    .map_err(|_| validation("无法释放恢复证明的锁探测句柄"))?;
                return Err(validation(
                    "恢复证明必须由仍持有构建来源锁的统一 data 命令执行",
                ));
            }
            Err(TryLockError::Error(_)) => return Err(validation("无法核验构建来源锁")),
        }
        drop(self.same_handle(false, LOCK)?);
        Ok(())
    }

    fn read_bytes(&self, limit: u64, label: &str) -> AppResult<Vec<u8>> {
        let opened = self.same_handle(false, label)?;
        let file = opened.as_file();
        let before = file
            .metadata()
            .map_err(|_| validation(format!("无法核验{label}")))?;
        if !(1..=limit).contains(&before.len()) {
            return Err(validation(format!("{label}为空或超过字节上限")));
        }
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| validation(format!("无法读取{label}")))?;
        if bytes.len() as u64 != before.len() {
            return Err(validation(format!("{label}读取期间长度发生变化")));
        }
        self.require_stable(file, &before, label)?;
        Ok(bytes)
    }

    fn require_bytes(&self, expected: &[u8], limit: u64, label: &str) -> AppResult<()> {
        if self.read_bytes(limit, label)? != expected {
            return Err(validation(format!("{label}原始字节与当前绑定不一致")));
        }
        Ok(())
    }

    fn require_artifact(&self, expected: &ArtifactSource) -> AppResult<()> {
        let opened = self.same_handle(false, TOOL)?;
        let file = opened.as_file();
        let before = file
            .metadata()
            .map_err(|_| validation("无法核验恢复证明核验器"))?;
        if before.len() != expected.bytes {
            return Err(validation("恢复证明核验器字节数与绑定不一致"));
        }
        let mut data = file.take(expected.bytes);
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = data
                .read(&mut buffer)
                .map_err(|_| validation("无法读取恢复证明核验器"))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if data.limit() != 0 || hex::encode(digest.finalize()) != expected.sha256 {
            return Err(validation("恢复证明核验器摘要与绑定不一致"));
        }
        self.require_stable(file, &before, TOOL)
    }

    fn require_stable(&self, file: &File, before: &fs::Metadata, label: &str) -> AppResult<()> {
        let after = file
            .metadata()
            .map_err(|_| validation(format!("无法核验{label}")))?;
        let path = self.same_handle(false, label)?;
        let path_metadata = path
            .as_file()
            .metadata()
            .map_err(|_| validation(format!("无法核验{label}")))?;
        if !same_metadata(before, &after) || !same_metadata(before, &path_metadata) {
            return Err(validation(format!("{label}读取期间发生变化")));
        }
        Ok(())
    }
}

fn open_file(path: &Path, writable: bool, label: &str) -> AppResult<Handle> {
    let before = regular_path(path, label).map_err(validation)?;
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .open(path)
        .map_err(|_| validation(format!("无法打开{label}")))?;
    let opened = file
        .metadata()
        .map_err(|_| validation(format!("无法核验{label}")))?;
    let identity =
        Handle::from_file(file).map_err(|_| validation(format!("无法核验{label}身份")))?;
    let after = regular_path(path, label).map_err(validation)?;
    let observed =
        Handle::from_path(path).map_err(|_| validation(format!("无法核验{label}身份")))?;
    if !opened.is_file()
        || identity != observed
        || !same_metadata(&before, &opened)
        || !same_metadata(&opened, &after)
    {
        return Err(validation(format!("{label}在打开期间被替换或修改")));
    }
    Ok(identity)
}

fn canonical_tool(value: &str) -> AppResult<PathBuf> {
    let path = Path::new(value);
    if value.contains(['\0', '\r', '\n']) {
        return Err(validation("恢复证明核验器必须是规范绝对文件路径"));
    }
    regular_path(path, TOOL).map_err(validation)?;
    let canonical = path
        .canonicalize()
        .map_err(|_| validation("无法核验恢复证明核验器规范路径"))?;
    let canonical = canonical
        .to_str()
        .ok_or_else(|| validation("恢复证明核验器规范路径不是 UTF-8"))?;
    #[cfg(windows)]
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
    if canonical != value {
        return Err(validation("恢复证明核验器必须使用规范绝对文件路径"));
    }
    Ok(path.to_path_buf())
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::Validation(message.into())
}
