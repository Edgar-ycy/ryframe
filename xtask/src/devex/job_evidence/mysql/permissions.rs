use std::{fs, path::Path};

pub(super) fn secure_directory(path: &Path) -> Result<(), &'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "mysql-option-permissions")?;
    }
    #[cfg(windows)]
    {
        crate::windows_acl::secure_directory(path).map_err(|_| "mysql-option-permissions")
    }
    #[cfg(not(windows))]
    verify(path, true)
}

pub(super) fn verify(path: &Path, directory: bool) -> Result<(), &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "mysql-option-permissions")?;
    if super::super::invocation::linked(&metadata)
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err("mysql-option-permissions");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let expected = if directory { 0o700 } else { 0o600 };
        if metadata.permissions().mode() & 0o777 != expected
            || metadata.uid() != nix::unistd::geteuid().as_raw()
        {
            return Err("mysql-option-permissions");
        }
    }
    #[cfg(windows)]
    crate::windows_acl::read(path)
        .and_then(|acl| acl.verify_mysql(false).map_err(Into::into))
        .map_err(|_| "mysql-option-permissions")?;
    #[cfg(not(any(unix, windows)))]
    return Err("mysql-option-permissions");
    Ok(())
}
