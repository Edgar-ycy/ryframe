//! 仅在调用方已独占的账本窗口内持久化单个事件。
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use same_file::Handle;
use serde_json::{Value, json};

use super::{
    HEAD_LIMIT, Image, common, created, digest, digest_shape, document, encoded, head, model,
    read_image, verify_journal,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const JOURNAL: &str = "events.ndjson";
const HEAD: &str = "head.json";
const LOCK: &str = "clone.lock";
const GUARD: &str = "copy-ledger.guard";
const JOURNAL_LIMIT: u64 = document::MAX_JSON_BYTES;

pub(in crate::recovery::clone::native) struct Storage<'a> {
    window: Window<'a>,
    plan: &'a str,
    generation: &'a str,
    state: Option<State>,
    poisoned: bool,
}

struct State {
    journal: Image,
    head: Image,
    events: Vec<Value>,
    sha256: Value,
}

/// 已同步且核对过的末帧；账本锁释放后可在同一 guard 下复核原始字节。
pub(super) struct Seal {
    root: PathBuf,
    identity: (u64, u64),
    state: State,
}

impl Seal {
    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        common::local_directory(backend, &self.root, "复制账本目录")?;
        if common::device_inode_for(&self.root, &fs::symlink_metadata(&self.root)?)?
            != self.identity
            || read_image(backend, &self.root.join(JOURNAL), JOURNAL_LIMIT)? != self.state.journal
            || read_image(backend, &self.root.join(HEAD), HEAD_LIMIT)? != self.state.head
        {
            return Err("复制账本收尾期间目录或完整原字节变化".into());
        }
        Ok(())
    }

    pub(super) fn completion(&self, plan: &str, generation: &str) -> Result<Option<Value>> {
        let projected = model::project(&self.state.events)?;
        if projected
            .session
            .as_ref()
            .is_some_and(|session| session.outcome == "publishing")
        {
            let binding = json!({"plan_sha256":plan,"generation_sha256":generation});
            return super::completion_value(&binding, &self.state.sha256, &projected).map(Some);
        }
        Ok(None)
    }
}

impl<'a> Storage<'a> {
    /// verify_owner 必须借用同一根的内核 guard 和实际 clone.lock owner。
    pub(in crate::recovery::clone::native) fn open(
        backend: &'a Path,
        root: &Path,
        plan: &'a str,
        generation: &'a str,
        verify_owner: &'a dyn Fn(&Path) -> Result<()>,
    ) -> Result<Self> {
        let mut storage = Self::bind(backend, root, plan, generation, verify_owner)?;
        let journal = storage.window.read(JOURNAL, JOURNAL_LIMIT)?;
        let head = storage.window.read(HEAD, HEAD_LIMIT)?;
        let verified = verify_journal(&journal.raw, &head.raw, plan, generation)?;
        let sha256 = verified.frames.last().ok_or("复制账本缺少最后一帧")?["sha256"].clone();
        let state = State {
            journal,
            head,
            events: verified.events,
            sha256,
        };
        storage.window.check()?;
        storage.check_state(Some(&state))?;
        storage.window.check()?;
        storage.state = Some(state);
        Ok(storage)
    }

    pub(in crate::recovery::clone::native) fn create(
        backend: &'a Path,
        root: &Path,
        plan: &'a str,
        generation: &'a str,
        verify_owner: &'a dyn Fn(&Path) -> Result<()>,
    ) -> Result<Self> {
        let mut storage = Self::bind(backend, root, plan, generation, verify_owner)?;
        storage.append(created(plan, generation))?;
        Ok(storage)
    }

    fn bind(
        backend: &'a Path,
        root: &Path,
        plan: &'a str,
        generation: &'a str,
        verify_owner: &'a dyn Fn(&Path) -> Result<()>,
    ) -> Result<Self> {
        digest_shape(&json!(plan))?;
        digest_shape(&json!(generation))?;
        Ok(Self {
            window: Window::new(backend, root, verify_owner)?,
            plan,
            generation,
            state: None,
            poisoned: false,
        })
    }

