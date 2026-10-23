use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
};

use serde_json::Value;

pub(crate) type Environment = BTreeMap<OsString, OsString>;

pub(crate) struct Invocation {
    pub(crate) command: Command,
    pub(crate) options: super::options::OptionFile,
}

impl Invocation {
    pub(crate) fn spawn(mut self) -> Result<(Child, super::options::OptionFile), &'static str> {
        match self.command.spawn() {
            Ok(child) => Ok((child, self.options)),
            Err(_) => {
                self.options.close()?;
                Err("mysql-spawn")
            }
        }
    }
}

impl std::ops::Deref for Invocation {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

/// 构造客户端及其独占凭据文件；调用方必须持有文件至进程退出。
pub(crate) fn command(
    binding: &Value,
    environment: &Environment,
) -> Result<Invocation, &'static str> {
    let control = binding.get("control").ok_or("mysql-binding")?;
    super::super::protocol::exact(
        control,
        &[
            "host", "port", "database", "username", "password", "tls_mode",
        ],
        "invalid_control_binding",
    )?;
    let (port, username, tls) = connection(control)?;
    let password = text(control, "password")?;
    let executable = Path::new(text(binding, "mysql_client")?);
    validate_client(executable)?;
    let options = super::options::OptionFile::create(password)?;
    let mut command = Command::new(executable);
    let mut first = OsString::from("--defaults-extra-file=");
    first.push(options.path());
    command.arg(first);
    command.args([
        "--host=127.0.0.1".to_owned(),
        format!("--port={port}"),
        format!("--user={username}"),
        "--protocol=TCP".to_owned(),
        "--default-character-set=utf8mb4".to_owned(),
        format!("--ssl-mode={tls}"),
    ]);
    command.args([
        "--batch",
        "--raw",
        "--unbuffered",
        "--skip-column-names",
        "--skip-reconnect",
        "--connect-timeout=5",
    ]);
    minimal_environment(&mut command, environment);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut command);
    Ok(Invocation { command, options })
}

/// 绝对客户端无需 PATH；仅 Windows 加载系统 DLL 所需的 OS 根目录可继承。
pub(super) fn minimal_environment(command: &mut Command, environment: &Environment) {
    command.env_clear();
    #[cfg(windows)]
    if let Some((_, value)) = environment
        .iter()
        .find(|(name, _)| name.as_encoded_bytes().eq_ignore_ascii_case(b"SystemRoot"))
    {
        command.env("SystemRoot", value);
    }
    #[cfg(not(windows))]
    let _ = environment;
}

fn connection(control: &Value) -> Result<(u16, &str, &'static str), &'static str> {
    if !matches!(text(control, "host")?, "localhost" | "127.0.0.1") {
        return Err("mysql-host");
    }
    let port = port(control.get("port").ok_or("mysql-port")?)?;
    let database = text(control, "database")?;
    if database.is_empty()
        || database.len() > 64
        || !database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("mysql-database");
    }
    let username = text(control, "username")?;
    if username.is_empty()
        || username.len() > 128
        || username.starts_with('-')
        || username.chars().any(char::is_control)
    {
        return Err("mysql-username");
    }
    let tls = match text(control, "tls_mode")? {
        "preferred" => "PREFERRED",
        "required" => "REQUIRED",
        "disabled" => "DISABLED",
        _ => return Err("mysql-tls"),
    };
    Ok((port, username, tls))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or("mysql-binding")
}

fn port(value: &Value) -> Result<u16, &'static str> {
    let number = match value {
        Value::String(text) => {
            if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("mysql-port");
            }
            text.parse::<u64>().map_err(|_| "mysql-port")?
        }
        Value::Number(number) => number.as_u64().ok_or("mysql-port")?,
        _ => return Err("mysql-port"),
    };
    u16::try_from(number)
        .ok()
        .filter(|number| *number != 0)
        .ok_or("mysql-port")
}

fn validate_client(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("mysql-client-path");
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| "mysql-client-path")?;
        if linked(&metadata) || (ancestor == path && !metadata.is_file()) {
            return Err("mysql-client-path");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)
            .map_err(|_| "mysql-client-path")?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err("mysql-client-path");
        }
    }
    validate_binary(path)
}

/// 拒绝同名脚本和普通占位文件，具体客户端来源仍由父层校验。
fn validate_binary(path: &Path) -> Result<(), &'static str> {
    use std::io::Read;
    let mut header = [0; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| "mysql-client-binary")?;
    #[cfg(windows)]
    let valid = header.starts_with(b"MZ");
    #[cfg(target_os = "linux")]
    let valid = header == *b"\x7fELF";
    #[cfg(not(any(windows, target_os = "linux")))]
    let valid = matches!(
        header,
        [0xfe, 0xed, 0xfa, 0xce | 0xcf]
            | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
    );
    if valid {
        Ok(())
    } else {
        Err("mysql-client-binary")
    }
}

#[cfg(windows)]
pub(super) fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0 || metadata.file_type().is_symlink()
}

#[cfg(not(windows))]
pub(super) fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
pub(super) fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
pub(super) fn hide_window(_: &mut Command) {}
