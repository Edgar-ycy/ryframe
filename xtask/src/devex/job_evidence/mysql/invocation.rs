use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;

pub(crate) type Environment = BTreeMap<OsString, OsString>;

/// 只构造命令，不启动客户端，也不修改当前进程的环境。
pub(crate) fn command(binding: &Value, environment: &Environment) -> Result<Command, &'static str> {
    let control = binding.get("control").ok_or("mysql-binding")?;
    let (port, username, tls) = connection(control)?;
    let password_name = text(control, "password_env")?;
    if !password_name
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_uppercase)
        || !password_name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("mysql-password-env");
    }
    let password = env_value(environment, password_name)?.ok_or("mysql-password-missing")?;
    if password.as_encoded_bytes().contains(&0) {
        return Err("mysql-password-env");
    }
    let (mut command, port) = if env_text(environment, "GITHUB_ACTIONS")? == Some("true") {
        docker_command(environment)?
    } else {
        let executable = Path::new(text(binding, "mysql_client")?);
        validate_client(executable)?;
        (Command::new(executable), port)
    };
    command.args([
        "--no-defaults".to_owned(),
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
    // 来源变量只由父进程读取，客户端只通过 MYSQL_PWD 接收本次密码。
    command
        .env_clear()
        .envs(environment)
        .env_remove(password_name)
        .env("MYSQL_PWD", password);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut command);
    Ok(command)
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

fn docker_command(environment: &Environment) -> Result<(Command, u16), &'static str> {
    let container =
        env_text(environment, "RYFRAME_CI_MYSQL_CONTAINER_ID")?.ok_or("mysql-ci-owner")?;
    let run = env_text(environment, "GITHUB_RUN_ID")?.ok_or("mysql-ci-owner")?;
    let attempt = env_text(environment, "GITHUB_RUN_ATTEMPT")?.ok_or("mysql-ci-owner")?;
    if !(12..=64).contains(&container.len())
        || !container
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !positive_decimal(run)
        || !positive_decimal(attempt)
        || env_text(environment, "APP_SCOPE_ID")? != Some(format!("ci-{run}-{attempt}").as_str())
    {
        return Err("mysql-ci-owner");
    }
    let mut command = Command::new("docker");
    command.args(["exec", "-i", "--env", "MYSQL_PWD", container, "mysql"]);
    Ok((command, 3306))
}

fn positive_decimal(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|byte| (b'1'..=b'9').contains(byte))
        && value.bytes().all(|byte| byte.is_ascii_digit())
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

/// 已知环境键遵循平台大小写语义；Windows 注入环境的大小写别名不得有歧义。
fn env_value<'a>(
    environment: &'a Environment,
    name: &str,
) -> Result<Option<&'a OsStr>, &'static str> {
    #[cfg(windows)]
    {
        let mut matches = environment
            .iter()
            .filter(|(key, _)| key.as_encoded_bytes().eq_ignore_ascii_case(name.as_bytes()));
        let value = matches.next().map(|(_, value)| value.as_os_str());
        if matches.next().is_some() {
            return Err("mysql-environment-ambiguous");
        }
        Ok(value)
    }
    #[cfg(not(windows))]
    {
        Ok(environment.get(OsStr::new(name)).map(OsString::as_os_str))
    }
}

fn env_text<'a>(environment: &'a Environment, name: &str) -> Result<Option<&'a str>, &'static str> {
    Ok(env_value(environment, name)?.and_then(OsStr::to_str))
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
fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0 || metadata.file_type().is_symlink()
}

#[cfg(not(windows))]
fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_window(_: &mut Command) {}
