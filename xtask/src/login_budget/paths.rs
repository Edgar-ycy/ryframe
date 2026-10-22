use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
};

use same_file::Handle;

use super::BudgetError;

pub(super) fn validate(path: &Path) -> Result<(), BudgetError> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(BudgetError::Path);
    }
    Ok(())
}

pub(super) fn appended(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

pub(super) fn create_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(super) fn ordinary(metadata: &Metadata, maximum: u64) -> Result<(), BudgetError> {
    if !metadata.is_file() || linked(metadata) {
        return Err(BudgetError::File);
    }
    if metadata.len() > maximum {
        return Err(BudgetError::Size);
    }
    Ok(())
}

#[cfg(windows)]
fn linked(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn linked(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn directory(path: &Path) -> Result<Handle, BudgetError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| BudgetError::Path)?;
    if !metadata.is_dir() || linked(&metadata) {
        return Err(BudgetError::Path);
    }
    let held = Handle::from_path(path).map_err(|_| BudgetError::Path)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| BudgetError::Path)?;
    if !metadata.is_dir()
        || linked(&metadata)
        || held != Handle::from_path(path).map_err(|_| BudgetError::Path)?
    {
        return Err(BudgetError::Path);
    }
    Ok(held)
}

pub(super) struct Parents(Vec<(PathBuf, Handle)>);

impl Parents {
    pub(super) fn prepare(path: &Path) -> Result<Self, BudgetError> {
        validate(path)?;
        let mut held = Self(Vec::new());
        let mut current = PathBuf::new();
        for component in path.parent().ok_or(BudgetError::Path)?.components() {
            current.push(component.as_os_str());
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            held.verify()?;
            match fs::symlink_metadata(&current) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match fs::create_dir(&current) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(_) => return Err(BudgetError::Io),
                    }
                }
                Err(_) => return Err(BudgetError::Path),
            }
            held.0.push((current.clone(), directory(&current)?));
        }
        held.verify()?;
        Ok(held)
    }

    pub(super) fn verify(&self) -> Result<(), BudgetError> {
        for (path, held) in &self.0 {
            if *held != directory(path)? {
                return Err(BudgetError::Path);
            }
        }
        Ok(())
    }
}
