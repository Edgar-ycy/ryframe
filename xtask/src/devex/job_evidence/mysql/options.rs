//! 凭据仅存放在会话独占文件中；先收紧并核验权限，再写入密码。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[path = "permissions.rs"]
mod permissions;

pub(crate) struct OptionFile {
    directory: Option<PathBuf>,
    path: PathBuf,
    file_created: bool,
}

impl OptionFile {
    pub(crate) fn create(password: &str) -> Result<Self, &'static str> {
        let contents = encode(password)?;
        let root = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&root).map_err(|_| "mysql-option-file")?;
        for ancestor in root.ancestors() {
            let metadata = fs::symlink_metadata(ancestor).map_err(|_| "mysql-option-file")?;
            if super::invocation::linked(&metadata) || !metadata.is_dir() {
                return Err("mysql-option-file");
            }
        }
        let directory = tempfile::Builder::new()
            .prefix("job-mysql-")
            .tempdir_in(&root)
            .map_err(|_| "mysql-option-file")?
            .keep();
        let mut options = Self {
            path: directory.join("client.cnf"),
            directory: Some(directory),
            file_created: false,
        };
        if let Err(reason) = options.write(&contents) {
            options.close()?;
            return Err(reason);
        }
        Ok(options)
    }

    fn write(&mut self, contents: &str) -> Result<(), &'static str> {
        let directory = self.directory.as_ref().ok_or("mysql-option-file")?;
        permissions::secure_directory(directory)?;
        let mut open = OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = open.open(&self.path).map_err(|_| "mysql-option-file")?;
        self.file_created = true;
        permissions::verify(&self.path, false)?;
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|_| "mysql-option-file")?;
        self.verify()
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn verify(&self) -> Result<(), &'static str> {
        let directory = self.directory.as_ref().ok_or("mysql-option-file")?;
        permissions::verify(directory, true)?;
        permissions::verify(&self.path, false)
    }

    /// 只移除当前会话创建的文件和空目录，不递归清理未知内容。
    pub(crate) fn close(&mut self) -> Result<(), &'static str> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        if self.file_created {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("mysql-option-cleanup"),
            }
            self.file_created = false;
        }
        fs::remove_dir(directory).map_err(|_| "mysql-option-cleanup")?;
        self.directory = None;
        Ok(())
    }

    /// 进程终止未获确认时保留凭据文件，禁止在存活客户端下提前删除。
    pub(super) fn preserve(&mut self) {
        self.directory = None;
    }
}

impl Drop for OptionFile {
    fn drop(&mut self) {
        if self.close().is_err() {
            eprintln!("mysql-option-cleanup");
        }
    }
}

/// MySQL option-file 的双引号值；拒绝没有明确转义形式的控制字符。
pub(crate) fn encode(password: &str) -> Result<String, &'static str> {
    let mut output = String::from("[client]\npassword=\"");
    for character in password.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{8}' => output.push_str("\\b"),
            value if value.is_control() => return Err("mysql-password-encoding"),
            value => output.push(value),
        }
    }
    output.push_str("\"\n");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncreated_option_file_is_never_deleted_during_failure_cleanup() {
        let root = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        let path = directory.path().join("client.cnf");
        fs::write(&path, b"unrelated").unwrap();
        let mut options = OptionFile {
            directory: Some(directory.path().to_owned()),
            path: path.clone(),
            file_created: false,
        };
        #[cfg(windows)]
        {
            assert_eq!(
                options.write("private-password-must-not-be-written"),
                Err("mysql-option-permissions")
            );
            assert_eq!(fs::read(&path).unwrap(), b"unrelated");
        }
        assert_eq!(options.close(), Err("mysql-option-cleanup"));
        assert_eq!(fs::read(&path).unwrap(), b"unrelated");
        fs::remove_file(path).unwrap();
        options.close().unwrap();
    }
}
