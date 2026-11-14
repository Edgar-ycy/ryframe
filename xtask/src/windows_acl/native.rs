use std::{
    fs::{self, File, OpenOptions},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsHandle,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use win_authorization_sys::{ObjectType, SecurityInformation, get_security_info};

use super::{Descriptor, ERROR, SYNCHRONIZE, descriptor};
use crate::Result;

const READ_CONTROL: u32 = 0x0002_0000;
const REPARSE_POINT: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;

pub(super) fn execute(operation: &str, path: &Path) -> Result<Descriptor> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let system = system_directory()?;
    let current = phase("读取当前 SID", current_sid(&system, deadline))?;
    let file = phase("打开并核对目录或文件身份", Guard::open(path))?;
    if operation == "secure-directory" {
        phase("确认新空目录", file.require_empty_directory())?;
        let subject = format!("*{current}");
        let owner = phase("变更前读取属主", file.owner())?;
        let mut stages = Vec::new();
        if owner != current {
            stages.push((
                "icacls 设置属主",
                vec!["/setowner".to_owned(), subject.clone()],
            ));
        }
        stages.extend([
            ("icacls 重置 DACL", vec!["/reset".to_owned()]),
            (
                "icacls 保护并授权",
                vec![
                    "/inheritance:r".to_owned(),
                    "/grant:r".to_owned(),
                    format!("{subject}:(OI)(CI)F"),
                ],
            ),
        ]);
        for (stage, arguments) in stages {
            phase("变更前核对空目录", file.require_empty_directory())?;
            let mut command = system_command(&system, "icacls.exe")?;
            command.arg(path).args(arguments);
            phase(stage, run(command, deadline))?;
            phase("变更后核对空目录", file.require_empty_directory())?;
        }
    } else if operation != "read" {
        return Err(ERROR.into());
    }
    let result = phase("读取并解析 owner/DACL", file.read(current))?;
    if operation == "secure-directory" {
        phase(
            "目录权限后验",
            result.verify_mysql(true).map_err(Into::into),
        )?;
        phase("后验空目录", file.require_empty_directory())?;
    }
    Ok(result)
}

fn phase<T>(stage: &'static str, result: Result<T>) -> Result<T> {
    result.map_err(|error| {
        let reason = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            .map(|code| format!("系统码 {code}"))
            .unwrap_or_else(|| error.to_string());
        format!("Windows ACL 阶段 {stage} 失败（{reason}）；保留现场，不自动重试").into()
    })
}

struct Guard {
    path: PathBuf,
    file: File,
    identity: same_file::Handle,
    directory: bool,
}

impl Guard {
    fn open(path: &Path) -> Result<Self> {
        validate_path(path)?;
        let before_file = phase("打开首个 READ_CONTROL | SYNCHRONIZE 句柄", open_file(path))?;
        let before = phase("核对首个句柄类型", checked_metadata(&before_file))?;
        let before_identity = same_file::Handle::from_file(before_file)?;
        let file = open_file(path)?;
        let opened = checked_metadata(&file)?;
        if opened.is_dir() != before.is_dir() {
            return Err(ERROR.into());
        }
        let identity = same_file::Handle::from_file(file.try_clone()?)?;
        if identity != before_identity {
            return Err(ERROR.into());
        }
        let guard = Self {
            path: path.to_owned(),
            file,
            identity,
            directory: opened.is_dir(),
        };
        guard.verify_path()?;
        Ok(guard)
    }

    fn verify_path(&self) -> Result<()> {
        validate_path(&self.path)?;
        let current = open_file(&self.path)?;
        if checked_metadata(&current)?.is_dir() != self.directory
            || same_file::Handle::from_file(current)? != self.identity
        {
            return Err(ERROR.into());
        }
        Ok(())
    }

    fn require_empty_directory(&self) -> Result<()> {
        self.verify_path()?;
        if !self.directory || fs::read_dir(&self.path)?.next().is_some() {
            return Err(ERROR.into());
        }
        Ok(())
    }

    fn read(&self, current: String) -> Result<Descriptor> {
        self.verify_path()?;
        let owned = get_security_info(
            self.file.as_handle(),
            ObjectType::FILE,
            SecurityInformation::OWNER | SecurityInformation::DACL,
        )
        .map_err(|_| ERROR)?;
        let result = descriptor::parse(owned.as_bytes(), current)?;
        self.verify_path()?;
        Ok(result)
    }

