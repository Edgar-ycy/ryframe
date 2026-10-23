//! 完整编译来源的有界双读、真实文件身份与聚合快照；不返回文件内容。
use std::{
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[path = "sources.rs"]
mod sources;

pub(super) const LIMIT: usize = 4 * 1024 * 1024;
const FILE_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub(super) path: String,
    pub(super) sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format_version: u32,
    root: PathBuf,
    sources: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compiled {
    root: PathBuf,
    files: Vec<Source>,
}

#[derive(Debug)]
pub(super) struct Failure {
    category: &'static str,
    path: String,
}

type Result<T> = std::result::Result<T, Failure>;

fn fail(category: &'static str, path: &Path) -> Failure {
    let label = path.to_string_lossy();
    Failure {
        category,
        // 非法超长或控制字符路径也只能产生小体积的固定错误协议。
        path: if label.len() <= 4096 && !label.contains(['\0', '\r', '\n']) {
            label.into_owned()
        } else {
            "verify-sources".into()
        },
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct State {
    device: u64,
    inode: u64,
    size: u64,
    mtime: i128,
    ctime: i128,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(super) struct Token {
    format_version: u32,
    kind: &'static str,
    source_count: usize,
    sha256: String,
}

pub(super) fn run() -> (serde_json::Value, i32) {
    match execute(&mut io::stdin().lock()) {
        Ok(token) => (
            serde_json::json!({"format_version":token.format_version,
            "kind":token.kind,"source_count":token.source_count,"sha256":token.sha256}),
            0,
        ),
        Err(error) => (
            serde_json::json!({"format_version":1,
            "kind":"frontend-origin-source-error","category":error.category,"path":error.path}),
            2,
        ),
    }
}

pub(super) fn execute(input: &mut impl Read) -> Result<Token> {
    let compiled: Compiled = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/frontend-origin-sources.json"
    )))
    .map_err(|_| fail("compiled_snapshot", Path::new(sources::POLICY)))?;
    let request = parse(input)?;
    verify(&request, &compiled.root, &compiled.files, |_, _| {})
}

fn parse(input: &mut impl Read) -> Result<Request> {
    let label = Path::new("verify-sources");
    let mut bytes = Vec::new();
    input
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("input", label))?;
    if bytes.len() > LIMIT || bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(fail("request", label));
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| fail("request", label))?;
    if request.format_version != 1
        || !request
            .sources
            .iter()
            .any(|source| source.path == sources::POLICY)
    {
        return Err(fail("request", label));
    }
    let mut previous = "";
    for source in &request.sources {
        if source.path.as_str() <= previous
            || !relative(&source.path)
            || source.sha256.len() != 64
            || !source
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(fail("request", label));
        }
        previous = &source.path;
    }
    Ok(request)
}

fn relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0', '\r', '\n'])
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn verify(
    request: &Request,
    compiled_root: &Path,
    compiled: &[Source],
    mut observe: impl FnMut(&Path, &'static str),
) -> Result<Token> {
    let root = ordinary(&request.root, true)?;
    let expected_root = ordinary(compiled_root, true)?;
    if root != expected_root || request.sources.as_slice() != compiled {
        return Err(fail("binding", &request.root));
    }
    let root_before = Held::open(&root, true)?.state(&root, true)?;
    complete(&root, compiled)?;
    let mut aggregate = Sha256::new();
    append(
        &mut aggregate,
        &(
            1_u32,
            "frontend-origin-source-snapshot",
            &root,
            (root_before.device, root_before.inode),
            compiled,
        ),
    )?;
    for source in compiled {
        let path = root.join(&source.path);
        let state = snapshot(&path, &source.sha256, &mut observe)?;
        append(&mut aggregate, &(source, state))?;
    }
    complete(&root, compiled)?;
    ordinary(&root, true)?;
    let root_after = Held::open(&root, true)?.state(&root, true)?;
    if (root_before.device, root_before.inode) != (root_after.device, root_after.inode) {
        return Err(fail("identity", &root));
    }
    Ok(Token {
        format_version: 1,
        kind: "frontend-origin-source-snapshot",
        source_count: compiled.len(),
        sha256: hex::encode(aggregate.finalize()),
    })
}

fn append(hasher: &mut Sha256, value: &impl Serialize) -> Result<()> {
    let encoded = serde_json::to_vec(value)
        .map_err(|_| fail("compiled_snapshot", Path::new(sources::POLICY)))?;
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(())
}

fn complete(root: &Path, compiled: &[Source]) -> Result<()> {
    let actual =
        sources::discover(root).map_err(|_| fail("source_set", &root.join(sources::POLICY)))?;
    if actual
        .iter()
        .map(String::as_str)
        .ne(compiled.iter().map(|v| v.path.as_str()))
    {
        return Err(fail("source_set", &root.join(sources::POLICY)));
    }
    Ok(())
}

fn snapshot(
    path: &Path,
    expected: &str,
    observe: &mut impl FnMut(&Path, &'static str),
) -> Result<State> {
    ordinary(path, false)?;
    let before = Held::open(path, false)?.state(path, false)?;
    observe(path, "before_open");
    let mut file = Held::open(path, false)?;
    let opened = file.state(path, false)?;
    if before != opened {
        return Err(fail("identity", path));
    }
    let first = file.read(path)?;
    observe(path, "between_reads");
    let second = file.read(path)?;
    let closed = file.state(path, false)?;
    if before != closed || first != second {
        return Err(fail("drift", path));
    }
    let sha256 = hex::encode(Sha256::digest(&first));
    if sha256 != expected {
        return Err(fail("digest", path));
    }
    drop(file);
    observe(path, "after_close");
    ordinary(path, false)?;
    let after = Held::open(path, false)?.state(path, false)?;
    if before != after {
        return Err(fail("identity", path));
    }
    Ok(before)
}

fn ordinary(path: &Path, directory: bool) -> Result<PathBuf> {
    if !path.is_absolute()
        || path.to_str().is_none_or(|v| {
            v.contains(['\0', '\r', '\n'])
                || v.split(['/', '\\']).any(|part| matches!(part, "." | ".."))
        })
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(fail("path", path));
    }
    let canonical = fs::canonicalize(path).map_err(|_| fail("path", path))?;
    for (index, ancestor) in path.ancestors().enumerate() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| fail("path", path))?;
        let is_directory = index != 0 || directory;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(fail("link", ancestor));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(fail("link", ancestor));
        }
        if if is_directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        } {
            return Err(fail("path", ancestor));
        }
        if fs::canonicalize(ancestor).map_err(|_| fail("path", ancestor))?
            != canonical_ancestor(&canonical, index)?
        {
            return Err(fail("path", ancestor));
        }
    }
    Ok(canonical)
}

