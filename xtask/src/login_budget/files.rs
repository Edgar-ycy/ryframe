use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use same_file::Handle;

use super::{
    Binding, BudgetError,
    locking::OwnedLock,
    model::Ledger,
    paths::{self, Parents},
};

const MAX_BYTES: u64 = 256 * 1024;

struct Snapshot {
    held: Handle,
    bytes: Vec<u8>,
}

pub(super) struct Current {
    pub(super) ledger: Ledger,
    original: Option<Snapshot>,
}

fn snapshot(path: &Path) -> Result<Option<Snapshot>, BudgetError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => paths::ordinary(&metadata, MAX_BYTES)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(BudgetError::Io),
    }
    let file = File::open(path).map_err(|_| BudgetError::Io)?;
    paths::ordinary(&file.metadata().map_err(|_| BudgetError::Io)?, MAX_BYTES)?;
    let held = Handle::from_file(file).map_err(|_| BudgetError::Io)?;
    let mut bytes = Vec::new();
    held.as_file()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BudgetError::Io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(BudgetError::Size);
    }
    paths::ordinary(
        &fs::symlink_metadata(path).map_err(|_| BudgetError::Io)?,
        MAX_BYTES,
    )?;
    if held != Handle::from_path(path).map_err(|_| BudgetError::Io)? {
        return Err(BudgetError::File);
    }
    Ok(Some(Snapshot { held, bytes }))
}

pub(super) fn read(path: &Path, binding: &Binding, now_ms: u64) -> Result<Current, BudgetError> {
    let original = snapshot(path)?;
    let ledger = match &original {
        None => Ledger::empty(binding.clone(), now_ms)?,
        Some(snapshot) => {
            serde_json::from_slice::<Ledger>(&snapshot.bytes).map_err(|_| BudgetError::Ledger)?
        }
    };
    ledger.validate(binding, now_ms)?;
    Ok(Current { ledger, original })
}

fn unchanged(path: &Path, original: &Option<Snapshot>) -> Result<(), BudgetError> {
    match (original, snapshot(path)?) {
        (None, None) => Ok(()),
        (Some(previous), Some(current))
            if previous.held == current.held && previous.bytes == current.bytes =>
        {
            Ok(())
        }
        _ => Err(BudgetError::File),
    }
}

pub(super) fn write(
    path: &Path,
    current: &Current,
    parents: &Parents,
    lock: &OwnedLock<'_>,
) -> Result<(), BudgetError> {
    let mut bytes = serde_json::to_vec(&current.ledger).map_err(|_| BudgetError::Ledger)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err(BudgetError::Size);
    }
    parents.verify()?;
    lock.verify()?;
    unchanged(path, &current.original)?;
    let mut temporary = Temporary::create(path, bytes)?;
    parents.verify()?;
    lock.verify()?;
    temporary.verify()?;
    unchanged(path, &current.original)?;
    // 一旦尝试发布就不删除未知临时文件，也不重试，以免丢失唯一证据。
    temporary.publication_attempted = true;
    fs::rename(&temporary.path, path).map_err(|_| BudgetError::PublicationUnknown)?;
    parents
        .verify()
        .map_err(|_| BudgetError::PublicationUnknown)?;
    lock.verify().map_err(|_| BudgetError::PublicationUnknown)?;
    let published = snapshot(path)
        .map_err(|_| BudgetError::PublicationUnknown)?
        .ok_or(BudgetError::PublicationUnknown)?;
    if published.held != temporary.held || published.bytes != temporary.bytes {
        return Err(BudgetError::PublicationUnknown);
    }
    Ok(())
}

struct Temporary {
    path: PathBuf,
    held: Handle,
    bytes: Vec<u8>,
    publication_attempted: bool,
}

impl Temporary {
    fn create(path: &Path, bytes: Vec<u8>) -> Result<Self, BudgetError> {
        let path = paths::appended(path, &format!(".{}.tmp", uuid::Uuid::new_v4()));
        let mut file = paths::create_new(&path).map_err(|_| BudgetError::Io)?;
        let held = Handle::from_file(file.try_clone().map_err(|_| BudgetError::Io)?)
            .map_err(|_| BudgetError::Io)?;
        file.write_all(&bytes).map_err(|_| BudgetError::Io)?;
        file.sync_all().map_err(|_| BudgetError::Io)?;
        Ok(Self {
            path,
            held,
            bytes,
            publication_attempted: false,
        })
    }

    fn verify(&self) -> Result<(), BudgetError> {
        let current = snapshot(&self.path)?.ok_or(BudgetError::File)?;
        if current.held != self.held || current.bytes != self.bytes {
            return Err(BudgetError::File);
        }
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.publication_attempted && self.verify().is_ok() {
            let _ = fs::remove_file(&self.path);
        }
    }
}