    fn owner(&self) -> Result<String> {
        self.verify_path()?;
        let owned = get_security_info(
            self.file.as_handle(),
            ObjectType::FILE,
            SecurityInformation::OWNER,
        )
        .map_err(|_| ERROR)?;
        let result = descriptor::owner(owned.as_bytes())?;
        self.verify_path()?;
        Ok(result)
    }
}

fn open_file(path: &Path) -> Result<File> {
    // 只申请安全描述符查询和同步句柄所需的权限。
    Ok(OpenOptions::new()
        .read(true)
        .access_mode(READ_CONTROL | SYNCHRONIZE)
        // 允许同一对象修改 ACL；持有期间禁止删除或重命名目标。
        .share_mode(3)
        .custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS)
        .open(path)?)
}

fn checked_metadata(file: &File) -> Result<fs::Metadata> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & REPARSE_POINT != 0
        || (!metadata.is_dir() && !metadata.is_file())
    {
        return Err(ERROR.into());
    }
    Ok(metadata)
}

fn validate_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(ERROR.into());
    }
    // 目标本身由 OPEN_REPARSE_POINT 句柄核验；祖先仍逐个拒绝链接和非目录。
    for ancestor in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(ancestor)?;
        if metadata.file_attributes() & REPARSE_POINT != 0 || !metadata.is_dir() {
            return Err(ERROR.into());
        }
    }
    Ok(())
}

pub(super) fn system_directory() -> Result<PathBuf> {
    let root = std::env::var_os("SystemRoot").ok_or(ERROR)?;
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err(ERROR.into());
    }
    Ok(root.join("System32"))
}

fn system_command(system: &Path, name: &str) -> Result<Command> {
    let mut command = Command::new(system.join(name));
    command
        .env_clear()
        .env("SystemRoot", system.parent().ok_or(ERROR)?);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.creation_flags(0x0800_0000);
    Ok(command)
}

fn run(command: Command, deadline: Instant) -> Result<Vec<u8>> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(ERROR)?;
    let output = crate::process::command_output_result_with_timeout(command, remaining)
        .map_err(|_| ERROR)?;
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "不可用".to_owned());
        return Err(format!("Windows ACL 系统命令退出码 {code}").into());
    }
    if output.stdout.len() > 64 * 1024 {
        return Err(ERROR.into());
    }
    Ok(output.stdout)
}

fn current_sid(system: &Path, deadline: Instant) -> Result<String> {
    let mut command = system_command(system, "whoami.exe")?;
    command.args(["/user", "/fo", "csv", "/nh"]);
    parse_whoami(&run(command, deadline)?).map_err(Into::into)
}

/// 用户名可采用系统代码页；只解析 CSV 结构和第二列的 ASCII 数值 SID。
pub(super) fn parse_whoami(bytes: &[u8]) -> std::result::Result<String, &'static str> {
    let mut rest = bytes;
    let name = csv_field(&mut rest)?;
    if name.is_empty() || rest.first() != Some(&b',') {
        return Err(ERROR);
    }
    rest = &rest[1..];
    let value = csv_field(&mut rest)?;
    if !matches!(rest, b"" | b"\n" | b"\r\n") {
        return Err(ERROR);
    }
    let sid = std::str::from_utf8(&value).map_err(|_| ERROR)?;
    let parts = sid.split('-').collect::<Vec<_>>();
    if !(4..=18).contains(&parts.len()) || parts[..2] != ["S", "1"] {
        return Err(ERROR);
    }
    let authority = parts[2].parse::<u64>().map_err(|_| ERROR)?;
    if authority > 0xffff_ffff_ffff || authority.to_string() != parts[2] {
        return Err(ERROR);
    }
    for part in &parts[3..] {
        let value = part.parse::<u32>().map_err(|_| ERROR)?;
        if value.to_string() != *part {
            return Err(ERROR);
        }
    }
    Ok(sid.to_owned())
}

fn csv_field(rest: &mut &[u8]) -> std::result::Result<Vec<u8>, &'static str> {
    if rest.first() != Some(&b'"') {
        return Err(ERROR);
    }
    *rest = &rest[1..];
    let mut value = Vec::new();
    while let Some((&byte, tail)) = rest.split_first() {
        *rest = tail;
        if byte == b'"' {
            if rest.first() != Some(&b'"') {
                return Ok(value);
            }
            *rest = &rest[1..];
        } else if matches!(byte, b'\r' | b'\n' | 0) {
            return Err(ERROR);
        }
        value.push(byte);
    }
    Err(ERROR)
}