fn canonical_ancestor(path: &Path, index: usize) -> Result<PathBuf> {
    path.ancestors()
        .nth(index)
        .map(Path::to_path_buf)
        .ok_or_else(|| fail("path", path))
}

#[cfg(unix)]
struct Held(fs::File);

#[cfg(unix)]
impl Held {
    fn open(path: &Path, _directory: bool) -> Result<Self> {
        fs::File::open(path)
            .map(Self)
            .map_err(|_| fail("read", path))
    }
    fn state(&self, path: &Path, directory: bool) -> Result<State> {
        use std::os::unix::fs::MetadataExt;
        let metadata = self.0.metadata().map_err(|_| fail("read", path))?;
        if metadata.is_dir() != directory || (!directory && !metadata.is_file()) {
            return Err(fail("path", path));
        }
        Ok(State {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime: i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
            ctime: i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
        })
    }
    fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        use std::io::{Seek, SeekFrom};
        self.0
            .seek(SeekFrom::Start(0))
            .map_err(|_| fail("read", path))?;
        let mut bytes = Vec::new();
        (&mut self.0)
            .take((FILE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| fail("read", path))?;
        if bytes.len() > FILE_LIMIT {
            return Err(fail("size", path));
        }
        Ok(bytes)
    }
}

#[cfg(windows)]
struct Held(winsafe::guard::CloseHandleGuard<winsafe::HFILE>);

#[cfg(windows)]
impl Held {
    fn open(path: &Path, _directory: bool) -> Result<Self> {
        use winsafe::{co, prelude::kernel_Hfile};
        let text = path.to_str().ok_or_else(|| fail("path", path))?;
        let (handle, _) = winsafe::HFILE::CreateFile(
            text,
            co::GENERIC::READ,
            Some(co::FILE_SHARE::READ | co::FILE_SHARE::WRITE | co::FILE_SHARE::DELETE),
            None,
            co::DISPOSITION::OPEN_EXISTING,
            co::FILE_ATTRIBUTE::NORMAL,
            Some(co::FILE_FLAG::BACKUP_SEMANTICS | co::FILE_FLAG::OPEN_REPARSE_POINT),
            None,
            None,
        )
        .map_err(|_| fail("read", path))?;
        Ok(Self(handle))
    }
    fn state(&self, path: &Path, directory: bool) -> Result<State> {
        use winsafe::prelude::kernel_Hfile;
        let mut info = winsafe::BY_HANDLE_FILE_INFORMATION::default();
        self.0
            .GetFileInformationByHandle(&mut info)
            .map_err(|_| fail("read", path))?;
        windows_state(&info, path, directory)
    }
    fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        use winsafe::{co, prelude::kernel_Hfile};
        self.0
            .SetFilePointerEx(0, co::FILE_STARTING_POINT::BEGIN)
            .map_err(|_| fail("read", path))?;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        while bytes.len() <= FILE_LIMIT {
            let capacity = buffer.len().min(FILE_LIMIT + 1 - bytes.len());
            let count = self
                .0
                .ReadFile(&mut buffer[..capacity], None)
                .map_err(|_| fail("read", path))? as usize;
            if count == 0 {
                return Ok(bytes);
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        Err(fail("size", path))
    }
}

#[cfg(windows)]
fn windows_state(
    info: &winsafe::BY_HANDLE_FILE_INFORMATION,
    path: &Path,
    directory: bool,
) -> Result<State> {
    use winsafe::{co, prelude::NativeBitflag};
    if info.dwFileAttributes.has(co::FILE_ATTRIBUTE::REPARSE_POINT) {
        return Err(fail("link", path));
    }
    if info.dwFileAttributes.has(co::FILE_ATTRIBUTE::DIRECTORY) != directory {
        return Err(fail("path", path));
    }
    let time = |v: winsafe::FILETIME| {
        // FILETIME 从 1601 年起算；转换为旧 Python stat 的 Unix 纳秒纪元。
        i128::from((u64::from(v.dwHighDateTime) << 32) | u64::from(v.dwLowDateTime)) * 100
            - 11_644_473_600_000_000_000_i128
    };
    Ok(State {
        device: u64::from(info.dwVolumeSerialNumber),
        inode: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        mtime: time(info.ftLastWriteTime),
        ctime: time(info.ftCreationTime),
    })
}

#[cfg(not(any(unix, windows)))]
struct Held;

#[cfg(not(any(unix, windows)))]
impl Held {
    fn open(path: &Path, _: bool) -> Result<Self> {
        Err(fail("platform", path))
    }
    fn state(&self, path: &Path, _: bool) -> Result<State> {
        Err(fail("platform", path))
    }
    fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        Err(fail("platform", path))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/frontend_origin/source_guard/tests.rs"]
mod tests;