    pub(in crate::recovery::clone::native) fn append(&mut self, event: Value) -> Result<()> {
        if self.poisoned {
            return Err("账本写入结果未知，必须停止并保留证据".into());
        }
        self.poisoned = true;
        self.persist(event)?;
        self.poisoned = false;
        Ok(())
    }

    pub(super) fn poisoned(&self) -> bool {
        self.poisoned
    }

    pub(super) fn projection(&self) -> Result<model::Projection> {
        model::project(&self.checked()?.events)
    }

    pub(super) fn seal(self) -> Result<Seal> {
        self.checked()?;
        Ok(Seal {
            root: self.window.root,
            identity: self.window.identity[0],
            state: self.state.ok_or("复制账本缺少已确认的持久末帧")?,
        })
    }

    fn checked(&self) -> Result<&State> {
        if self.poisoned {
            return Err("账本写入结果未知，必须停止并保留证据".into());
        }
        let state = self.state.as_ref().ok_or("复制账本尚未创建")?;
        self.window.check()?;
        self.check_state(Some(state))?;
        self.window.check()?;
        Ok(state)
    }

    fn persist(&mut self, event: Value) -> Result<()> {
        self.window.check()?;
        self.check_state(self.state.as_ref())?;
        let mut events = self
            .state
            .as_ref()
            .map(|state| state.events.clone())
            .unwrap_or_default();
        let sequence = events.len();
        let previous = self
            .state
            .as_ref()
            .map(|state| state.sha256.clone())
            .unwrap_or_else(|| json!("0".repeat(64)));
        let mut frame = json!({"sequence":sequence,"previous":previous,"event":event});
        events.push(event);
        model::project(&events)?;
        let sha256 = json!(digest::plan_hash(&frame)?);
        frame["sha256"] = sha256.clone();
        let line = encoded(&frame)?;
        let head_raw = encoded(&head(self.plan, self.generation, sequence, &sha256))?;
        let mut expected = self
            .state
            .as_ref()
            .map(|state| state.journal.raw.clone())
            .unwrap_or_default();
        if line.len() as u64 > JOURNAL_LIMIT.saturating_sub(expected.len() as u64)
            || head_raw.len() as u64 > HEAD_LIMIT
        {
            return Err("新增事件将超过账本上限，不能开始资源写入".into());
        }
        expected.extend_from_slice(&line);
        let journal = self.write_journal(&line, &expected)?;
        let head = self.publish_head(&journal, &head_raw)?;
        self.state = Some(State {
            journal,
            head,
            events,
            sha256,
        });
        Ok(())
    }

    fn check_state(&self, state: Option<&State>) -> Result<()> {
        if let Some(state) = state {
            self.window
                .check_image(JOURNAL, JOURNAL_LIMIT, &state.journal)?;
            self.window.check_image(HEAD, HEAD_LIMIT, &state.head)?;
        } else {
            self.window.require_absent(JOURNAL)?;
            self.window.require_absent(HEAD)?;
        }
        Ok(())
    }

    fn write_journal(&self, line: &[u8], expected: &[u8]) -> Result<Image> {
        self.window.check()?;
        let path = self.window.native_root.join(JOURNAL);
        let mut file = if self.state.is_some() {
            OpenOptions::new().append(true).open(&path)?
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?
        };
        let opened = Handle::from_file(file.try_clone()?)?;
        if opened != Handle::from_path(&path)? {
            return Err("复制账本 journal 在打开期间被替换".into());
        }
        if let Some(state) = &self.state {
            self.window
                .check_image(JOURNAL, JOURNAL_LIMIT, &state.journal)?;
            self.window.check_image(HEAD, HEAD_LIMIT, &state.head)?;
        } else {
            self.window.require_absent(HEAD)?;
        }
        self.window.check()?;
        file.write_all(line)?;
        file.flush()?;
        file.sync_all()?;
        let journal = self.window.read(JOURNAL, JOURNAL_LIMIT)?;
        if journal.raw != expected || opened != Handle::from_path(&path)? {
            return Err("复制账本 journal 写后身份或字节不同；保留现场".into());
        }
        Ok(journal)
    }

    fn publish_head(&self, journal: &Image, raw: &[u8]) -> Result<Image> {
        let name = format!("{HEAD}.{}", uuid::Uuid::new_v4().simple());
        let mut pending = tempfile::Builder::new()
            .prefix(&name)
            .rand_bytes(0)
            .suffix(".tmp")
            .disable_cleanup(true)
            .tempfile_in(&self.window.native_root)?;
        pending.write_all(raw)?;
        pending.flush()?;
        pending.as_file().sync_all()?;
        self.window.check()?;
        self.window.check_image(JOURNAL, JOURNAL_LIMIT, journal)?;
        if let Some(state) = &self.state {
            self.window.check_image(HEAD, HEAD_LIMIT, &state.head)?;
        } else {
            self.window.require_absent(HEAD)?;
        }
        self.window.check()?;
        let target = self.window.native_root.join(HEAD);
        // 同一受管锁下复核旧 head，再原子替换。原子发布不提供绕过锁的文件 ID CAS。
        let published = if self.state.is_some() {
            pending.persist(&target)
        } else {
            pending.persist_noclobber(&target)
        }
        .map_err(|error| error.error)?;
        let published = Handle::from_file(published)?;
        let head = self.window.read(HEAD, HEAD_LIMIT)?;
        if head.raw != raw || published != Handle::from_path(&target)? {
            return Err("复制账本 head 写后身份或字节不同；保留现场".into());
        }
        self.window.check_image(JOURNAL, JOURNAL_LIMIT, journal)?;
        self.window.check()?;
        Ok(head)
    }
}

struct Window<'a> {
    backend: &'a Path,
    root: PathBuf,
    native_root: PathBuf,
    identity: [(u64, u64); 3],
    verify_owner: &'a dyn Fn(&Path) -> Result<()>,
}

impl<'a> Window<'a> {
    fn new(
        backend: &'a Path,
        root: &Path,
        verify_owner: &'a dyn Fn(&Path) -> Result<()>,
    ) -> Result<Self> {
        let root = common::local_directory(backend, root, "复制账本目录")?;
        verify_owner(&root)?;
        let native_root = fs::canonicalize(&root)?;
        let identity = window_identity(backend, &root)?;
        let window = Self {
            backend,
            root,
            native_root,
            identity,
            verify_owner,
        };
        window.check()?;
        Ok(window)
    }

    fn check(&self) -> Result<()> {
        (self.verify_owner)(&self.root)?;
        if window_identity(self.backend, &self.root)? != self.identity
            || fs::canonicalize(&self.root)? != self.native_root
        {
            return Err("复制账本目录或受管锁身份发生变化；保留现场".into());
        }
        Ok(())
    }

    fn read(&self, name: &str, limit: u64) -> Result<Image> {
        read_image(self.backend, &self.root.join(name), limit)
    }

    fn check_image(&self, name: &str, limit: u64, expected: &Image) -> Result<()> {
        if &self.read(name, limit)? != expected {
            return Err("持锁期间账本文件身份或原始字节发生变化；保留现场".into());
        }
        Ok(())
    }

    fn require_absent(&self, name: &str) -> Result<()> {
        match fs::symlink_metadata(self.root.join(name)) {
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => Err("账本新收据不能覆盖既有文件".into()),
        }
    }
}

fn window_identity(backend: &Path, root: &Path) -> Result<[(u64, u64); 3]> {
    let root = common::local_directory(backend, root, "复制账本目录")?;
    let lock = common::local_directory(backend, &root.join(LOCK), "复制账本 clone.lock")?;
    // 内核 guard 正被持锁，Windows 只检查文件身份，不另开流读锁内容。
    let guard =
        validate_local_test_path(&root.join(GUARD), backend, LocalTestPathKind::ExistingFile)?;
    let identity = |path: &Path| common::device_inode_for(path, &fs::symlink_metadata(path)?);
    Ok([identity(&root)?, identity(&lock)?, identity(&guard)?])
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_ledger/storage.rs"]
mod tests;
